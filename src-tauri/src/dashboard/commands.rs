//! Tauri command surface for the dashboard.

use tauri::State;

use crate::dashboard::query;
use crate::dashboard::types::{DashboardPullRequest, DashboardSort, DashboardView};
use crate::db::DbHandle;
use crate::triage::types::ChipKey;

/// Read the dashboard PR list for the active view.
///
/// `view` selects the relation (Authored / Assigned / Watching) or the
/// repo-opt-in source (Team). `sort` controls ordering. `account_id = None`
/// returns the union across every tracked account. `active_chips = None`
/// (or an empty `Some` vector) skips the chip filter; otherwise the chips
/// AND-compose into the WHERE per `docs/contracts/triage-ux.md`
/// ("Filter chip semantics").
#[tauri::command]
pub fn list_dashboard_pull_requests(
    view: DashboardView,
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: Option<Vec<ChipKey>>,
    db: State<'_, DbHandle>,
) -> Result<Vec<DashboardPullRequest>, String> {
    let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
    let chips = active_chips.unwrap_or_default();
    query::list_pull_requests(&conn, view, sort, account_id, &chips).map_err(|err| err.to_string())
}
