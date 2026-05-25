//! Tauri command surface for the `app_settings` singleton.
//!
//! Commands shipping here:
//!
//! * [`get_app_settings`] - read the singleton row. The migration's CHECK +
//!   seed INSERT guarantee exactly one row, so the read never has to handle
//!   an empty case. Called on app boot and whenever the Settings panel mounts.
//! * [`update_app_settings`] - UPDATE the singleton row with the caller's
//!   prefs payload, bump `updated_at`, and return the post-write state. The
//!   Settings panel uses the round-trip to render "Updated <relative>"
//!   affordances against the same struct it sent.
//! * [`set_notification_permission_state`] - persist the OS-grant outcome
//!   answered by the panel's explicit-ask path (ADR 0017 decision 5).
//! * [`set_last_seen_version`] - persist the in-app "What's new" dialog's
//!   version cursor (ADR 0025): written by the launch hook on first run, and
//!   by the dialog dismiss handler on every subsequent version transition.
//!
//! `notification_permission_state`, `last_seen_version`, and the two
//! `auto_update_last_*` columns are intentionally _not_ rewritten by
//! `update_app_settings` - they are owned by non-Settings gestures (OS
//! permission prompt, "What's new" dialog dismiss, updater worker), and
//! letting the panel echo stale values back would either re-prompt the OS,
//! re-suppress the dialog, or stomp on the latest check's bookkeeping.
//!
//! Errors are stringified for the renderer (matches the existing settings
//! style; the type itself is internal so leaking the rusqlite message is
//! fine).

use tauri::State;

use crate::db::DbHandle;
use crate::settings::types::{AppSettings, NotificationPermissionState};

/// Inclusive cap on the persisted auto-archive window. The migration's
/// CHECK constraint enforces the same bound, but clamping at the writer
/// gives a calm clamp-to-edge behaviour for an over-eager UI rather than
/// a noisy CHECK failure. Issue #333.
const AUTO_ARCHIVE_DAYS_MAX: i64 = 365;

/// Inclusive bounds on the persisted notifications-inbox row cap (issue
/// #380). The migration's CHECK constraint enforces the same range; the
/// writer clamps so an over-eager UI doesn't trip the CHECK.
const NOTIFICATION_RETENTION_MIN: i64 = 50;
const NOTIFICATION_RETENTION_MAX: i64 = 5000;

/// Read the singleton `app_settings` row. Called by the Settings panel and
/// by the frontend's app-level notification preference store.
///
/// `async` so the renderer can `await` without thinking; the body is a
/// single short SQL read so there's no actual yield point.
#[tauri::command]
pub async fn get_app_settings(db: State<'_, DbHandle>) -> Result<AppSettings, String> {
    let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
    AppSettings::load(&conn).map_err(|e| format!("load app_settings: {e}"))
}

/// UPDATE the singleton row with the caller's prefs and return the
/// post-write state. The `updated_at` column is bumped server-side so
/// inbound payloads never have to think about it.
///
/// The permission-state field on the inbound payload is _ignored_: it is
/// owned by the notification sink's deferred-ask flow (ADR 0017 decision 5)
/// and gets read back from the DB before being returned to the caller. This
/// keeps the Settings panel from accidentally re-prompting the OS by
/// echoing a stale state.
#[tauri::command]
pub async fn update_app_settings(
    db: State<'_, DbHandle>,
    prefs: AppSettings,
) -> Result<AppSettings, String> {
    let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
    // The UPDATE deliberately leaves `notification_permission_state`
    // untouched: the OS-grant lifecycle lives inside the notification sink
    // (ADR 0017 decision 5), and letting the panel echo back a stale state
    // would re-prompt the user on every save.
    let auto_archive_days = prefs.auto_archive_days.clamp(0, AUTO_ARCHIVE_DAYS_MAX);
    let notification_retention_max = prefs
        .notification_retention_max
        .clamp(NOTIFICATION_RETENTION_MIN, NOTIFICATION_RETENTION_MAX);
    conn.execute(
        "UPDATE app_settings
            SET notifications_enabled = ?1,
                notify_on_needs_attention = ?2,
                notify_on_mention = ?3,
                auto_update_enabled = ?4,
                auto_update_interval_seconds = ?5,
                auto_archive_days = ?6,
                notification_retention_max = ?7,
                updated_at = strftime('%s', 'now')
          WHERE id = 1",
        rusqlite::params![
            prefs.notifications_enabled as i64,
            prefs.notify_on_needs_attention as i64,
            prefs.notify_on_mention as i64,
            prefs.auto_update_enabled as i64,
            prefs.auto_update_interval_seconds,
            auto_archive_days,
            notification_retention_max,
        ],
    )
    .map_err(|e| format!("update app_settings: {e}"))?;
    AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
}

