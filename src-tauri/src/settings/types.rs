//! Typed mirror of the `app_settings` singleton row and supporting enums.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// OS-level notification permission as reported by `tauri-plugin-notification`.
///
/// Persisted to `app_settings.notification_permission_state` so the Settings
/// panel can render the right call-to-action without re-asking the OS on every
/// open. Reasoning lives in ADR 0017 (decision 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationPermissionState {
    /// The user has not been asked yet. The notification sink will trigger the
    /// OS prompt the first time it would dispatch a toast.
    Unprompted,
    /// The OS granted permission; toasts may fire when the prefs allow.
    Granted,
    /// The OS denied permission. Toasts are skipped; the Settings panel shows
    /// a callout pointing at the OS notification preferences.
    Denied,
}

impl NotificationPermissionState {
    pub(crate) fn from_storage(value: &str) -> Self {
        match value {
            "granted" => Self::Granted,
            "denied" => Self::Denied,
            // Forward-compatibility: any unrecognised value treats the slot as
            // never-asked, which is the safe default (the sink will re-prompt
            // and persist the OS answer). The migration's only seeded value is
            // `unprompted` so this also handles a future write that hits an
            // older binary which hasn't learned the new variant yet.
            _ => Self::Unprompted,
        }
    }
}

/// Typed mirror of the singleton row in `app_settings`. Always reflects the
/// row pinned at `id = 1` (the CHECK constraint and the migration's seed
/// INSERT guarantee exactly one row).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    pub notifications_enabled: bool,
    pub notify_on_needs_attention: bool,
    pub notify_on_mention: bool,
    pub notification_permission_state: NotificationPermissionState,
    /// Last app version the user dismissed the in-app "What's new" dialog
    /// against. `None` means the cursor has never been written (fresh
    /// install) - the launch hook records the current version silently and
    /// suppresses the dialog so a first-time user isn't greeted with a
    /// "what's new" they have no context for. Subsequent version transitions
    /// open the dialog, which writes the new version on dismiss. See
    /// ADR 0025 for the full design.
    pub last_seen_version: Option<String>,
    /// Auto-update toggle. Defaults to `false` per ADR-0024 (opt-in). When
    /// `true`, the `update::worker` polls the manifest endpoint on the
    /// configured interval; when `false`, no network calls happen for
    /// updates.
    pub auto_update_enabled: bool,
    /// Auto-update poll cadence in seconds. Defaults to 21600 (6 hours) per
    /// ADR-0024; v1.x ships this fixed value, the column is here so a
    /// future configurable-cadence setting has the natural home.
    pub auto_update_interval_seconds: i64,
    /// Unix seconds of the last update check attempt (success or failure).
    /// `None` means no check has ever run; the Settings panel renders
    /// nothing in that case.
    pub auto_update_last_check_at: Option<i64>,
    /// Short human-readable error from the last failed check, or `None`
    /// when the last check succeeded. The Settings panel surfaces
    /// "Last check failed: <message>" iff this column is set; cleared on
    /// the next successful check. ADR-0024's silent-failure policy: no
    /// toast, no banner, just this line in the Settings panel.
    pub auto_update_last_failure_message: Option<String>,
    /// Unix seconds. Updated by the writer command on every change so the
    /// frontend can show "Updated <relative>" affordances if needed.
    pub updated_at: i64,
}

impl AppSettings {
    /// Load the singleton row. Returns an error only if the table is missing
    /// or the row was somehow deleted - both indicate a corrupted DB, which
    /// is also what the migration's CHECK / seed INSERT prevent.
    pub fn load(conn: &Connection) -> rusqlite::Result<Self> {
        conn.query_row(
            "SELECT notifications_enabled,
                    notify_on_needs_attention,
                    notify_on_mention,
                    notification_permission_state,
                    last_seen_version,
                    auto_update_enabled,
                    auto_update_interval_seconds,
                    auto_update_last_check_at,
                    auto_update_last_failure_message,
                    updated_at
               FROM app_settings
              WHERE id = 1",
            [],
            |row| {
                let perm: String = row.get(3)?;
                Ok(Self {
                    notifications_enabled: row.get::<_, i64>(0)? != 0,
                    notify_on_needs_attention: row.get::<_, i64>(1)? != 0,
                    notify_on_mention: row.get::<_, i64>(2)? != 0,
                    notification_permission_state: NotificationPermissionState::from_storage(&perm),
                    last_seen_version: row.get(4)?,
                    auto_update_enabled: row.get::<_, i64>(5)? != 0,
                    auto_update_interval_seconds: row.get(6)?,
                    auto_update_last_check_at: row.get(7)?,
                    auto_update_last_failure_message: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrate;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        migrate::run(&mut conn).expect("run migrations");
        conn
    }

    #[test]
    fn defaults_match_migration_seed() {
        let conn = fresh();
        let settings = AppSettings::load(&conn).expect("load settings");
        assert!(settings.notifications_enabled, "master defaults ON");
        assert!(
            settings.notify_on_needs_attention,
            "needs-attention trigger defaults ON"
        );
        assert!(settings.notify_on_mention, "mention trigger defaults ON");
        assert_eq!(
            settings.notification_permission_state,
            NotificationPermissionState::Unprompted,
            "permission state starts unprompted"
        );
        assert_eq!(
            settings.last_seen_version, None,
            "last_seen_version starts NULL so the first launch is the silent-record path (ADR 0025)"
        );
        assert!(
            !settings.auto_update_enabled,
            "auto-update defaults OFF (opt-in per ADR-0024)"
        );
        assert_eq!(
            settings.auto_update_interval_seconds, 21600,
            "auto-update interval defaults to 6h per ADR-0024"
        );
        assert_eq!(settings.auto_update_last_check_at, None);
        assert_eq!(settings.auto_update_last_failure_message, None);
        assert!(settings.updated_at > 0, "updated_at seeded to now()");
    }

    #[test]
    fn permission_state_decodes_known_storage_values() {
        assert_eq!(
            NotificationPermissionState::from_storage("unprompted"),
            NotificationPermissionState::Unprompted,
        );
        assert_eq!(
            NotificationPermissionState::from_storage("granted"),
            NotificationPermissionState::Granted,
        );
        assert_eq!(
            NotificationPermissionState::from_storage("denied"),
            NotificationPermissionState::Denied,
        );
    }

    #[test]
    fn unknown_permission_storage_falls_back_to_unprompted() {
        // Forward-compatibility guard: a future ADR that adds a new variant
        // and downgrades the binary must not panic; the safe default is
        // unprompted so the sink will re-ask and re-persist.
        assert_eq!(
            NotificationPermissionState::from_storage("not-a-real-state"),
            NotificationPermissionState::Unprompted
        );
    }
}
