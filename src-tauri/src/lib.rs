//! PRism backend entry point.
//!
//! Module wiring landed across M1 (issues #8-#14) and M2 (issue #35+):
//! - `db`: SQLite schema + migration runner
//! - `auth`: PAT + OS keychain
//! - `github`: GraphQL + REST clients
//! - `sync`: background worker
//! - `dashboard`: dashboard query surface (M2)
//! - `repos`: repo discovery + Tracked opt-in (M2-D)
//! - `conversation`: per-thread state, conversation stats, lazy hydrator (M3)
//! - `triage`: per-account read-state, mention counters, "needs my attention" (M4)
//! - `settings`: app-wide settings singleton (notification prefs, M6 foundation)
//! - `notify`: OS notification dispatch sink (M6 plumbing; ADR 0017)
//! - `startup`: graceful failure surface for setup-hook + `run()` errors (M7, issue #239)

pub mod app_metadata;
pub mod auth;
pub mod conversation;
pub mod dashboard;
pub mod db;
pub mod github;
pub mod notify;
pub mod repos;
pub mod settings;
pub mod startup;
pub mod sync;
pub mod triage;

use std::sync::Arc;

use tauri::{App, Emitter, Manager, WindowEvent};

const NOTIFICATION_OPEN_PR_EVENT: &str = "notification://open-pr";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .on_window_event(on_window_event)
        .setup(|app| {
            // Run the real setup inside a helper so we can intercept any `?`
            // failure here and route it through `startup::report_failure`
            // (native dialog + on-disk log) before Tauri panics in its event
            // loop. The post-`run()` `Err` branch below only catches runtime
            // failures - setup-hook errors panic from inside the platform
            // event loop before they ever reach the outer `Result`.
            let result = run_setup(app);
            if let Err(ref err) = result {
                startup::report_failure(err.as_ref());
            }
            result
        })
        .invoke_handler(tauri::generate_handler![
            app_metadata::get_app_metadata,
            auth::commands::add_account,
            auth::commands::list_accounts,
            auth::commands::remove_account,
            auth::commands::update_token,
            auth::commands::validate_token_cmd,
            conversation::commands::fetch_pr_conversation,
            conversation::commands::get_pr_conversation_stats,
            conversation::commands::list_pr_threads,
            conversation::commands::list_pr_timeline_events,
            dashboard::commands::get_pr_route_metadata,
            dashboard::commands::list_dashboard_pull_requests,
            dashboard::commands::list_dashboard_view_counts,
            repos::commands::list_repos_for_account,
            repos::commands::refresh_account_repos,
            repos::commands::set_repo_tracked,
            settings::commands::get_app_settings,
            settings::commands::set_notification_permission_state,
            settings::commands::update_app_settings,
            sync::commands::get_sync_status,
            sync::commands::list_recent_activity,
            sync::commands::refresh_now,
            sync::commands::set_sync_interval,
            triage::commands::list_filter_chip_counts,
            triage::commands::list_sidebar_attention_counts,
            triage::commands::mark_pr_archived,
            triage::commands::mark_pr_read,
            triage::commands::mark_pr_unarchived,
            triage::commands::mark_pr_unread,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        // Runtime failures (post-setup) and any builder-side error funnel
        // through this arm. Setup-hook errors are surfaced earlier inside
        // `run_setup` because Tauri panics on them before they propagate
        // here.
        startup::report_failure(&err);
        std::process::exit(1);
    }
}

/// Window-event handler: replays queued notification deep-link payloads on
/// the next main-window focus, since `tauri-plugin-notification` v2.3.3 ships
/// no per-toast click callback (issue #201).
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    // Toast clicks activate the originating app on every desktop OS, which
    // surfaces as `WindowEvent::Focused(true)` on the main window. The
    // notification sink enqueued a payload before each toast fired; drain
    // the queue and replay each as `notification://open-pr` so the
    // frontend's `useNotificationRouter` composable can deep-link. The TTL
    // inside `PendingPayloadQueue::drain_fresh` bounds the false positive
    // when the user focuses the app for an unrelated reason (dock click,
    // alt-tab) past the TTL.
    //
    // The desktop `tauri-plugin-notification` v2.3.3 surface ships no
    // per-notification action callback and no global click hook (issue #201
    // - see `notify::pending` for the constraint narrative); this
    // focus-driven replay is the contract-faithful path that keeps the
    // frontend listener unchanged.
    let WindowEvent::Focused(true) = event else {
        return;
    };
    if window.label() != "main" {
        return;
    }
    let app = window.app_handle();
    let queue = app.state::<notify::PendingPayloadQueueHandle>();
    for payload in queue.drain_fresh() {
        if let Err(err) = app.emit(NOTIFICATION_OPEN_PR_EVENT, &payload) {
            eprintln!("notify: emit {NOTIFICATION_OPEN_PR_EVENT} failed: {err}");
        }
    }
}

