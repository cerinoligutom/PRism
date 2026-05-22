//! Tauri command surface for the dashboard.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::State;
use thiserror::Error;

use crate::dashboard::query;
use crate::dashboard::types::{
    DashboardPullRequest, DashboardSort, DashboardView, DashboardViewCounts,
};
use crate::db::DbHandle;
use crate::triage::types::ChipKey;

/// User-facing error shape for `dashboard::*` commands. Mirrors the
/// `AuthCommandError` pattern: internal failures (lock poison, rusqlite errors)
/// fold into a single opaque variant so internals never leak to the renderer
/// (CLAUDE.md security rule).
#[derive(Debug, Error, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DashboardCommandError {
    /// The (account, PR) pair carried by a route-metadata lookup doesn't
    /// resolve to a row. Distinct from `Internal` so the caller (currently
    /// `useNotificationRouter`) can drop the route push without surfacing a
    /// generic error - the in-app badge stays the source of truth.
    #[error("pull request not found")]
    NotFound,
    #[error("an unexpected error occurred")]
    Internal,
}

/// Read the dashboard PR list for the active view.
///
/// `view` selects the relation (Authored / Assigned / Watching) or the
/// repo-opt-in source (Tracked). `sort` controls ordering. `account_id = None`
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
) -> Result<Vec<DashboardPullRequest>, DashboardCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    let chips = active_chips.unwrap_or_default();
    query::list_pull_requests(&conn, view, sort, account_id, &chips)
        .map_err(|e| internal(&format!("list_dashboard_pull_requests: {e}")))
}

/// Return the five view row counts for the active account scope in one SQL
/// round-trip (M7 perf, issue #230). Mirrors the predicate shapes of
/// `list_dashboard_pull_requests` so each field equals the length of the
/// matching per-view call.
///
/// Replaces the dashboard store's five-way `Promise.all` fan-out: the sidebar
/// chips stay honest while the SQL planner walks the row scope once per view
/// instead of executing the full projection, hydration, and ordering for each.
#[tauri::command]
pub fn list_dashboard_view_counts(
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<DashboardViewCounts, String> {
    let conn = db.lock().map_err(|_| "db lock poisoned".to_string())?;
    query::list_view_counts(&conn, account_id).map_err(|err| err.to_string())
}

/// Row metadata the frontend needs to push onto the `pr-detail` route after a
/// notification click (ADR 0017 decision 4, issue #201).
///
/// The router's `pr-detail` path takes `:view` and `:id`; the deep-link
/// composable derives `view` from the relation flags (Authored / Assigned /
/// Watching) and falls back to `archive` for archived rows, then `authored`
/// when no flag fits (e.g. a Tracked-view PR with no relation row of its own).
/// The repo coords aren't strictly required by the existing route but are
/// returned so future surfaces (an "Open in GitHub" deep link, a cold-load
/// breadcrumb) don't need a second round-trip.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PrRouteMetadata {
    pub pull_request_id: i64,
    pub number: i64,
    pub owner: String,
    pub name: String,
    /// The dashboard view name (kebab-case) the route should target. Picked
    /// from the relation row's flags in this order: `archive` if the relation
    /// is archived, then `authored`, `assigned`, `watching`. Falls back to
    /// `authored` when no flag is set, which is harmless: the route stays
    /// valid and the detail view's `setView` re-aligns the list on mount.
    pub view: &'static str,
}

/// Resolve route-shaped metadata for the (account, PR) pair carried by a
/// `notification://open-pr` event.
///
/// Errors when the PR id doesn't exist or when no relation row pins the
/// (account, PR) pair - both indicate the caller's payload pre-dates a
/// schema change or a relation prune, and routing to a row the viewer
/// doesn't have a relation with would land on an empty detail view.
#[tauri::command]
pub fn get_pr_route_metadata(
    account_id: i64,
    pull_request_id: i64,
    db: State<'_, DbHandle>,
) -> Result<PrRouteMetadata, DashboardCommandError> {
    let conn = db.lock().map_err(|_| internal("db lock poisoned"))?;
    resolve_pr_route_metadata(&conn, account_id, pull_request_id).map_err(|err| match err {
        rusqlite::Error::QueryReturnedNoRows => DashboardCommandError::NotFound,
        other => internal(&format!("resolve_pr_route_metadata: {other}")),
    })
}

/// SQL row buffer for the route-metadata lookup. Lifting the columns into a
/// named struct keeps the query function clippy-clean (the tuple form trips
/// `clippy::type_complexity`) and the field-by-name reads document the SQL
/// projection order at the use site.
struct RouteMetadataRow {
    number: i64,
    owner: String,
    name: String,
    archived_at: Option<i64>,
    is_authored: i64,
    is_review_requested: i64,
    is_involved: i64,
}

