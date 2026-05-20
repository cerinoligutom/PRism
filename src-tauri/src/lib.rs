//! PRism backend entry point.
//!
//! Module wiring lands as each M1 issue merges:
//! - `db` (issue #9): SQLite schema + migration runner
//! - `auth` (issue #10): PAT + OS keychain
//! - `github` (issues #11/#12): GraphQL + REST clients
//! - `sync` (issue #13): background worker
//! - `sync::status_timeline` (issue #14): timeline-event-derived status

pub mod auth;
pub mod db;
pub mod github;
pub mod sync;

use std::sync::Arc;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let db_handle = db::init(app.handle())?;
            app.manage(db_handle.clone());
            auth::commands::install(&app.handle().clone())
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

            // Build the worker context against the live keychain + DB. Reading
            // back the auth state lets us share the same `TokenSource` the
            // `add_account` command uses, so re-auth doesn't require a restart.
            let auth_state = app.state::<auth::commands::AuthState>();
            let etag_store: Arc<dyn github::EtagStore> =
                Arc::new(db::SqliteEtagStore::new(db_handle.clone()));
            let client_factory = Arc::new(sync::DefaultClientFactory::new(
                auth_state.token_source.clone(),
                etag_store,
            ));
            let ctx = sync::WorkerContext {
                db: db_handle.clone(),
                accounts: auth_state.store.clone(),
                clients: client_factory,
                config: sync::SchedulerConfig::shared(),
                state: sync::SyncStateMap::new(),
                emit: Arc::new(sync::AppHandleEmitter::new(app.handle().clone())),
                reauth: Arc::new(sync::AppHandleReauth::new(app.handle().clone())),
            };
            let worker = Arc::new(sync::spawn_worker(ctx));
            // Hook the worker into the auth commands so add/remove account
            // hot-mutates the worker pool without an app restart.
            let auth_state = app.state::<auth::commands::AuthState>();
            auth_state.set_listener(
                worker.clone() as Arc<dyn auth::commands::AccountChangeListener>,
            );
            app.manage(worker);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth::commands::add_account,
            auth::commands::list_accounts,
            auth::commands::remove_account,
            auth::commands::validate_token_cmd,
            sync::commands::get_sync_status,
            sync::commands::refresh_now,
            sync::commands::set_sync_interval,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