/// Setup-hook body, lifted to a free function so the `?` failure paths can be
/// caught by the outer closure and routed through `startup::report_failure`.
fn run_setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let db_handle = db::init(app.handle())?;
    app.manage(db_handle.clone());
    auth::commands::install(&app.handle().clone(), db_handle.clone())
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

    // Build the worker context against the live keychain + DB. Reading
    // back the auth state lets us share the same `TokenSource` the
    // `add_account` command uses, so re-auth doesn't require a restart.
    let auth_state = app.state::<auth::commands::AuthState>();
    let etag_store: Arc<dyn github::EtagStore> =
        Arc::new(db::SqliteEtagStore::new(db_handle.clone()));
    let client_factory: Arc<dyn sync::ClientFactory> = Arc::new(sync::DefaultClientFactory::new(
        auth_state.token_source.clone(),
        etag_store,
    ));
    let account_store = auth_state.store.clone();
    // Hoist the factory + store into Tauri-managed state so the
    // conversation hydrator (`fetch_pr_conversation`) can build a
    // per-account client without going through the worker handle. The
    // worker shares the same Arcs.
    app.manage::<conversation::commands::ClientFactoryHandle>(client_factory.clone());
    app.manage::<conversation::commands::AccountStoreHandle>(account_store.clone());

    // Activity buffer - shared between the worker (writes) and the
    // Tauri command (reads). Issue #122 / `docs/contracts/sync-observability.md`.
    let activity_buffer: sync::ActivityBuffer = sync::new_activity_buffer();
    app.manage(activity_buffer.clone());

    // Notification sink (ADR 0017, issue #192). Production sink wraps
    // `tauri-plugin-notification` via the `PluginPermissionAsker`; the
    // worker + triage commands share the same `Arc`.
    let permission_asker = Arc::new(notify::PluginPermissionAsker::new(app.handle().clone()));
    // Shared pending-payload queue (issue #201): sink enqueues on
    // each dispatched toast, the `on_window_event` hook above drains
    // it on the next main-window focus event and emits
    // `notification://open-pr` for each entry.
    let pending_queue: notify::PendingPayloadQueueHandle = notify::PendingPayloadQueue::new();
    app.manage(pending_queue.clone());
    let notify_sink: notify::NotificationSinkHandle = Arc::new(notify::TauriNotificationSink::new(
        app.handle().clone(),
        db_handle.clone(),
        permission_asker,
        pending_queue.clone(),
    ));
    app.manage(notify_sink.clone());

    // Hydrate the poll interval from `app_settings` so the user's last
    // chosen cadence survives across restarts. Falls back to the default
    // if the column read fails or the persisted value is out of range.
    let scheduler_config = sync::SchedulerConfig::shared();
    if let Some(persisted) = sync::read_persisted_interval(&db_handle) {
        scheduler_config.set_interval(persisted);
    }

    let ctx = sync::WorkerContext {
        db: db_handle.clone(),
        accounts: account_store,
        clients: client_factory,
        config: scheduler_config,
        state: sync::SyncStateMap::new(),
        emit: Arc::new(sync::AppHandleEmitter::new(app.handle().clone())),
        reauth: Arc::new(sync::AppHandleReauth::new(app.handle().clone())),
        badge: Arc::new(notify::AppHandleBadge::new(
            app.handle().clone(),
            db_handle.clone(),
        )),
        activity: activity_buffer,
        notify_sink: notify_sink.clone(),
    };
    let worker = Arc::new(sync::spawn_worker(ctx));
    // Hook the worker into the auth commands so add/remove account
    // hot-mutates the worker pool without an app restart.
    let auth_state = app.state::<auth::commands::AuthState>();
    auth_state.set_listener(worker.clone() as Arc<dyn auth::commands::AccountChangeListener>);
    app.manage(worker);
    Ok(())
}
