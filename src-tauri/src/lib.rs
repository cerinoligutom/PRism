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
//! - `notifications`: persistent inbox mirroring dispatched toasts (issue #378)
//! - `startup`: graceful failure surface for setup-hook + `run()` errors (M7, issue #239)
//! - `update`: auto-update subsystem (ADR-0024, issue #308)
//!
//! The `prism://` custom URL scheme (issue #339) is wired through the
//! `tauri-plugin-deep-link` plugin registered below; the bundle config in
//! `tauri.conf.json > plugins.deep-link.desktop.schemes` emits the macOS
//! `CFBundleURLTypes`, Linux `.desktop` MimeType, and Windows registry
//! entries during bundling. Frontend handler in `src/composables/useDeepLinkRouter.ts`.

pub mod app_metadata;
pub mod auth;
pub mod conversation;
pub mod dashboard;
pub mod db;
pub mod github;
pub mod notifications;
pub mod notify;
pub mod repos;
pub mod settings;
pub mod startup;
pub mod sync;
pub mod triage;
pub mod update;

use std::sync::Arc;

use tauri::{App, Emitter, Manager, WindowEvent};

const NOTIFICATION_OPEN_PR_EVENT: &str = "notification://open-pr";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();

    // Linux + Windows route inbound `prism://` URLs by spawning a new app
    // instance with the URL as a CLI argument. The single-instance plugin
    // (with the `deep-link` feature) intercepts that spawn, forwards the
    // URL into the running app's deep-link channel, and quits the duplicate
    // process so the existing window handles the URL (issue #339). macOS
    // doesn't need this - the OS routes the URL into the live process
    // directly via `onOpenUrl`.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }));
    }

    let result = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_deep_link::init())
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
            dashboard::commands::pr_lookup_by_coordinates,
            notifications::commands::clear_all_notifications,
            notifications::commands::delete_notification,
            notifications::commands::list_notifications,
            notifications::commands::mark_all_notifications_read,
            notifications::commands::mark_notification_read,
            notifications::commands::unread_notification_count,
            repos::commands::list_repos_for_account,
            repos::commands::refresh_account_repos,
            repos::commands::set_repo_tracked,
            settings::commands::get_app_settings,
            settings::commands::record_update_check,
            settings::commands::set_last_seen_version,
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
            triage::commands::mark_prs_archived,
            triage::commands::mark_view_read,
            update::commands::check_for_update_now,
            update::commands::install_update_now,
            update::commands::install_update_on_quit,
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

/// Window-event handler. Two responsibilities:
///
/// * On `Focused(true)` for the main window, replay queued notification
///   deep-link payloads since `tauri-plugin-notification` v2.3.3 ships no
///   per-toast click callback (issue #201). Toast clicks activate the
///   originating app on every desktop OS, which surfaces as a focus
///   event on the main window. The TTL inside
///   `PendingPayloadQueue::drain_fresh` bounds the false positive when
///   the user focuses the app for an unrelated reason (dock click,
///   alt-tab) past the TTL.
/// * On `CloseRequested` for the main window, if the updater state has
///   `install_on_quit == true`, spawn a background download + install
///   task that runs the standard plugin install path (ADR-0024, issue
///   #308). The close proceeds normally; the install completes
///   opportunistically and the next launch picks up the new binary.
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    let app = window.app_handle();
    match event {
        WindowEvent::Focused(true) => {
            let queue = app.state::<notify::PendingPayloadQueueHandle>();
            for payload in queue.drain_fresh() {
                if let Err(err) = app.emit(NOTIFICATION_OPEN_PR_EVENT, &payload) {
                    tracing::warn!(event = NOTIFICATION_OPEN_PR_EVENT, %err, "notify: emit failed");
                }
            }
        }
        WindowEvent::CloseRequested { .. } => {
            let state = app.state::<update::UpdateStateHandle>();
            if !state.install_on_quit() {
                return;
            }
            // Drop the flag right away so a second close event (multiple
            // windows, programmatic close) doesn't kick off a second
            // install task. The close itself is not blocked - the
            // install runs concurrently and lands on the filesystem
            // before the process exits; the next launch picks up the
            // new binary.
            state.set_install_on_quit(false);
            let app_handle = app.clone();
            let state_handle: update::UpdateStateHandle = state.inner().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = update::worker::install_quietly(&app_handle, &state_handle).await
                {
                    tracing::error!(%err, "update: install-on-quit failed");
                }
            });
        }
        _ => {}
    }
}

/// Setup-hook body, lifted to a free function so the `?` failure paths can be
/// caught by the outer closure and routed through `startup::report_failure`.
fn run_setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
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

    // Register the `prism://` scheme with the OS on Linux + Windows dev
    // launches (issue #339). Bundling emits the `.desktop` MimeType / registry
    // entries from `tauri.conf.json`, but a `cargo tauri dev` run skips those
    // paths; runtime `register("prism")` fills that gap. macOS / iOS / Android
    // ignore this call per the plugin's documentation.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        if let Err(err) = app.deep_link().register("prism") {
            tracing::warn!(%err, "deep-link: runtime register failed");
        }
    }

    // Auto-update subsystem (ADR-0024, issue #308). The state handle
    // holds the pending-update slot the Settings panel + banner read,
    // plus the install-on-quit flag the close-request hook consults.
    // The worker spins regardless of the toggle so a flip from OFF to
    // ON picks up on the next tick without needing a restart; the
    // enabled check happens inside the loop.
    let update_state = update::state::UpdateState::new();
    app.manage::<update::state::UpdateStateHandle>(update_state.clone());
    let update_worker = update::spawn_worker(app.handle().clone(), db_handle, update_state);
    app.manage::<update::UpdateWorkerHandle>(update_worker);
    Ok(())
}

/// Initialise the global `tracing` subscriber for stdout. ADR 0026: stdout
/// only, no rolling file. The filter reads `RUST_LOG` when present and falls
/// back to `warn` so a default run stays quiet. `try_init` swallows the
/// "already set" error so this is safe to call from re-entered setup hooks
/// (the Tauri builder runs setup once per `run()` call, but cargo test loads
/// the lib crate multiple times).
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt().with_env_filter(filter).try_init();
}
