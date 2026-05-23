//! Background updater worker (ADR-0024).
//!
//! One tokio task drives the whole loop. Cadence comes from
//! `app_settings.auto_update_interval_seconds` (default 21600s / 6h); the
//! enabled flag from `auto_update_enabled`. The worker reads both once per
//! tick so a settings flip picks up without a restart.
//!
//! Every check (success or failure) is recorded through the
//! `record_update_check` Tauri command path - we drive the SQL directly to
//! avoid a circular call through the command surface from inside the
//! plugin runtime. On success with an update available, the worker emits a
//! `update://available` Tauri event so the renderer can surface the
//! in-app banner; on failure the persisted column drives the Settings
//! panel.
//!
//! The cancellation token lets the setup hook tear the worker down on
//! shutdown without leaving a dangling tokio task.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::Notify;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

use crate::db::{lock_db, DbHandle};
use crate::update::state::{PendingUpdate, UpdateStateHandle};

/// Default cadence per ADR-0024. The interval is hydrated from
/// `app_settings.auto_update_interval_seconds` on startup; this constant
/// is the fallback when the column read fails (corrupted DB, migration
/// mid-flight) and the floor when a stored value reads as zero or
/// negative.
pub const DEFAULT_INTERVAL_SECS: u64 = 21_600;

/// Floor applied to any persisted interval. A user who somehow writes a
/// rapid cadence (one-second poll) shouldn't be able to DDoS the GH Pages
/// manifest endpoint from PRism. Five minutes is the smallest interval
/// the v1.x channel will honour.
pub const MIN_INTERVAL_SECS: u64 = 300;

/// First check fires after a short warmup so launch isn't competing for
/// the network with the initial sync cycle (ADR-0024). The interval check
/// then takes over.
pub const WARMUP_SECS: u64 = 60;

/// Tauri event emitted when a check finds an update.
pub const UPDATE_AVAILABLE_EVENT: &str = "update://available";

/// Tauri event emitted when a check completes without finding an update.
/// The Settings panel uses this to update its "last checked" line on the
/// next render without a poll.
pub const UPDATE_CHECK_EVENT: &str = "update://checked";

#[derive(Debug, Clone, Serialize)]
pub struct UpdateAvailablePayload {
    pub version: String,
    pub release_notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheckPayload {
    pub success: bool,
    pub failure_message: Option<String>,
}

/// Handle to the running worker. Holds the cancellation token + a
/// `Notify` the manual "Check now" command uses to short-circuit the
/// sleep on the current cycle.
pub struct UpdateWorker {
    cancel: CancellationToken,
    nudge: Arc<Notify>,
    _task: JoinHandle<()>,
}

pub type UpdateWorkerHandle = Arc<UpdateWorker>;

impl UpdateWorker {
    /// Nudge the worker to run an immediate check. Used by the manual
    /// `check_for_update_now` command.
    pub fn nudge(&self) {
        self.nudge.notify_one();
    }

    /// Cancel the loop. Idempotent.
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }
}

/// Spawn the updater worker. Returns the handle the setup hook stashes in
/// Tauri-managed state.
pub fn spawn_worker<R: Runtime>(
    app: AppHandle<R>,
    db: DbHandle,
    state: UpdateStateHandle,
) -> UpdateWorkerHandle {
    let cancel = CancellationToken::new();
    let nudge = Arc::new(Notify::new());

    let task =
        tauri::async_runtime::spawn(loop_body(app, db, state, cancel.clone(), nudge.clone()));

    Arc::new(UpdateWorker {
        cancel,
        nudge,
        _task: task,
    })
}

async fn loop_body<R: Runtime>(
    app: AppHandle<R>,
    db: DbHandle,
    state: UpdateStateHandle,
    cancel: CancellationToken,
    nudge: Arc<Notify>,
) {
    if wait_or_cancel(&cancel, &nudge, Duration::from_secs(WARMUP_SECS)).await {
        return;
    }

    loop {
        if cancel.is_cancelled() {
            return;
        }

        // Re-read both fields on every tick so toggling the setting
        // without restarting the app applies on the next iteration.
        let (enabled, interval) = read_settings_snapshot(&db);

        if enabled {
            run_check(&app, &db, &state).await;
        }

        let wait = Duration::from_secs(interval.max(MIN_INTERVAL_SECS));
        if wait_or_cancel(&cancel, &nudge, wait).await {
            return;
        }
    }
}