/// Record the outcome of an updater check. Called by the updater worker
/// after every poll (background or manual). On success, the failure column
/// is cleared so the Settings panel's "Last check failed" line disappears
/// on the next render. On failure, the message is truncated so a verbose
/// error doesn't blow out the column.
///
/// Kept off `update_app_settings` for the same reason the notification
/// permission state is: the column is owned by a non-Settings gesture (the
/// worker), and letting the panel echo a stale value would corrupt the
/// failure bookkeeping. Mirrors the ADR-0017 decision-5 pattern.
#[tauri::command]
pub async fn record_update_check(
    db: State<'_, DbHandle>,
    success: bool,
    failure_message: Option<String>,
) -> Result<AppSettings, String> {
    let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
    let truncated = failure_message.as_ref().map(|s| truncate_failure(s));
    let stored_failure: Option<&str> = if success { None } else { truncated.as_deref() };
    conn.execute(
        "UPDATE app_settings
            SET auto_update_last_check_at = strftime('%s', 'now'),
                auto_update_last_failure_message = ?1,
                updated_at = strftime('%s', 'now')
          WHERE id = 1",
        rusqlite::params![stored_failure],
    )
    .map_err(|e| format!("record update check: {e}"))?;
    AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
}

/// Cap the persisted failure message so a verbose underlying error doesn't
/// blow out the column. The Settings panel renders a single line, so any
/// detail past the ellipsis would be off-screen anyway.
fn truncate_failure(raw: &str) -> String {
    const MAX: usize = 240;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}...", &raw[..MAX])
    }
}

/// Persist the OS-reported `notification_permission_state` answered by an
/// explicit panel-driven request.
///
/// The Settings panel calls the `tauri-plugin-notification` `requestPermission`
/// API from the frontend on the user gesture (master switch flipping ON while
/// the slot is `unprompted`), then echoes the result here so the DB is the
/// single source of truth the notification sink reads on dispatch.
///
/// This is intentionally separate from `update_app_settings`, which never
/// writes the permission column (ADR 0017 decision 5): the sink owns the
/// deferred-ask flow, the panel owns the explicit-ask flow, and both feed
/// the same column.
#[tauri::command]
pub async fn set_notification_permission_state(
    db: State<'_, DbHandle>,
    state: NotificationPermissionState,
) -> Result<AppSettings, String> {
    let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
    let stored = match state {
        NotificationPermissionState::Unprompted => "unprompted",
        NotificationPermissionState::Granted => "granted",
        NotificationPermissionState::Denied => "denied",
    };
    conn.execute(
        "UPDATE app_settings
            SET notification_permission_state = ?1,
                updated_at = strftime('%s', 'now')
          WHERE id = 1",
        rusqlite::params![stored],
    )
    .map_err(|e| format!("update notification_permission_state: {e}"))?;
    AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
}

