//! Tauri command surface for the `app_settings` singleton.
//!
//! Two commands ship here:
//!
//! * [`get_app_settings`] - read the singleton row. The migration's CHECK +
//!   seed INSERT guarantee exactly one row, so the read never has to handle
//!   an empty case. Called on app boot and whenever the Settings panel mounts.
//! * [`update_app_settings`] - UPDATE the singleton row with the caller's
//!   prefs payload, bump `updated_at`, and return the post-write state. The
//!   Settings panel uses the round-trip to render "Updated <relative>"
//!   affordances against the same struct it sent.
//!
//! `notification_permission_state` is intentionally _not_ rewritten by this
//! command - the OS-grant lifecycle lives inside the notification sink (ADR
//! 0017 decision 5), and letting the frontend overwrite it from a stale view
//! would re-prompt the user on every panel save.
//!
//! Errors are stringified for the renderer (matches the existing settings
//! style; the type itself is internal so leaking the rusqlite message is
//! fine).

use tauri::State;

use crate::db::DbHandle;
use crate::settings::types::AppSettings;

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
    conn.execute(
        "UPDATE app_settings
            SET notifications_enabled = ?1,
                notify_on_needs_attention = ?2,
                notify_on_mention = ?3,
                updated_at = strftime('%s', 'now')
          WHERE id = 1",
        rusqlite::params![
            prefs.notifications_enabled as i64,
            prefs.notify_on_needs_attention as i64,
            prefs.notify_on_mention as i64,
        ],
    )
    .map_err(|e| format!("update app_settings: {e}"))?;
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
        conn.execute(
            "UPDATE app_settings
                SET notifications_enabled = ?1,
                    notify_on_needs_attention = ?2,
                    notify_on_mention = ?3,
                    updated_at = strftime('%s', 'now')
              WHERE id = 1",
            rusqlite::params![
                prefs.notifications_enabled as i64,
                prefs.notify_on_needs_attention as i64,
                prefs.notify_on_mention as i64,
            ],
        )
        .map_err(|e| format!("update app_settings: {e}"))?;
        AppSettings::load(&conn).map_err(|e| format!("reload app_settings: {e}"))
    }

    #[test]
    fn get_returns_seeded_defaults_on_fresh_db() {
        let db = fresh_db();
        let settings = invoke_get(&db).expect("get app_settings");
        assert!(!settings.notifications_enabled, "master defaults OFF");
        assert!(settings.notify_on_needs_attention);
        assert!(settings.notify_on_mention);
        assert_eq!(
            settings.notification_permission_state,
            NotificationPermissionState::Unprompted
        );
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
        assert!(
            after.updated_at > before.updated_at,
            "updated_at must advance on write"
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
                updated_at: 0,
            },
        )
        .expect("update");
        assert!(after.notifications_enabled);
        assert!(!after.notify_on_needs_attention);
        assert!(!after.notify_on_mention);
    }
}