/// Returns `true` if the wait was cancelled (caller should bail). A
/// nudge or the timeout both fall through to the next loop iteration,
/// which re-reads the enabled flag before running another check.
async fn wait_or_cancel(
    cancel: &CancellationToken,
    nudge: &Arc<Notify>,
    duration: Duration,
) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => true,
        _ = nudge.notified() => false,
        _ = sleep(duration) => false,
    }
}

/// Read `auto_update_enabled` + `auto_update_interval_seconds` from the
/// singleton row. Returns sensible defaults on any error so the loop
/// stays alive across a transient DB hiccup.
fn read_settings_snapshot(db: &DbHandle) -> (bool, u64) {
    let conn = match lock_db(db) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("update: settings snapshot - db lock failed: {err}");
            return (false, DEFAULT_INTERVAL_SECS);
        }
    };
    match conn.query_row(
        "SELECT auto_update_enabled, auto_update_interval_seconds
           FROM app_settings WHERE id = 1",
        [],
        |row| {
            let enabled: i64 = row.get(0)?;
            let interval: i64 = row.get(1)?;
            Ok((enabled != 0, interval))
        },
    ) {
        Ok((enabled, interval)) => {
            let secs = if interval > 0 {
                interval as u64
            } else {
                DEFAULT_INTERVAL_SECS
            };
            (enabled, secs.max(MIN_INTERVAL_SECS))
        }
        Err(err) => {
            eprintln!("update: settings snapshot - query failed: {err}");
            (false, DEFAULT_INTERVAL_SECS)
        }
    }
}

/// Run a single check + emit. Records the outcome to `app_settings`
/// either way. ADR-0024's silent-failure rule applies to background
/// failures; foreground (`check_for_update_now`) callers read the
/// returned message off the command return value.
pub async fn run_check<R: Runtime>(app: &AppHandle<R>, db: &DbHandle, state: &UpdateStateHandle) {
    let outcome = check_once(app).await;
    match outcome {
        Ok(Some(update)) => {
            let pending = PendingUpdate {
                version: update.version.clone(),
                release_notes: update.release_notes.clone(),
            };
            state.set_pending(Some(pending.clone()));
            let payload = UpdateAvailablePayload {
                version: pending.version,
                release_notes: pending.release_notes,
            };
            if let Err(err) = app.emit(UPDATE_AVAILABLE_EVENT, &payload) {
                eprintln!("update: emit available failed: {err}");
            }
            persist_outcome(db, true, None);
            emit_check_event(app, true, None);
        }
        Ok(None) => {
            state.set_pending(None);
            persist_outcome(db, true, None);
            emit_check_event(app, true, None);
        }
        Err(message) => {
            // Silent background failure (ADR-0024). The Settings panel
            // reads the persisted column to render the "Last check
            // failed" line; no toast, no banner.
            persist_outcome(db, false, Some(message.clone()));
            emit_check_event(app, false, Some(message));
        }
    }
}

/// Outcome of a single check. `Some(...)` means an update is available;
/// `None` means we're current; `Err(...)` is any plugin or transport
/// failure.
pub struct UpdateRecord {
    pub version: String,
    pub release_notes: Option<String>,
}

async fn check_once<R: Runtime>(app: &AppHandle<R>) -> Result<Option<UpdateRecord>, String> {
    let updater = app.updater().map_err(|e| format!("updater handle: {e}"))?;
    let outcome = updater.check().await.map_err(format_updater_error)?;
    Ok(outcome.map(|u| UpdateRecord {
        version: u.version.clone(),
        release_notes: u.body.clone(),
    }))
}

pub(crate) fn persist_outcome(db: &DbHandle, success: bool, failure_message: Option<String>) {
    let conn = match lock_db(db) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("update: persist outcome - db lock failed: {err}");
            return;
        }
    };
    let truncated = failure_message.map(|s| truncate_failure(&s));
    let stored: Option<&str> = if success { None } else { truncated.as_deref() };
    if let Err(err) = conn.execute(
        "UPDATE app_settings
            SET auto_update_last_check_at = strftime('%s', 'now'),
                auto_update_last_failure_message = ?1,
                updated_at = strftime('%s', 'now')
          WHERE id = 1",
        rusqlite::params![stored],
    ) {
        eprintln!("update: persist outcome - write failed: {err}");
    }
}

