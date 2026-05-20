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

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = db::init(app.handle())?;
            app.manage(handle);
            auth::commands::install(&app.handle().clone())
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            auth::commands::add_account,
            auth::commands::list_accounts,
            auth::commands::remove_account,
            auth::commands::validate_token_cmd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
