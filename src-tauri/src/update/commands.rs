//! Tauri command surface for the auto-update subsystem.
//!
//! Three commands ship here:
//!
//! * [`check_for_update_now`] - foreground check. Surfaces both "no
//!   update" and any error to the caller so the Settings panel renders
//!   them inline (ADR-0024: foreground failures are loud, background
//!   failures are silent).
//! * [`install_update_now`] - downloads, verifies, installs, restarts.
//!   The happy path diverges through `app.restart()`; errors flow back
//!   for the panel to surface.
//! * [`install_update_on_quit`] - sets a flag the window-close hook
//!   reads to defer the install until the user next quits the app.
//!
//! The worker emits `update://available` and `update://checked` events
//! when its own loop runs; commands invoked from the panel may or may
//! not emit the same events (foreground readers usually consume the
//! return value directly).

use serde::Serialize;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_updater::UpdaterExt;

use crate::db::DbHandle;
use crate::update::state::{PendingUpdate, UpdateStateHandle};
use crate::update::worker::{format_updater_error, install_now, persist_outcome};

/// Outcome of a foreground check. The Settings panel renders this
/// directly: `update_available` drives the inline callout, `version` and
/// `release_notes` populate it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckForUpdateResult {
    pub update_available: bool,
    pub version: Option<String>,
    pub release_notes: Option<String>,
}

/// Foreground "Check now" handler. Errors are stringified for the
/// renderer; the panel surfaces them inline because the user explicitly
/// asked (ADR-0024's foreground-loud / background-silent split). The
/// outcome is also persisted to `app_settings` so the panel's
/// "last checked" line stays accurate regardless of which path ran the
/// check.
#[tauri::command]
pub async fn check_for_update_now<R: Runtime>(
    app: AppHandle<R>,
    db: State<'_, DbHandle>,
    state: State<'_, UpdateStateHandle>,
) -> Result<CheckForUpdateResult, String> {
    let updater = app.updater().map_err(|e| format!("updater handle: {e}"))?;
    let outcome = updater.check().await;
    match outcome {
        Ok(Some(update)) => {
            let pending = PendingUpdate {
                version: update.version.clone(),
                release_notes: update.body.clone(),
            };
            state.set_pending(Some(pending.clone()));
            persist_outcome(&db, true, None);
            Ok(CheckForUpdateResult {
                update_available: true,
                version: Some(pending.version),
                release_notes: pending.release_notes,
            })
        }
        Ok(None) => {
            state.set_pending(None);
            persist_outcome(&db, true, None);
            Ok(CheckForUpdateResult {
                update_available: false,
                version: None,
                release_notes: None,
            })
        }
        Err(err) => {
            let message = format_updater_error(err);
            persist_outcome(&db, false, Some(message.clone()));
            Err(message)
        }
    }
}

/// Install the pending update right now and restart the app. The happy
/// path diverges through `app.restart()`.
#[tauri::command]
pub async fn install_update_now<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, UpdateStateHandle>,
) -> Result<(), String> {
    install_now(&app, state.inner()).await
}

/// Defer the install to the next quit. The window-close hook in `lib.rs`
/// checks this flag on the main window's `CloseRequested` event.
#[tauri::command]
pub fn install_update_on_quit(state: State<'_, UpdateStateHandle>) -> Result<(), String> {
    state.set_install_on_quit(true);
    Ok(())
}