fn emit_check_event<R: Runtime>(
    app: &AppHandle<R>,
    success: bool,
    failure_message: Option<String>,
) {
    let payload = UpdateCheckPayload {
        success,
        failure_message,
    };
    if let Err(err) = app.emit(UPDATE_CHECK_EVENT, &payload) {
        eprintln!("update: emit check failed: {err}");
    }
}

fn truncate_failure(raw: &str) -> String {
    const MAX: usize = 240;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}...", &raw[..MAX])
    }
}

/// Stringify an updater error into a short, end-user-presentable message.
/// The Settings panel renders these on a single line; the underlying
/// debug string can be verbose, so we strip stack-trace style detail.
pub fn format_updater_error(err: tauri_plugin_updater::Error) -> String {
    let raw = err.to_string();
    truncate_failure(&raw)
}

/// Install the pending update right away, then relaunch. `app.restart()`
/// terminates the process, so this function diverges on the happy path.
/// Errors propagate to the caller (the Settings panel's "Install now"
/// button surfaces them inline).
pub async fn install_now<R: Runtime>(
    app: &AppHandle<R>,
    state: &UpdateStateHandle,
) -> Result<(), String> {
    let updater = app.updater().map_err(|e| format!("updater handle: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(format_updater_error)?
        .ok_or_else(|| "no update is available".to_string())?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(format_updater_error)?;
    state.set_pending(None);
    state.set_install_on_quit(false);
    app.restart();
    // `app.restart()` returns `!`; the line above never falls through.
}

/// Download + install the pending update without relaunching. Used by
/// the install-on-quit close-request hook: the user is already closing,
/// so the next launch is what picks up the new binary.
pub async fn install_quietly<R: Runtime>(
    app: &AppHandle<R>,
    state: &UpdateStateHandle,
) -> Result<(), String> {
    let updater = app.updater().map_err(|e| format!("updater handle: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(format_updater_error)?
        .ok_or_else(|| "no update is available".to_string())?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(format_updater_error)?;
    state.set_pending(None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex};

    fn fresh_db() -> DbHandle {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        Arc::new(Mutex::new(conn))
    }

    #[test]
    fn settings_snapshot_reads_defaults() {
        let db = fresh_db();
        let (enabled, interval) = read_settings_snapshot(&db);
        assert!(!enabled, "fresh install defaults OFF (ADR-0024)");
        assert_eq!(interval, DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn settings_snapshot_clamps_interval_to_floor() {
        let db = fresh_db();
        let conn = db.lock().expect("db lock");
        conn.execute(
            "UPDATE app_settings
                SET auto_update_enabled = 1,
                    auto_update_interval_seconds = 30
              WHERE id = 1",
            [],
        )
        .expect("seed");
        drop(conn);
        let (enabled, interval) = read_settings_snapshot(&db);
        assert!(enabled);
        assert_eq!(
            interval, MIN_INTERVAL_SECS,
            "rapid cadence floors to MIN_INTERVAL_SECS"
        );
    }

    #[test]
    fn settings_snapshot_zero_interval_falls_back_to_default() {
        let db = fresh_db();
        let conn = db.lock().expect("db lock");
        conn.execute(
            "UPDATE app_settings SET auto_update_interval_seconds = 0 WHERE id = 1",
            [],
        )
        .expect("seed");
        drop(conn);
        let (_, interval) = read_settings_snapshot(&db);
        assert_eq!(interval, DEFAULT_INTERVAL_SECS);
    }

    #[test]
    fn persist_outcome_success_clears_failure() {
        let db = fresh_db();
        persist_outcome(&db, false, Some("network down".into()));
        persist_outcome(&db, true, None);
        let conn = db.lock().expect("db lock");
        let (ts, msg): (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT auto_update_last_check_at, auto_update_last_failure_message
                   FROM app_settings WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(ts.is_some());
        assert_eq!(msg, None);
    }

    #[test]
    fn persist_outcome_failure_truncates_long_message() {
        let db = fresh_db();
        let long = "x".repeat(500);
        persist_outcome(&db, false, Some(long));
        let conn = db.lock().expect("db lock");
        let msg: Option<String> = conn
            .query_row(
                "SELECT auto_update_last_failure_message FROM app_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let stored = msg.expect("failure message stored");
        assert!(
            stored.ends_with("..."),
            "long messages must be truncated with an ellipsis"
        );
        assert!(stored.len() <= 245, "truncate cap must hold");
    }
}
