//! Tauri command surface for the dashboard.

use tauri::State;

use crate::dashboard::types::{DashboardPullRequest, DashboardSort, DashboardView};
use crate::db::DbHandle;

/// Read the dashboard PR list for the active view.
///
/// `view` selects the relation (Authored / Assigned / Watching) or the
/// repo-opt-in source (Team). `sort` controls ordering; M2 ships `Updated`
/// only. `account_id = None` returns the union across every tracked account.
///
/// Wave 2-C implements the body. The command is registered now so the
/// TypeScript bindings see it during M2 frontend work.
#[tauri::command]
pub fn list_dashboard_pull_requests(
    view: DashboardView,
    sort: DashboardSort,
    account_id: Option<i64>,
    _db: State<'_, DbHandle>,
) -> Result<Vec<DashboardPullRequest>, String> {
    let _ = (view, sort, account_id);
    Err("list_dashboard_pull_requests not implemented (M2-C)".into())
}