fn resolve_pr_route_metadata(
    conn: &rusqlite::Connection,
    account_id: i64,
    pull_request_id: i64,
) -> rusqlite::Result<PrRouteMetadata> {
    let row = conn
        .query_row(
            "SELECT pr.number,
                    r.owner,
                    r.name,
                    rel.archived_at,
                    COALESCE(rel.is_authored, 0),
                    COALESCE(rel.is_review_requested, 0),
                    COALESCE(rel.is_involved, 0)
               FROM pull_requests pr
               JOIN repos r ON r.id = pr.repo_id
               LEFT JOIN pull_request_viewer_relations rel
                 ON rel.pull_request_id = pr.id
                AND rel.account_id = ?1
              WHERE pr.id = ?2",
            params![account_id, pull_request_id],
            |row| {
                Ok(RouteMetadataRow {
                    number: row.get(0)?,
                    owner: row.get(1)?,
                    name: row.get(2)?,
                    archived_at: row.get(3)?,
                    is_authored: row.get(4)?,
                    is_review_requested: row.get(5)?,
                    is_involved: row.get(6)?,
                })
            },
        )
        .optional()?;
    let row = row.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let view = if row.archived_at.is_some() {
        "archive"
    } else if row.is_authored != 0 {
        "authored"
    } else if row.is_review_requested != 0 {
        "assigned"
    } else if row.is_involved != 0 {
        "watching"
    } else {
        // Tracked-view PR with no relation row, or a stale notification whose
        // relation got pruned. `authored` is a safe landing pad - the detail
        // view's `setView` aligns the list on mount, and the route stays
        // valid for a back-navigation.
        "authored"
    };
    Ok(PrRouteMetadata {
        pull_request_id,
        number: row.number,
        owner: row.owner,
        name: row.name,
        view,
    })
}

fn internal(message: &str) -> DashboardCommandError {
    eprintln!("dashboard command internal error: {message}");
    DashboardCommandError::Internal
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh_db() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn
    }

    fn seed_pr(conn: &Connection, pr_id: i64, number: i64) {
        conn.execute_batch(&format!(
            "INSERT OR IGNORE INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'web', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES ({pr_id}, 10, {number}, 't', 'open', 0, 'bob',
                        0, 0, 'main', 'feat');"
        ))
        .unwrap();
    }

    fn seed_relation(
        conn: &Connection,
        pr_id: i64,
        is_authored: i64,
        is_review_requested: i64,
        is_involved: i64,
        archived_at: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, last_seen_at, archived_at)
                VALUES (1, ?1, ?2, ?3, ?4, 0, ?5)",
            params![
                pr_id,
                is_authored,
                is_review_requested,
                is_involved,
                archived_at
            ],
        )
        .unwrap();
    }

    #[test]
    fn resolves_repo_coords_and_picks_authored_view() {
        let conn = fresh_db();
        seed_pr(&conn, 100, 42);
        seed_relation(&conn, 100, 1, 0, 0, None);

        let meta = resolve_pr_route_metadata(&conn, 1, 100).expect("resolved");

        assert_eq!(meta.pull_request_id, 100);
        assert_eq!(meta.number, 42);
        assert_eq!(meta.owner, "owner");
        assert_eq!(meta.name, "web");
        assert_eq!(meta.view, "authored");
    }

    #[test]
    fn picks_assigned_view_for_review_requested_relation() {
        let conn = fresh_db();
        seed_pr(&conn, 100, 42);
        seed_relation(&conn, 100, 0, 1, 0, None);

        let meta = resolve_pr_route_metadata(&conn, 1, 100).expect("resolved");

        assert_eq!(meta.view, "assigned");
    }

    #[test]
    fn picks_watching_view_for_involved_only_relation() {
        let conn = fresh_db();
        seed_pr(&conn, 100, 42);
        seed_relation(&conn, 100, 0, 0, 1, None);

        let meta = resolve_pr_route_metadata(&conn, 1, 100).expect("resolved");

        assert_eq!(meta.view, "watching");
    }

    #[test]
    fn archived_relation_wins_over_other_flags() {
        let conn = fresh_db();
        seed_pr(&conn, 100, 42);
        // Even a multi-flag relation routes through Archive when it's archived,
        // matching the W2 Archive view's "show archived rows here" rule.
        seed_relation(&conn, 100, 1, 1, 1, Some(123));

        let meta = resolve_pr_route_metadata(&conn, 1, 100).expect("resolved");

        assert_eq!(meta.view, "archive");
    }

    #[test]
    fn falls_back_to_authored_when_no_relation_row_exists() {
        // Tracked-view path: the PR sits in a tracked repo but the viewer
        // doesn't hold a personal relation row for it. The fallback keeps the
        // route valid; the detail view's onMounted `setView` aligns the list
        // so the breadcrumb still reads.
        let conn = fresh_db();
        seed_pr(&conn, 100, 42);

        let meta = resolve_pr_route_metadata(&conn, 1, 100).expect("resolved");

        assert_eq!(meta.view, "authored");
    }

    #[test]
    fn missing_pr_returns_no_rows_error() {
        let conn = fresh_db();

        let err = resolve_pr_route_metadata(&conn, 1, 999).unwrap_err();

        assert!(
            matches!(err, rusqlite::Error::QueryReturnedNoRows),
            "expected QueryReturnedNoRows, got {err:?}"
        );
    }

    #[test]
    fn internal_variant_serialises_without_leaking_inner_message() {
        // CLAUDE.md security rule: internal failure detail must never reach
        // the renderer. The `Internal` variant carries no payload, so the
        // serialised JSON only ever exposes its kind tag.
        let err = internal("rusqlite: table 'pull_requests' has no column named secret");
        let serialised = serde_json::to_string(&err).expect("serialise");
        assert_eq!(serialised, r#"{"kind":"internal"}"#);
        assert!(!serialised.contains("rusqlite"));
        assert!(!serialised.contains("secret"));
    }

    #[test]
    fn not_found_variant_serialises_to_kind_only() {
        let err = DashboardCommandError::NotFound;
        let serialised = serde_json::to_string(&err).expect("serialise");
        assert_eq!(serialised, r#"{"kind":"not_found"}"#);
    }
}
