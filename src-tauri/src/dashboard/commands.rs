//! Tauri command surface for the dashboard.

use tauri::State;

use crate::dashboard::query;
use crate::dashboard::types::{DashboardPullRequest, DashboardSort, DashboardView};
use crate::db::DbHandle;

/// Read the dashboard PR list for the active view.
///
/// `view` selects the relation (Authored / Assigned / Watching) or the
/// repo-opt-in source (Team). `sort` controls ordering; M2 ships `Updated`
/// only. `account_id = None` returns the union across every tracked account.
#[tauri::command]
pub fn list_dashboard_pull_requests(
    view: DashboardView,
    sort: DashboardSort,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<Vec<DashboardPullRequest>, String> {
    let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
    query::list_pull_requests(&conn, view, sort, account_id).map_err(|err| err.to_string())
}
