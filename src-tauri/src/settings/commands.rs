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
//! `notification_permission_state` and `last_seen_version` are intentionally
//! _not_ rewritten by `update_app_settings` - both columns are owned by
//! non-Settings gestures, and letting the panel echo stale values back would
//! either re-prompt the OS or silently re-suppress the dialog.
//!
//! Errors are stringified for the renderer (matches the existing settings
//! style; the type itself is internal so leaking the rusqlite message is
//! fine).

use tauri::State;

use crate::db::DbHandle;
use crate::settings::types::{AppSettings, NotificationPermissionState};

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
}
