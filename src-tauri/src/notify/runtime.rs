//! Production [`NotificationSink`] backed by `tauri-plugin-notification`.
//!
//! Owns three things:
//! * an `AppHandle` so it can address the plugin (the plugin extends every
//!   Tauri runtime handle via `NotificationExt`);
//! * a [`DbHandle`] so it can read the master switch + permission state and
//!   persist the result of a prompt;
//! * a [`PermissionAsker`] so the prompt path is testable without booting a
//!   real Tauri runtime.
//!
//! Permission lifecycle (ADR 0017 decision 5):
//!
//! 1. Read the singleton `app_settings` row.
//! 2. If `notifications_enabled` is OFF (master switch), skip - never touch
//!    permission state. The user hasn't opted in, so there's no reason to
//!    poke the OS prompt.
//! 3. If the master is ON, branch on `notification_permission_state`:
//!    * `Unprompted` -> ask the OS, persist the result, then dispatch iff
//!      granted;
//!    * `Granted` -> dispatch;
//!    * `Denied` -> log and skip (re-prompting after denial would be
//!      browser-grade dark-pattern territory).
//!
//! The trigger -> notification formatting step (issue #192) is the caller's
//! responsibility; the sink only handles dispatch and the OS handshake.

use std::sync::Arc;

use tauri::{AppHandle, Runtime};

use crate::db::DbHandle;
use crate::notify::pending::PendingPayloadQueueHandle;
use crate::notify::sink::{NotificationSink, PermissionAsker};
use crate::notify::types::Notification;
use crate::settings::{AppSettings, NotificationPermissionState};

/// Production [`NotificationSink`] used by `lib.rs`.
pub struct TauriNotificationSink<R: Runtime, A: PermissionAsker> {
    app: AppHandle<R>,
    db: DbHandle,
    asker: Arc<A>,
    pending: PendingPayloadQueueHandle,
}

impl<R: Runtime, A: PermissionAsker> TauriNotificationSink<R, A> {
    pub fn new(
        app: AppHandle<R>,
        db: DbHandle,
        asker: Arc<A>,
        pending: PendingPayloadQueueHandle,
    ) -> Self {
        Self {
            app,
            db,
            asker,
            pending,
        }
    }
}

impl<R: Runtime, A: PermissionAsker> NotificationSink for TauriNotificationSink<R, A> {
    fn dispatch(&self, notification: &Notification) {
        if !decide_dispatch(&self.db, self.asker.as_ref()) {
            return;
        }
        // Enqueue the deep-link payload before the toast fires. The
        // window-event hook in `lib.rs` drains the queue on the next
        // `WindowEvent::Focused(true)` and emits `notification://open-pr`
        // for each entry - the OS-native click-activates-app behaviour
        // surfaces as a focus event on every desktop platform, and the
        // `tauri-plugin-notification` v2.3.3 desktop API doesn't expose a
        // per-notification action callback. See `notify::pending` for the
        // TTL bound on stale-focus false positives.
        self.pending.enqueue(notification.payload.clone());

        use tauri_plugin_notification::NotificationExt;
        if let Err(err) = self
            .app
            .notification()
            .builder()
            .title(notification.title.clone())
            .body(notification.body.clone())
            .show()
        {
            tracing::error!(%err, "notify: plugin show failed");
        }
    }
}

/// Run the master-switch + permission-state decision and return `true` if
/// the caller should proceed with the OS toast. Side effects: persists the
/// answered permission state when the prompt path runs, logs on rusqlite or
/// plugin failures. Lifted out of [`TauriNotificationSink::dispatch`] so the
/// tests can exercise the decision without constructing an `AppHandle`.
pub(crate) fn decide_dispatch(db: &DbHandle, asker: &dyn PermissionAsker) -> bool {
    let settings = match load_settings(db) {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(%err, "notify: load app_settings failed");
            return false;
        }
    };
    if !settings.notifications_enabled {
        // Master switch off. Don't prompt; don't dispatch. The in-app badge
        // keeps doing its job (ADR 0017 decision drivers).
        return false;
    }
    match settings.notification_permission_state {
        NotificationPermissionState::Granted => true,
        NotificationPermissionState::Unprompted => {
            let answered = asker.request();
            if let Err(err) = persist_permission(db, answered) {
                tracing::error!(%err, "notify: persist permission state failed");
            }
            answered == NotificationPermissionState::Granted
        }
        NotificationPermissionState::Denied => {
            tracing::debug!("notify: skipping dispatch, OS permission denied");
            false
        }
    }
}

/// Production [`PermissionAsker`] backed by the plugin. Wraps an `AppHandle`
/// and maps the plugin's `PermissionState` (which has four variants
/// including Android-specific ones) onto our three-state DB shape.
pub struct PluginPermissionAsker<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> PluginPermissionAsker<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> PermissionAsker for PluginPermissionAsker<R> {
    fn current(&self) -> NotificationPermissionState {
        use tauri_plugin_notification::NotificationExt;
        match self.app.notification().permission_state() {
            Ok(state) => map_plugin_state(state),
            Err(err) => {
                tracing::warn!(%err, "notify: plugin permission_state failed");
                NotificationPermissionState::Denied
            }
        }
    }

    fn request(&self) -> NotificationPermissionState {
        use tauri_plugin_notification::NotificationExt;
        match self.app.notification().request_permission() {
            Ok(state) => map_plugin_state(state),
            Err(err) => {
                // Mapping plugin failures to Denied keeps the user in control
                // of a retry: the sink will persist Denied, the Settings
                // panel will show the "blocked" callout (#195), and the next
                // dispatch won't re-ping the OS.
                tracing::warn!(%err, "notify: plugin request_permission failed");
                NotificationPermissionState::Denied
            }
        }
    }
}

fn map_plugin_state(
    state: tauri_plugin_notification::PermissionState,
) -> NotificationPermissionState {
    use tauri_plugin_notification::PermissionState as P;
    match state {
        P::Granted => NotificationPermissionState::Granted,
        P::Denied => NotificationPermissionState::Denied,
        // Prompt / PromptWithRationale both mean "we haven't asked yet";
        // collapse onto our `Unprompted` so the next dispatch path triggers
        // the OS prompt rather than treating the slot as denied.
        _ => NotificationPermissionState::Unprompted,
    }
}

fn load_settings(db: &DbHandle) -> rusqlite::Result<AppSettings> {
    let conn = db.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
    AppSettings::load(&conn)
}

fn persist_permission(db: &DbHandle, state: NotificationPermissionState) -> rusqlite::Result<()> {
    let conn = db.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
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
    )?;
    Ok(())
}
