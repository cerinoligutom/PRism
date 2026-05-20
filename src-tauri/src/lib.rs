//! PRism backend entry point.
//!
//! Module wiring landed across M1 (issues #8-#14) and M2 (issue #35+):
//! - `db`: SQLite schema + migration runner
//! - `auth`: PAT + OS keychain
//! - `github`: GraphQL + REST clients
//! - `sync`: background worker
//! - `dashboard`: dashboard query surface (M2)
//! - `repos`: repo discovery + Team-tracked opt-in (M2-D)

pub mod auth;
pub mod dashboard;
pub mod db;
pub mod github;
pub mod repos;
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
            auth::commands::install(&app.handle().clone(), db_handle.clone())
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
            auth_state
                .set_listener(worker.clone() as Arc<dyn auth::commands::AccountChangeListener>);
            app.manage(worker);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth::commands::add_account,
            auth::commands::list_accounts,
            auth::commands::remove_account,
            auth::commands::validate_token_cmd,
            dashboard::commands::list_dashboard_pull_requests,
            repos::commands::list_repos_for_account,
            repos::commands::refresh_account_repos,
            repos::commands::set_repo_team_tracked,
            sync::commands::get_sync_status,
            sync::commands::refresh_now,
            sync::commands::set_sync_interval,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
