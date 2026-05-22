//! Abstract sinks for the notification pipeline.
//!
//! Two traits live here, both very narrow:
//!
//! * [`NotificationSink`] - the one the recompute emitter (issue #192) talks
//!   to. Mirrors `ReauthSink` from `sync/worker.rs:135`: a single fire and
//!   forget method, no return value, no async, no boxed futures. Failures
//!   inside the sink log and continue; the caller never blocks waiting for
//!   the OS to finish toasting.
//!
//! * [`PermissionAsker`] - a tiny abstraction over the bit of the plugin
//!   surface we care about (reading the current OS permission state and
//!   prompting for it). The production [`super::runtime::TauriNotificationSink`]
//!   wires it to `tauri-plugin-notification`; the tests swap a fake in so
//!   the gating + persistence logic can run without booting Tauri.
//!
//! ADR 0017 decision 5 records the deferred-ask permission lifecycle the
//! sink enforces against the `app_settings.notification_permission_state`
//! column.

use std::sync::Arc;

use crate::notify::types::Notification;
use crate::settings::NotificationPermissionState;

/// Single dispatch surface for OS-level notifications. The recompute emitter
/// (issue #192) calls this; the sink owns the master switch + per trigger
/// toggle + permission state gating.
///
/// Mirrors `ReauthSink` in `sync/worker.rs`: dispatch is fire-and-forget, no
/// `Result` - a failed toast is logged and dropped because the in-app badge
/// is the always-on signal (ADR 0017 decision drivers).
pub trait NotificationSink: Send + Sync {
    fn dispatch(&self, notification: &Notification);
}

/// Shared handle to the production [`NotificationSink`]. Mounted via
/// `tauri::Builder::manage` so the triage commands + conversation hydrator can
/// dispatch triggers without going through the sync worker. The sync worker
/// holds the same `Arc` on its [`crate::sync::WorkerContext`].
pub type NotificationSinkHandle = Arc<dyn NotificationSink>;

/// Minimal slice of the `tauri-plugin-notification` permission surface.
///
/// The plugin's `Notification::request_permission` and `permission_state` are
/// hard to mock without a live `AppHandle`; threading this trait between the
/// runtime sink and the plugin lets the tests substitute a fake without
/// touching the production wiring. ADR 0017 favours testable seams over
/// shipping untested permission flow.
///
/// Both methods are infallible from the sink's perspective: any underlying
/// plugin error maps to `Denied` so the user keeps the in-app badge and the
/// next dispatch retries (the plugin error is logged on the way past).
pub trait PermissionAsker: Send + Sync {
    /// Return the OS-reported permission state without prompting.
    fn current(&self) -> NotificationPermissionState;
    /// Prompt the user for permission (if the OS even supports a prompt) and
    /// return the resulting state.
    fn request(&self) -> NotificationPermissionState;
}
