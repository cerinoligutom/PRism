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
                    updated_at: row.get(4)?,
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