/// Persist the version cursor used by the in-app "What's new" dialog
/// (ADR 0025).
///
/// Two call sites write here:
///   * The launch hook on first run, when `last_seen_version` is `NULL`. It
///     echoes the current `app_metadata.version` so the dialog suppresses
///     itself on a fresh install and only fires on the next version
///     transition.
///   * The dialog's dismiss handler (close button, Esc, "Got it" CTA), which
///     advances the cursor to the running version after the user has
///     acknowledged the changelog.
///
/// Kept off `update_app_settings` for the same reason
/// `notification_permission_state` is: the column is owned by a non-Settings
/// gesture, and letting the Settings panel echo a stale cursor would
/// silently re-suppress the dialog. Mirrors the ADR 0017 decision 5 pattern.
#[tauri::command]
pub async fn set_last_seen_version(
    db: State<'_, DbHandle>,
    version: String,
) -> Result<AppSettings, String> {
    let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
    conn.execute(
        "UPDATE app_settings
            SET last_seen_version = ?1,
                updated_at = strftime('%s', 'now')
          WHERE id = 1",
        rusqlite::params![version],
    )
    .map_err(|e| format!("update last_seen_version: {e}"))?;
    AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::types::NotificationPermissionState;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn fresh_db() -> DbHandle {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        Arc::new(Mutex::new(conn))
    }

    /// Drive the read path without going through the Tauri `State<...>`
    /// container. Mirrors the body of [`get_app_settings`].
    fn invoke_get(db: &DbHandle) -> Result<AppSettings, String> {
        let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
        AppSettings::load(&conn).map_err(|e| format!("load app_settings: {e}"))
    }

    /// Drive the write path without going through `State<...>`. Mirrors
    /// [`update_app_settings`].
    fn invoke_update(db: &DbHandle, prefs: AppSettings) -> Result<AppSettings, String> {
        let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
        let auto_archive_days = prefs.auto_archive_days.clamp(0, AUTO_ARCHIVE_DAYS_MAX);
        let notification_retention_max = prefs
            .notification_retention_max
            .clamp(NOTIFICATION_RETENTION_MIN, NOTIFICATION_RETENTION_MAX);
        conn.execute(
            "UPDATE app_settings
                SET notifications_enabled = ?1,
                    notify_on_needs_attention = ?2,
                    notify_on_mention = ?3,
                    auto_update_enabled = ?4,
                    auto_update_interval_seconds = ?5,
                    auto_archive_days = ?6,
                    notification_retention_max = ?7,
                    updated_at = strftime('%s', 'now')
              WHERE id = 1",
            rusqlite::params![
                prefs.notifications_enabled as i64,
                prefs.notify_on_needs_attention as i64,
                prefs.notify_on_mention as i64,
                prefs.auto_update_enabled as i64,
                prefs.auto_update_interval_seconds,
                auto_archive_days,
                notification_retention_max,
            ],
        )
        .map_err(|e| format!("update app_settings: {e}"))?;
        AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
    }

    /// Drive the record-update-check write path without going through `State<...>`.
    /// Mirrors [`record_update_check`].
    fn invoke_record_check(
        db: &DbHandle,
        success: bool,
        failure_message: Option<&str>,
    ) -> Result<AppSettings, String> {
        let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
        let stored_failure: Option<&str> = if success { None } else { failure_message };
        conn.execute(
            "UPDATE app_settings
                SET auto_update_last_check_at = strftime('%s', 'now'),
                    auto_update_last_failure_message = ?1,
                    updated_at = strftime('%s', 'now')
              WHERE id = 1",
            rusqlite::params![stored_failure],
        )
        .map_err(|e| format!("record update check: {e}"))?;
        AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
    }

    #[test]
    fn get_returns_seeded_defaults_on_fresh_db() {
        let db = fresh_db();
        let settings = invoke_get(&db).expect("get app_settings");
        assert!(settings.notifications_enabled, "master defaults ON");
        assert!(settings.notify_on_needs_attention);
        assert!(settings.notify_on_mention);
        assert_eq!(
            settings.notification_permission_state,
            NotificationPermissionState::Unprompted
        );
        assert_eq!(
            settings.last_seen_version, None,
            "last_seen_version starts NULL on a fresh install (ADR 0025)"
        );
        assert!(
            !settings.auto_update_enabled,
            "auto-update defaults OFF on fresh install (ADR-0024)"
        );
        assert_eq!(settings.auto_update_interval_seconds, 21600);
        assert_eq!(settings.auto_update_last_check_at, None);
        assert_eq!(settings.auto_update_last_failure_message, None);
        assert!(settings.updated_at > 0);
    }

    #[test]
    fn update_round_trips_with_advanced_updated_at() {
        let db = fresh_db();
        let before = invoke_get(&db).expect("seed read");
        // Sleep one second so the strftime('%s','now') epoch advances; the
        // schema stores integer seconds.
        std::thread::sleep(std::time::Duration::from_secs(1));
        let payload = AppSettings {
            notifications_enabled: true,
            notify_on_needs_attention: false,
            notify_on_mention: true,
            // Permission state on the payload is ignored by the writer; we
            // pass an arbitrary value to confirm it doesn't leak back.
            notification_permission_state: NotificationPermissionState::Granted,
            last_seen_version: Some("9.9.9".into()),
            auto_update_enabled: true,
            auto_update_interval_seconds: 21600,
            // Last-check fields on the payload are ignored by the writer;
            // arbitrary values here confirm they don't leak back.
            auto_update_last_check_at: Some(42),
            auto_update_last_failure_message: Some("stale failure".into()),
            auto_archive_days: 45,
            notification_retention_max: 1000,
            updated_at: 0,
        };
        let after = invoke_update(&db, payload).expect("update app_settings");
        assert!(after.notifications_enabled);
        assert!(!after.notify_on_needs_attention);
        assert!(after.notify_on_mention);
        assert_eq!(
            after.notification_permission_state,
            NotificationPermissionState::Unprompted,
            "writer must never echo a stale permission state from the payload"
        );
        assert_eq!(
            after.last_seen_version, None,
            "writer must never echo a stale last_seen_version from the payload (ADR 0025)"
        );
        assert!(
            after.auto_update_enabled,
            "auto-update toggle persists through the writer"
        );
        assert_eq!(
            after.auto_update_last_check_at, None,
            "writer must never echo a stale last-check timestamp"
        );
        assert_eq!(
            after.auto_update_last_failure_message, None,
            "writer must never echo a stale failure message"
        );
        assert_eq!(
            after.auto_archive_days, 45,
            "auto-archive window persists through the writer (issue #333)"
        );
        assert_eq!(
            after.notification_retention_max, 1000,
            "notifications retention cap persists through the writer (issue #380)"
        );
        assert!(
            after.updated_at > before.updated_at,
            "updated_at must advance on write"
        );
    }

    /// Drive the permission-state write path without going through `State<...>`.
    /// Mirrors [`set_notification_permission_state`].
    fn invoke_set_perm(
        db: &DbHandle,
        state: NotificationPermissionState,
    ) -> Result<AppSettings, String> {
        let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
        let stored = match state {
            NotificationPermissionState::Unprompted => "unprompted",
            NotificationPermissionState::Granted => "granted",
            NotificationPermissionState::Denied => "denied",
        };
        conn.execute(
            "UPDATE app_settings
                SET notification_permission_state = ?1,
                    updated_at = strftime('%s', 'now')
              WHERE id = 1",
            rusqlite::params![stored],
        )
        .map_err(|e| format!("update notification_permission_state: {e}"))?;
        AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
    }

    #[test]
    fn set_permission_state_persists_grant_and_round_trips() {
        // Panel-driven explicit-ask path (ADR 0017 decision 5): when the user
        // toggles master ON and the OS prompt returns Granted, the frontend
        // echoes the result here so the sink reads the new state on dispatch.
        let db = fresh_db();
        let after = invoke_set_perm(&db, NotificationPermissionState::Granted)
            .expect("set permission state");
        assert_eq!(
            after.notification_permission_state,
            NotificationPermissionState::Granted
        );
    }

    #[test]
    fn set_permission_state_persists_denial() {
        // Mirrors the case where the OS reports Denied (or the user dismisses
        // the prompt). The panel then renders the "blocked" callout off the
        // persisted state without re-prompting.
        let db = fresh_db();
        let after = invoke_set_perm(&db, NotificationPermissionState::Denied)
            .expect("set permission state");
        assert_eq!(
            after.notification_permission_state,
            NotificationPermissionState::Denied
        );
    }

    #[test]
    fn update_clears_both_trigger_toggles() {
        // Settings panel "mute everything via toggles" path. Master can stay
        // ON while both per-trigger flags are OFF; the recompute emitter
        // checks the per-trigger flags before invoking the sink (#192).
        let db = fresh_db();
        let after = invoke_update(
            &db,
            AppSettings {
                notifications_enabled: true,
                notify_on_needs_attention: false,
                notify_on_mention: false,
                notification_permission_state: NotificationPermissionState::Unprompted,
                last_seen_version: None,
                auto_update_enabled: false,
                auto_update_interval_seconds: 21600,
                auto_update_last_check_at: None,
                auto_update_last_failure_message: None,
                auto_archive_days: 30,
                notification_retention_max: 500,
                updated_at: 0,
            },
        )
        .expect("update");
        assert!(after.notifications_enabled);
        assert!(!after.notify_on_needs_attention);
        assert!(!after.notify_on_mention);
    }

    /// Drive the last-seen-version write path without going through `State<...>`.
    /// Mirrors [`set_last_seen_version`].
    fn invoke_set_last_seen(db: &DbHandle, version: &str) -> Result<AppSettings, String> {
        let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
        conn.execute(
            "UPDATE app_settings
                SET last_seen_version = ?1,
                    updated_at = strftime('%s', 'now')
              WHERE id = 1",
            rusqlite::params![version],
        )
        .map_err(|e| format!("update last_seen_version: {e}"))?;
        AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
    }

    #[test]
    fn set_last_seen_version_persists_initial_cursor() {
        // First-launch path (ADR 0025): the launch hook detects
        // `last_seen_version IS NULL` and silently writes the current binary
        // version so the dialog stays suppressed until the next transition.
        let db = fresh_db();
        let after = invoke_set_last_seen(&db, "0.1.0").expect("set last_seen_version");
        assert_eq!(after.last_seen_version, Some("0.1.0".into()));
    }

    #[test]
    fn set_last_seen_version_advances_cursor_on_dismiss() {
        // Dialog-dismiss path (ADR 0025): after the user acknowledges the
        // concatenated changelog, the cursor advances to the running version
        // so the next launch on the same binary doesn't re-show the dialog.
        let db = fresh_db();
        invoke_set_last_seen(&db, "0.1.0").expect("seed prior cursor");
        let after = invoke_set_last_seen(&db, "0.4.0").expect("advance cursor");
        assert_eq!(after.last_seen_version, Some("0.4.0".into()));
    }

    #[test]
    fn record_update_check_success_clears_failure_message() {
        // Worker happy path (ADR-0024): a successful check stamps the
        // last-check timestamp and clears any prior failure message so the
        // Settings panel's "Last check failed" line disappears on the next
        // render.
        let db = fresh_db();
        invoke_record_check(&db, false, Some("network down")).expect("seed failure");
        let after = invoke_record_check(&db, true, None).expect("record success");
        assert!(
            after.auto_update_last_check_at.is_some(),
            "timestamp must advance on every check"
        );
        assert_eq!(
            after.auto_update_last_failure_message, None,
            "success must clear the failure message"
        );
    }

    #[test]
    fn record_update_check_failure_persists_message() {
        // Worker failure path (ADR-0024): the message is the only signal the
        // user sees, so it has to survive the round-trip. No toast, no
        // banner.
        let db = fresh_db();
        let after = invoke_record_check(&db, false, Some("manifest 404")).expect("record failure");
        assert!(after.auto_update_last_check_at.is_some());
        assert_eq!(
            after.auto_update_last_failure_message.as_deref(),
            Some("manifest 404")
        );
    }

    /// Build a payload that mutates `auto_archive_days` to `value` while
    /// leaving the rest at the migration defaults. Keeps the clamp tests
    /// from carrying noise from unrelated fields.
    fn payload_with_archive_days(value: i64) -> AppSettings {
        AppSettings {
            notifications_enabled: true,
            notify_on_needs_attention: true,
            notify_on_mention: true,
            notification_permission_state: NotificationPermissionState::Unprompted,
            last_seen_version: None,
            auto_update_enabled: false,
            auto_update_interval_seconds: 21600,
            auto_update_last_check_at: None,
            auto_update_last_failure_message: None,
            auto_archive_days: value,
            notification_retention_max: 500,
            updated_at: 0,
        }
    }

    /// Build a payload that mutates `notification_retention_max` to `value`
    /// while leaving the rest at the migration defaults. Mirrors
    /// [`payload_with_archive_days`] so the retention clamp tests stay
    /// noise-free.
    fn payload_with_retention(value: i64) -> AppSettings {
        AppSettings {
            notifications_enabled: true,
            notify_on_needs_attention: true,
            notify_on_mention: true,
            notification_permission_state: NotificationPermissionState::Unprompted,
            last_seen_version: None,
            auto_update_enabled: false,
            auto_update_interval_seconds: 21600,
            auto_update_last_check_at: None,
            auto_update_last_failure_message: None,
            auto_archive_days: 30,
            notification_retention_max: value,
            updated_at: 0,
        }
    }

    #[test]
    fn update_persists_zero_auto_archive_days_to_disable_sweep() {
        // Issue #333: setting the window to 0 disables the auto-archive
        // sweep entirely. The writer must accept 0 without clamping it up.
        let db = fresh_db();
        let after = invoke_update(&db, payload_with_archive_days(0)).expect("update");
        assert_eq!(after.auto_archive_days, 0);
    }

    #[test]
    fn update_clamps_auto_archive_days_above_cap() {
        // Defence in depth: the migration's CHECK already caps at 365, but
        // the writer clamps to the same bound so an over-eager UI never
        // triggers a CHECK failure on the persist round-trip.
        let db = fresh_db();
        let after = invoke_update(&db, payload_with_archive_days(9_999)).expect("update");
        assert_eq!(after.auto_archive_days, 365);
    }

    #[test]
    fn update_clamps_negative_auto_archive_days_to_zero() {
        // Negative values reach the writer as plain `i64`s from the renderer;
        // the migration's CHECK rejects them, so clamp to the disable
        // sentinel rather than surface a CHECK failure.
        let db = fresh_db();
        let after = invoke_update(&db, payload_with_archive_days(-7)).expect("update");
        assert_eq!(after.auto_archive_days, 0);
    }

    #[test]
    fn update_persists_retention_value_within_bounds() {
        // Happy path (issue #380): a value inside `[50, 5000]` round-trips
        // unchanged.
        let db = fresh_db();
        let after = invoke_update(&db, payload_with_retention(250)).expect("update");
        assert_eq!(after.notification_retention_max, 250);
    }

    #[test]
    fn update_clamps_retention_above_cap() {
        // Defence in depth: the migration's CHECK caps at 5000; the writer
        // clamps so the round-trip doesn't surface a CHECK failure.
        let db = fresh_db();
        let after = invoke_update(&db, payload_with_retention(9_999)).expect("update");
        assert_eq!(after.notification_retention_max, 5000);
    }

    #[test]
    fn update_clamps_retention_below_floor() {
        // The lower bound is 50 (issue #380). Anything smaller clamps up to
        // 50 so the CHECK constraint isn't tripped.
        let db = fresh_db();
        let after = invoke_update(&db, payload_with_retention(10)).expect("update");
        assert_eq!(after.notification_retention_max, 50);
    }
}
