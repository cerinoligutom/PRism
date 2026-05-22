//! Read-only SQL composition for `list_dashboard_pull_requests`.
//!
//! See `docs/contracts/dashboard-data.md` for the DTO contract and the
//! per-view shape this module implements. Each view shares the same outer
//! projection and reviewer hydration; they differ only in the FROM clause
//! that selects the PR set:
//!
//! - Authored / Assigned / Watching read from `pull_request_viewer_relations`
//!   gated by the matching flag column (each flag has a partial index).
//! - Team reads `repos.is_team_tracked = 1` directly; the relations table is
//!   not touched because the Team relationship is a repo property.
//!
//! `account_id = None` returns the dedupe-and-merge union across every
//! account (ADR 0016). The relation-backed views GROUP BY `pr.id` so a PR
//! authored by account A and review-requested for account B surfaces as one
//! row whose triage signals are merged (`unread = MAX`,
//! `needs_attention = MAX`, `mentioned_count_unread = SUM`) and whose
//! `account_ids` carries every relation owner. The Team view's union path
//! keeps the same GROUP BY but joins relations without an account scope so
//! every relation row a PR has feeds the aggregations; PRs in team-tracked
//! repos with no relation rows still surface (the Team filter is the repo's
//! `is_team_tracked = 1`, not the relations table) with an empty
//! `account_ids` and the default triage values.
//!
//! ## Threads rollup
//!
//! ADR 0016 retires the pre-aggregated `pull_requests.threads_*` columns in
//! favour of query-time computation. The four buckets are derived inside a
//! `thread_buckets` LEFT JOIN that GROUPs `review_threads` by PR; the
//! involvement test scopes against the in-scope account set so a multi-account
//! union no longer flickers with whichever account synced last. The legacy
//! columns stay on the schema (SQLite column-drop is non-trivial) and are no
//! longer written or read.
//!
//! ## Failure isolation in the union path
//!
//! The relation join in the union path is a LEFT JOIN with no account
//! predicate. A failing account whose relation rows got pruned mid-cycle does
//! not drop PRs another account also sees. The merge aggregates over zero or
//! more relation rows per PR; an empty `account_ids` slot is the visible
//! signal that a PR surfaced via the Team-view path (or that every relation
//! row for the PR was pruned in the most recent cycle).

use std::collections::HashMap;

use rusqlite::{params_from_iter, Connection, Row};

use crate::dashboard::types::{
    CiSummary, DashboardPullRequest, DashboardSort, DashboardView, RepoRef, ReviewerEntry,
    ReviewerState, ThreadsSummary,
};
use crate::triage::query as triage_query;
use crate::triage::types::ChipKey;

/// SQL fragment that selects every column the row projection needs in the
/// single-account-scoped path. Joined to `repos` and `accounts`; each view
/// prepends its own FROM clause to this body.
///
/// The trailing `rel.*` projections (M4-C) read the active account's triage
/// state. Relation-backed views (Authored / Assigned / Watching) already JOIN
/// `pull_request_viewer_relations rel` so the columns flow directly. The Team
/// view adds a LEFT JOIN on the same alias - scoped to the active account when
/// one is provided, an inert `ON 0` join otherwise - so the SELECT keeps a
/// stable shape across every view. See `docs/contracts/triage-ux.md`
/// ("Read-state derivation") and ADR 0015.
///
/// The `tb.*` projections come from the `thread_buckets` subquery that every
/// view's FROM clause LEFT JOINs (ADR 0016). `COALESCE(tb.total, 0)` keeps the
/// muted em-dash state working: a PR with no `review_threads` rows misses the
/// join and reads as zero; `project_pr_row` then emits `threads = None`.
const PR_PROJECTION_COLUMNS: &str = "
    pr.id,
    pr.number,
    pr.title,
    pr.state,
    pr.draft,
    pr.mergeable,
    pr.review_decision,
    pr.author_login,
    author_u.avatar_url AS author_avatar_url,
    pr.base_ref,
    pr.head_ref,
    pr.created_at,
    pr.updated_at,
    pr.latest_status_change_at,
    pr.additions,
    pr.deletions,
    pr.changed_files,
    pr.ci_state,
    pr.ci_total,
    pr.ci_passing,
    COALESCE(tb.total, 0) AS threads_total,
    COALESCE(tb.unresolved_involved, 0) AS threads_unresolved_involved,
    COALESCE(tb.unresolved_uninvolved, 0) AS threads_unresolved_uninvolved,
    COALESCE(tb.resolved_involved, 0) AS threads_resolved_involved,
    COALESCE(tb.resolved_uninvolved, 0) AS threads_resolved_uninvolved,
    r.id,
    r.owner,
    r.name,
    CAST(a.id AS TEXT) AS account_ids,
    a.host,
    CASE
        WHEN rel.read_at IS NULL THEN 1
        WHEN pr.updated_at > COALESCE(rel.read_pr_updated_at, 0) THEN 1
        ELSE 0
    END AS unread,
    COALESCE(rel.needs_attention, 0) AS needs_attention,
    COALESCE(rel.mentioned_count_unread, 0) AS mentioned_count_unread
";

/// Unified-mode projection. Adds the per-relation merge aggregations
/// (`MAX(unread)`, `MAX(needs_attention)`, `SUM(mentioned_count_unread)`) and
/// the comma-separated `account_ids` marker. `host` is read from the repo's
/// owning account (`acc_repo`) because the repo, not the relation, anchors
/// the PR to exactly one host; the URL builder needs the right host for
/// `https://{host}/...` regardless of which accounts have relations. The
/// projection only touches columns the GROUP BY tolerates: every non-aggregated
/// reference is `pr.*`, `r.*`, `acc_repo.host`, or a `tb.*` column from a
/// subquery that already GROUPs by `pull_request_id`.
const PR_PROJECTION_COLUMNS_UNION: &str = "
    pr.id,
    pr.number,
    pr.title,
    pr.state,
    pr.draft,
    pr.mergeable,
    pr.review_decision,
    pr.author_login,
    author_u.avatar_url AS author_avatar_url,
    pr.base_ref,
    pr.head_ref,
    pr.created_at,
    pr.updated_at,
    pr.latest_status_change_at,
    pr.additions,
    pr.deletions,
    pr.changed_files,
    pr.ci_state,
    pr.ci_total,
    pr.ci_passing,
    COALESCE(tb.total, 0) AS threads_total,
    COALESCE(tb.unresolved_involved, 0) AS threads_unresolved_involved,
    COALESCE(tb.unresolved_uninvolved, 0) AS threads_unresolved_uninvolved,
    COALESCE(tb.resolved_involved, 0) AS threads_resolved_involved,
    COALESCE(tb.resolved_uninvolved, 0) AS threads_resolved_uninvolved,
    r.id,
    r.owner,
    r.name,
    COALESCE(GROUP_CONCAT(DISTINCT rel.account_id ORDER BY rel.account_id), '')
        AS account_ids,
    acc_repo.host,
    MAX(CASE
            WHEN rel.read_at IS NULL THEN 1
            WHEN pr.updated_at > COALESCE(rel.read_pr_updated_at, 0) THEN 1
            ELSE 0
        END) AS unread,
    MAX(COALESCE(rel.needs_attention, 0)) AS needs_attention,
    SUM(COALESCE(rel.mentioned_count_unread, 0)) AS mentioned_count_unread
";

/// Whether the query runs the single-account-scoped projection or the
/// unified-mode dedupe-and-merge path. Single-account keeps the SQL
/// byte-identical to before this ticket; union mode adds `GROUP BY pr.id`
/// and the triage merge aggregations.
#[derive(Clone, Copy)]
enum QueryShape {
    SingleAccount,
    Union,
}

/// Common projection: PR + repo + account, ordered by the requested sort.
/// `from_and_where` substitutes in the view-specific JOIN and WHERE clauses.
/// `chip_clause` is the optional `AND (chip_1) AND (chip_2) ...` fragment
/// that pins the chip-filter composition into the WHERE; empty when no chips
/// are active. Parameter order is determined by the `from_and_where` body;
/// the caller passes the matching parameters when invoking the prepared
/// statement.
///
/// `NeedsMe` references `rel.needs_attention`; the caller must ensure `rel`
/// is in scope (either via the relation-view JOIN or via a LEFT JOIN against
/// `pull_request_viewer_relations` in the Team view path).
///
/// `QueryShape::Union` swaps the projection for the merged-aggregation one
/// and appends `GROUP BY pr.id` before the chip clause. The chip predicates
/// run before the GROUP BY so the merge only sees rows the chips already
/// admitted, matching the single-account behaviour where the chip filters
/// rows directly.
///
/// `order_override` lets a caller substitute a view-specific ORDER BY. The
/// Archive view (ADR 0018) uses this to default to `archived_at DESC` when
/// the caller didn't pick a non-default sort; passing `None` keeps the
/// sort-derived ORDER BY (the behaviour every default view relies on).
fn build_sql(
    from_and_where: &str,
    chip_clause: &str,
    sort: DashboardSort,
    shape: QueryShape,
    order_override: Option<&str>,
) -> String {
    let order_by = match sort {
        DashboardSort::Updated => {
            "ORDER BY COALESCE(pr.latest_status_change_at, pr.updated_at) DESC, pr.id DESC"
        }
        DashboardSort::Stale => "ORDER BY pr.updated_at ASC, pr.id DESC",
        DashboardSort::NeedsMe => {
            // `COALESCE` keeps the column NULL-safe for the Team view path,
            // where the LEFT JOIN against `pull_request_viewer_relations`
            // misses when the active account has no relation row. The union
            // path's MAX over the relation rows is non-NULL when any row
            // matched and `COALESCE(NULL, 0) = 0` when none did, so the same
            // expression works for both shapes.
            "ORDER BY COALESCE(MAX(rel.needs_attention), 0) DESC, \
                      COALESCE(pr.latest_status_change_at, pr.updated_at) DESC, \
                      pr.id DESC"
        }
    };
    match shape {
        QueryShape::SingleAccount => {
            // The single-account ORDER BY references `rel.needs_attention`
            // directly (no aggregation). The constant string above wraps it
            // in `MAX(...)` for symmetry with the union path; the planner
            // still drives off the relation row in the absence of a GROUP BY
            // and the result is byte-identical.
            let order_by_single = match sort {
                DashboardSort::NeedsMe => {
                    "ORDER BY COALESCE(rel.needs_attention, 0) DESC, \
                              COALESCE(pr.latest_status_change_at, pr.updated_at) DESC, \
                              pr.id DESC"
                }
                _ => order_by,
            };
            let effective_order = order_override.unwrap_or(order_by_single);
            format!(
                "SELECT {PR_PROJECTION_COLUMNS}
                 {from_and_where}
                 {chip_clause}
                 {effective_order}"
            )
        }
        QueryShape::Union => {
            let effective_order = order_override.unwrap_or(order_by);
            format!(
                "SELECT {PR_PROJECTION_COLUMNS_UNION}
                 {from_and_where}
                 {chip_clause}
                 GROUP BY pr.id
                 {effective_order}"
            )
        }
    }
}

/// Bucket projection (ADR 0016) that LEFT JOINs the outer PR row. Computes
/// the four `(resolved x involved)` counts plus `total` from `review_threads`
/// / `review_comments`. The involvement EXISTS scopes against the in-scope
/// account set: `a.id = ?1` when one account is active, `a.id IN (SELECT id
/// FROM accounts)` when the view is unioned across every tracked account.
/// The single-account variant reuses `?1` with the per-view WHERE clauses so
/// the bound vector stays length-1.
///
/// The subquery GROUPs `review_threads` by `pull_request_id` and the outer
/// `LEFT JOIN ... ON tb.pull_request_id = pr.id` ties it to the row. A PR
/// with no threads misses the join entirely; the projection's `COALESCE(...,
/// 0)` then defaults to zero so `project_pr_row` emits `threads = None`.
///
/// Host disambiguation is not applied here. The single-account path filters
/// by `a.id = ?1` so the EXISTS only admits the active account's row even if
/// another account shares its login on a different host. The union path
/// admits every tracked account; a login collision among the user's own
/// identities means at least one of them genuinely authored the comment, so
/// the involvement signal stays correct. Contrast with
/// `sync::worker::scan_mentions_and_recompute_attention`, which keys against
/// `pr.author_login` (a recorded login on the PR's host) and therefore needs
/// the viewer-host = PR-host guard to avoid cross-host false positives.
fn thread_buckets_subquery(in_scope_predicate: &str) -> String {
    format!(
        "LEFT JOIN (
            SELECT t.pull_request_id,
                   COUNT(*) AS total,
                   SUM(CASE WHEN t.is_resolved = 0
                             AND EXISTS (SELECT 1 FROM review_comments c
                                          JOIN accounts a ON a.login = c.author_login
                                         WHERE c.review_thread_id = t.id
                                           AND {in_scope_predicate})
                            THEN 1 ELSE 0 END) AS unresolved_involved,
                   SUM(CASE WHEN t.is_resolved = 0
                             AND NOT EXISTS (SELECT 1 FROM review_comments c
                                              JOIN accounts a ON a.login = c.author_login
                                             WHERE c.review_thread_id = t.id
                                               AND {in_scope_predicate})
                            THEN 1 ELSE 0 END) AS unresolved_uninvolved,
                   SUM(CASE WHEN t.is_resolved = 1
                             AND EXISTS (SELECT 1 FROM review_comments c
                                          JOIN accounts a ON a.login = c.author_login
                                         WHERE c.review_thread_id = t.id
                                           AND {in_scope_predicate})
                            THEN 1 ELSE 0 END) AS resolved_involved,
                   SUM(CASE WHEN t.is_resolved = 1
                             AND NOT EXISTS (SELECT 1 FROM review_comments c
                                              JOIN accounts a ON a.login = c.author_login
                                             WHERE c.review_thread_id = t.id
                                               AND {in_scope_predicate})
                            THEN 1 ELSE 0 END) AS resolved_uninvolved
              FROM review_threads t
             GROUP BY t.pull_request_id
         ) tb ON tb.pull_request_id = pr.id"
    )
}

/// AND-compose the chip predicates for the active chip set. Returns an empty
/// string when no chips are active; otherwise returns a leading-` AND `
/// fragment that drops straight onto the end of the view's WHERE clause.
///
/// `Composition rule` from the contract: active chips AND the view; the
/// predicates themselves never reference each other.
fn chip_where_clause(active_chips: &[ChipKey]) -> String {
    if active_chips.is_empty() {
        return String::new();
    }
    let mut clause = String::new();
    for chip in active_chips {
        clause.push_str(" AND (");
        clause.push_str(triage_query::chip_predicate(*chip));
        clause.push(')');
    }
    clause
}

/// Build the per-view SQL + parameter vector for [`list_dashboard_pull_requests`].
///
/// `active_chips` is the chip-filter set; empty means "no chip filter". The
/// chips AND-compose into the WHERE per the contract's "Composition rule".
fn view_query(
    view: DashboardView,
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> (String, Vec<i64>) {
    match view {
        DashboardView::Authored => {
            relation_view_query("is_authored", sort, account_id, active_chips)
        }
        DashboardView::Assigned => {
            relation_view_query("is_review_requested", sort, account_id, active_chips)
        }
        DashboardView::Watching => {
            relation_view_query("is_involved", sort, account_id, active_chips)
        }
        DashboardView::Team => team_view_query(sort, account_id, active_chips),
        DashboardView::Archive => archive_view_query(sort, account_id, active_chips),
    }
}

/// In-scope predicate fragment for the threads rollup subquery. Returns the
/// EXISTS-clause body that scopes the involvement test to the active account
/// (single-account view) or every tracked account (union view). `?1` is the
/// account-id parameter shared with the per-view WHERE clauses so the call
/// site only pushes the value once.
fn thread_buckets_in_scope_predicate(account_id: Option<i64>) -> &'static str {
    if account_id.is_some() {
        "a.id = ?1"
    } else {
        "a.id IN (SELECT id FROM accounts)"
    }
}

/// Build the SQL for the three relation-backed views (Authored / Assigned /
/// Watching). `flag_column` must be one of `is_authored`,
/// `is_review_requested`, `is_involved`. Never user-supplied, so safe to
/// interpolate.
///
/// Two shapes here, gated on `account_id`:
///
/// 1. **Single-account (`Some(id)`).** One row per `(account, PR)` relation:
///    the FROM hangs off `pull_request_viewer_relations` with the matching
///    flag and the active account predicate. Projection reads the relation's
///    triage columns verbatim. Same as the pre-ADR-0016 behaviour.
/// 2. **Unified (`None`).** GROUP BY `pr.id`. The view-flag predicate stays a
///    WHERE on the relation row's `is_*` column so we only count PRs at
///    least one tracked account has the view-typed relation to. The triage
///    columns are merged via `MAX` / `SUM` over every relation row the
///    GROUP BY folds together (regardless of which `is_*` flags those rows
///    set), so an unread mention on a Watching relation rolls into the
///    unread tally for a PR that surfaces here via its Authored relation.
///    The relation join stays inside the FROM (no LEFT JOIN needed) because
///    the view-filter EXISTS guarantees at least one relation row per PR.
fn relation_view_query(
    flag_column: &str,
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> (String, Vec<i64>) {
    let thread_buckets = thread_buckets_subquery(thread_buckets_in_scope_predicate(account_id));
    let chip_clause = chip_where_clause(active_chips);
    match account_id {
        Some(id) => {
            // ADR 0018, decision 5: default views hide rows the active account
            // has archived. The INNER JOIN keys on the (account, PR) relation
            // row, so a `rel.archived_at IS NOT NULL` row drops directly from
            // the WHERE.
            // Post-M6: closed/merged PRs are filtered from default views
            // (`pr.state = 'open'`). The auto-archive sweep routes them to the
            // Archive view immediately on state change, so this predicate +
            // the sweep together make default views an "active work" surface
            // without a stale-data window.
            let from_and_where = format!(
                "FROM pull_request_viewer_relations rel
                 JOIN pull_requests pr ON pr.id = rel.pull_request_id
                 JOIN repos r ON r.id = pr.repo_id
                 JOIN accounts a ON a.id = rel.account_id
                 LEFT JOIN users author_u ON author_u.login = pr.author_login
                 {thread_buckets}
                 WHERE rel.{flag_column} = 1
                   AND rel.account_id = ?1
                   AND rel.archived_at IS NULL
                   AND pr.state = 'open'"
            );
            (
                build_sql(
                    &from_and_where,
                    &chip_clause,
                    sort,
                    QueryShape::SingleAccount,
                    None,
                ),
                vec![id],
            )
        }
        None => {
            // ADR 0018 unified-scope semantics: a PR is archived in the union
            // iff every relation owner has archived it. Equivalently the PR is
            // visible iff at least one relation has `archived_at IS NULL`
            // (the MAX aggregation idiom from ADR 0016:
            // `MAX(rel.archived_at IS NULL) = 1` over surviving rows).
            //
            // Two predicates encode this:
            // 1. The view-filter EXISTS gains `archived_at IS NULL` so the PR
            //    only enters the union when at least one tracked account has
            //    an unarchived relation with the right flag.
            // 2. The LEFT JOIN's ON clause filters relations to the unarchived
            //    subset so the GROUP BY merges only over unarchived rows. The
            //    `account_ids` projection then carries every surviving
            //    relation owner; archived relations don't appear in the merged
            //    row's identity. Mirrors ADR 0016's failure-isolation shape.
            //
            // Together: a PR with two relations (one archived, one not)
            // surfaces once with `account_ids` containing only the unarchived
            // owner. A PR with every relation archived fails the EXISTS and
            // drops from the union.
            let from_and_where = format!(
                "FROM pull_requests pr
                 JOIN repos r ON r.id = pr.repo_id
                 JOIN accounts acc_repo ON acc_repo.id = r.account_id
                 LEFT JOIN users author_u ON author_u.login = pr.author_login
                 LEFT JOIN pull_request_viewer_relations rel
                   ON rel.pull_request_id = pr.id
                   AND rel.archived_at IS NULL
                 {thread_buckets}
                 WHERE pr.state = 'open'
                   AND EXISTS (
                    SELECT 1 FROM pull_request_viewer_relations rel_filter
                     WHERE rel_filter.pull_request_id = pr.id
                       AND rel_filter.{flag_column} = 1
                       AND rel_filter.archived_at IS NULL
                 )"
            );
            (
                build_sql(&from_and_where, &chip_clause, sort, QueryShape::Union, None),
                Vec::new(),
            )
        }
    }
}

/// Team view: PRs in repos opted into team tracking. The relation row is read
/// account-scoped via a LEFT JOIN so the triage projections (`unread`,
/// `needs_attention`, `mentioned_count_unread`) reflect the active account.
/// Without an account filter (the union case) the LEFT JOIN drops the
/// per-account predicate so every relation row for the PR feeds the merge
/// aggregations; PRs in team-tracked repos with no relation rows still
/// surface (the view filter is `repos.is_team_tracked = 1`, not the
/// relations table) with `account_ids = []` and the default triage values.
fn team_view_query(
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> (String, Vec<i64>) {
    let thread_buckets = thread_buckets_subquery(thread_buckets_in_scope_predicate(account_id));
    let chip_clause = chip_where_clause(active_chips);
    match account_id {
        Some(id) => {
            // ADR 0018, decision 5: Team view also hides archived rows. The
            // relation join is a LEFT JOIN (PRs in team-tracked repos surface
            // even without a relation row), so the archive predicate sits on
            // the ON clause - an archived relation row drops to NULL during
            // the join and the PR keeps surfacing with the default triage
            // values (same shape as a Team-view PR the viewer has no relation
            // to). The repo-flag filter remains the row-set predicate; archive
            // is a per-account viewer signal layered on top.
            let from_and_where = format!(
                "FROM pull_requests pr
                 JOIN repos r ON r.id = pr.repo_id
                 JOIN accounts a ON a.id = r.account_id
                 LEFT JOIN users author_u ON author_u.login = pr.author_login
                 {thread_buckets}
                 LEFT JOIN pull_request_viewer_relations rel
                     ON rel.pull_request_id = pr.id
                    AND rel.account_id = ?1
                    AND rel.archived_at IS NULL
                  WHERE r.is_team_tracked = 1
                    AND r.account_id = ?1
                    AND pr.state = 'open'"
            );
            (
                build_sql(
                    &from_and_where,
                    &chip_clause,
                    sort,
                    QueryShape::SingleAccount,
                    None,
                ),
                vec![id],
            )
        }
        None => {
            // ADR 0018 unified-scope Team view: hide a PR iff it has at least
            // one relation AND every relation is archived. A team-tracked PR
            // with no relations stays visible (the team filter is repo-based;
            // there is nothing to archive). A PR with mixed archive states
            // stays visible via the unarchived relation.
            //
            // The LEFT JOIN filters relations to the unarchived subset so the
            // GROUP BY's `account_ids` only carries surviving relation owners.
            // The WHERE adds a "no relations OR at least one unarchived" guard
            // so the all-archived case drops from the union.
            let from_and_where = format!(
                "FROM pull_requests pr
                 JOIN repos r ON r.id = pr.repo_id
                 JOIN accounts acc_repo ON acc_repo.id = r.account_id
                 LEFT JOIN users author_u ON author_u.login = pr.author_login
                 LEFT JOIN pull_request_viewer_relations rel
                     ON rel.pull_request_id = pr.id
                    AND rel.archived_at IS NULL
                 {thread_buckets}
                  WHERE r.is_team_tracked = 1
                    AND pr.state = 'open'
                    AND (NOT EXISTS (
                            SELECT 1 FROM pull_request_viewer_relations rel_any
                             WHERE rel_any.pull_request_id = pr.id
                         )
                         OR EXISTS (
                            SELECT 1 FROM pull_request_viewer_relations rel_un
                             WHERE rel_un.pull_request_id = pr.id
                               AND rel_un.archived_at IS NULL
                         ))"
            );
            (
                build_sql(&from_and_where, &chip_clause, sort, QueryShape::Union, None),
                Vec::new(),
            )
        }
    }
}

/// Archive view: PRs the viewer has archived (ADR 0018). Inverts the
/// archive predicate from the four default views - the FROM/WHERE keys on
/// `rel.archived_at IS NOT NULL`. Ignores `is_authored` / `is_review_requested`
/// / `is_involved` / `repos.is_team_tracked`; archive is global across every
/// relation a viewer holds.
///
/// Default sort: `archived_at DESC`, most-recently-archived first. The Archive
/// view substitutes this for `DashboardSort::Updated` (the contract's default
/// passed in by the dashboard store); the explicit `Stale` and `NeedsMe`
/// selections still apply when the user picks them.
///
/// Filter chips intentionally do not apply in this PR. Most chips
/// (`needs-attention`, `stale`) are oriented around the active queue; their
/// interaction with the archive view is a UX decision deferred to the W2
/// frontend issue. Active chips passed in are still composed into the WHERE
/// so the SQL composition stays uniform with the other views, but the W2 UI
/// is expected to hide the chip rail on this view.
fn archive_view_query(
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> (String, Vec<i64>) {
    let thread_buckets = thread_buckets_subquery(thread_buckets_in_scope_predicate(account_id));
    let chip_clause = chip_where_clause(active_chips);
    // `archived_at DESC` is the Archive view's most-recently-archived-first
    // default; `pr.id DESC` keeps the order stable when two relations share
    // an archive timestamp.
    let single_order = "ORDER BY rel.archived_at DESC, pr.id DESC";
    let union_order = "ORDER BY MAX(rel.archived_at) DESC, pr.id DESC";
    let order_override_single = match sort {
        DashboardSort::Updated => Some(single_order),
        _ => None,
    };
    let order_override_union = match sort {
        DashboardSort::Updated => Some(union_order),
        _ => None,
    };
    match account_id {
        Some(id) => {
            // Single-account scope: INNER JOIN keys on the (account, PR)
            // relation row, predicate inverts the default-view archive guard.
            let from_and_where = format!(
                "FROM pull_request_viewer_relations rel
                 JOIN pull_requests pr ON pr.id = rel.pull_request_id
                 JOIN repos r ON r.id = pr.repo_id
                 JOIN accounts a ON a.id = rel.account_id
                 LEFT JOIN users author_u ON author_u.login = pr.author_login
                 {thread_buckets}
                 WHERE rel.account_id = ?1
                   AND rel.archived_at IS NOT NULL"
            );
            (
                build_sql(
                    &from_and_where,
                    &chip_clause,
                    sort,
                    QueryShape::SingleAccount,
                    order_override_single,
                ),
                vec![id],
            )
        }
        None => {
            // Unified scope: the view surfaces a PR iff at least one tracked
            // account has archived it. The LEFT JOIN admits only archived
            // relations so `account_ids` reflects the archiving owners. The
            // view-filter EXISTS bounds the row set to PRs with at least one
            // archived relation; an unarchived relation on a co-owner doesn't
            // pull the row out of the archive (that's the symmetric inverse
            // of the default-view rule - the row appears in both the active
            // queue and the archive when only some accounts archived).
            let from_and_where = format!(
                "FROM pull_requests pr
                 JOIN repos r ON r.id = pr.repo_id
                 JOIN accounts acc_repo ON acc_repo.id = r.account_id
                 LEFT JOIN users author_u ON author_u.login = pr.author_login
                 LEFT JOIN pull_request_viewer_relations rel
                   ON rel.pull_request_id = pr.id
                   AND rel.archived_at IS NOT NULL
                 {thread_buckets}
                 WHERE EXISTS (
                    SELECT 1 FROM pull_request_viewer_relations rel_filter
                     WHERE rel_filter.pull_request_id = pr.id
                       AND rel_filter.archived_at IS NOT NULL
                 )"
            );
            (
                build_sql(
                    &from_and_where,
                    &chip_clause,
                    sort,
                    QueryShape::Union,
                    order_override_union,
                ),
                Vec::new(),
            )
        }
    }
}

/// Execute the per-view list query against `conn` and project each row into a
/// `DashboardPullRequest` with empty reviewer lists. Reviewer hydration is a
/// second pass so we batch one `WHERE pull_request_id IN (...)` query per call
/// instead of N per-row reads.
///
/// `active_chips` is the chip-filter set; an empty slice means "no chip
/// filter applied". See `docs/contracts/triage-ux.md` ("Filter chip semantics")
/// for the chip predicates and composition rule.
pub fn list_pull_requests(
    conn: &Connection,
    view: DashboardView,
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> Result<Vec<DashboardPullRequest>, rusqlite::Error> {
    let (sql, params) = view_query(view, sort, account_id, active_chips);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params.iter()))?;

    let mut prs: Vec<DashboardPullRequest> = Vec::new();
    while let Some(row) = rows.next()? {
        prs.push(project_pr_row(row)?);
    }
    drop(rows);
    drop(stmt);

    if !prs.is_empty() {
        hydrate_reviewers(conn, &mut prs)?;
    }
    Ok(prs)
}

/// Project one PR row using the column order in [`PR_PROJECTION_COLUMNS`].
fn project_pr_row(row: &Row<'_>) -> Result<DashboardPullRequest, rusqlite::Error> {
    let draft: i64 = row.get(4)?;
    let ci_state: Option<String> = row.get(17)?;
    let ci_total: Option<i64> = row.get(18)?;
    let ci_passing: Option<i64> = row.get(19)?;
    let ci = ci_state.map(|state| CiSummary {
        state,
        total: ci_total.unwrap_or(0),
        passing: ci_passing.unwrap_or(0),
    });

    // ADR 0016: the four bucket counts come from the `thread_buckets`
    // LEFT JOIN. A PR with no `review_threads` rows misses the join entirely;
    // `COALESCE(..., 0)` defaults the columns to zero so the contract's
    // "muted em-dash state" branch trips here. A PR whose buckets are all
    // zero but threads exist (every row dropped from involvement, impossible
    // at v1 sizes) reads the same way - acceptable: zero threads renders
    // nothing.
    let threads_total: i64 = row.get(20)?;
    let threads_unresolved_involved: i64 = row.get(21)?;
    let threads_unresolved_uninvolved: i64 = row.get(22)?;
    let threads_resolved_involved: i64 = row.get(23)?;
    let threads_resolved_uninvolved: i64 = row.get(24)?;
    let threads = if threads_total == 0 {
        None
    } else {
        Some(ThreadsSummary {
            total: threads_total,
            unresolved_involved: threads_unresolved_involved,
            unresolved_uninvolved: threads_unresolved_uninvolved,
            resolved_involved: threads_resolved_involved,
            resolved_uninvolved: threads_resolved_uninvolved,
        })
    };

    let repo_id: i64 = row.get(25)?;
    let repo_owner: String = row.get(26)?;
    let repo_name: String = row.get(27)?;
    // Column 28 carries `account_ids` as a CSV string:
    // - single-account path: `CAST(a.id AS TEXT)` -> exactly one id, e.g. "1".
    // - union path: `GROUP_CONCAT(DISTINCT rel.account_id ORDER BY ...)` ->
    //   a sorted, comma-joined list. Empty when no relation row was joined
    //   (Team-view PR with no relations).
    let account_ids_csv: String = row.get(28)?;
    let account_ids = parse_account_ids_csv(&account_ids_csv);
    let account_host: String = row.get(29)?;

    // M4-C: triage projections from `pull_request_viewer_relations rel`.
    // `unread` is computed in SQL via CASE; COALESCE handles missing relation
    // rows (Team-view union case) by defaulting to false / 0. See ADR 0015
    // ("Read-state storage") and `docs/contracts/triage-ux.md`
    // ("Read-state derivation").
    //
    // In the unified path the column carries `MAX(...)` / `SUM(...)` over
    // every relation row the GROUP BY folded together, so the same scalar
    // read works for both shapes. When no relation joined, MAX returns NULL
    // and Option<i64> would surface that; the outer COALESCE in the SQL
    // pins the default to 0 either way.
    let unread: i64 = row.get(30)?;
    let needs_attention: i64 = row.get(31)?;
    // SUM in SQLite returns NULL when every aggregated row is NULL (i.e. no
    // relation joined). The outer COALESCE keeps the column non-NULL in the
    // single-account path; the union path needs the Option<i64> read because
    // `SUM(COALESCE(..., 0))` over zero rows still returns NULL.
    let mentioned_count_unread: i64 = row.get::<_, Option<i64>>(32)?.unwrap_or(0);

    let pr_number: i64 = row.get(1)?;
    let url = format!("https://{account_host}/{repo_owner}/{repo_name}/pull/{pr_number}");

    Ok(DashboardPullRequest {
        id: row.get(0)?,
        number: pr_number,
        title: row.get(2)?,
        url,
        state: row.get(3)?,
        is_draft: draft != 0,
        mergeable: row.get(5)?,
        review_decision: row.get(6)?,
        author_login: row.get(7)?,
        author_avatar_url: row.get(8)?,
        base_ref: row.get(9)?,
        head_ref: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        latest_status_change_at: row.get(13)?,
        additions: row.get(14)?,
        deletions: row.get(15)?,
        changed_files: row.get(16)?,
        ci,
        threads,
        reviewers: Vec::new(),
        repo: RepoRef {
            id: repo_id,
            owner: repo_owner,
            name: repo_name,
        },
        account_ids,
        unread: unread != 0,
        needs_attention: needs_attention != 0,
        mentioned_count_unread,
    })
}

/// Parse the comma-separated `account_ids` projection into a sorted vector.
/// Empty input -> empty vector (Team-view PR with no relations).
/// Non-numeric tokens are dropped silently; the projection only ever emits
/// integer ids so a parse failure indicates a SQL composition bug rather than
/// a runtime data shape we should propagate. The list is already sorted in
/// the union path via `GROUP_CONCAT(... ORDER BY rel.account_id)`; the sort
/// here is defensive for the single-account path's single-id case and any
/// future caller that doesn't pre-sort.
fn parse_account_ids_csv(csv: &str) -> Vec<i64> {
    if csv.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<i64> = csv
        .split(',')
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();
    out.sort_unstable();
    out
}

/// Reviewer entries for one PR: one row per unique reviewer login, picking the
/// latest submitted review per login (with a state-priority tie-break), then
/// filling in pending entries for requested reviewers who have never
/// submitted. The viewer marker `is_you` is true when the reviewer's identity
/// `(login, host)` matches any of the row's `account_ids` -> `(login, host)`
/// pairs _and_ the matched account lives on the PR's owning host. Rows whose
/// latest state doesn't map to a [`ReviewerState`] (e.g. `DISMISSED`) are
/// dropped; a login dropped this way does not re-appear as `Pending` from
/// `requested_reviewers` because the login still counts as having a submitted
/// review.
///
/// Cross-host isolation (issue #169): GitHub logins are unique per host, not
/// globally. Two accounts can share login `ada` on github.com and on a GHE
/// host, but they are different identities. The lookup keys on
/// `account_id -> (login, host)` and the `is_you` test compares against the
/// PR's owning host (fetched per call from `repos -> accounts`). If none of
/// the row's in-scope account identities sits on the PR's host, `is_you`
/// stays false regardless of any login string coincidence.
///
/// In the unified path `pr.account_ids` carries every relation owner the
/// GROUP BY folded together; the `is_you` scan tests the reviewer against
/// the union of those identities. A reviewer logged in as account B's login
/// (but not account A's) still flips `is_you` if account B has a relation row
/// for this PR and shares the PR's host.
fn hydrate_reviewers(
    conn: &Connection,
    prs: &mut [DashboardPullRequest],
) -> Result<(), rusqlite::Error> {
    // account_id -> (login, host). Lookup keyed by the row's `account_ids`;
    // `host` is carried so the cross-host `is_you` guard below can compare it
    // against the PR's owning host.
    let mut account_identities: HashMap<i64, (String, String)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT id, login, host FROM accounts")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let login: String = row.get(1)?;
            let host: String = row.get(2)?;
            account_identities.insert(id, (login, host));
        }
    }

    // Bucket the PR ids so we can fetch reviewers in one round trip per source
    // table (reviews + requested_reviewers).
    let pr_ids: Vec<i64> = prs.iter().map(|pr| pr.id).collect();
    let placeholders = vec!["?"; pr_ids.len()].join(",");

    // PR id -> owning host (the repo's owning account host). One round trip
    // for the loaded set; the EXISTS / JOIN form would otherwise rerun per row
    // during the `is_you` derivation.
    let mut pr_owner_host_by_pr: HashMap<i64, String> = HashMap::new();
    {
        let sql = format!(
            "SELECT pr.id, acc.host
               FROM pull_requests pr
               JOIN repos r ON r.id = pr.repo_id
               JOIN accounts acc ON acc.id = r.account_id
              WHERE pr.id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(pr_ids.iter()))?;
        while let Some(row) = rows.next()? {
            let pr_id: i64 = row.get(0)?;
            let host: String = row.get(1)?;
            pr_owner_host_by_pr.insert(pr_id, host);
        }
    }

    let mut reviewers_by_pr: HashMap<i64, Vec<ReviewerEntry>> = HashMap::new();
    // Track every login that has _any_ submitted review per PR (including
    // DISMISSED) so the pending pass below can skip them — a reviewer who
    // submitted then was dismissed shouldn't reappear as pending.
    let mut submitted_logins_by_pr: HashMap<i64, std::collections::HashSet<String>> =
        HashMap::new();

    // Submitted reviews, deduplicated to one row per (PR, login). The window
    // function picks the latest `submitted_at`; ties break by state priority
    // (`CHANGES_REQUESTED` > `APPROVED` > `COMMENTED` > `DISMISSED` > `PENDING`)
    // so a same-second pair surfaces the more actionable state. `LEFT JOIN
    // users` resolves the reviewer's avatar URL (ADR 0013).
    let sql_reviews = format!(
        "WITH ranked_reviews AS (
            SELECT
                r.pull_request_id,
                r.reviewer_login,
                r.state,
                ROW_NUMBER() OVER (
                    PARTITION BY r.pull_request_id, r.reviewer_login
                    ORDER BY COALESCE(r.submitted_at, 0) DESC,
                             CASE r.state
                                 WHEN 'CHANGES_REQUESTED' THEN 0
                                 WHEN 'APPROVED'          THEN 1
                                 WHEN 'COMMENTED'         THEN 2
                                 WHEN 'DISMISSED'         THEN 3
                                 WHEN 'PENDING'           THEN 4
                                 ELSE 5
                             END ASC,
                             r.id DESC
                ) AS rn
              FROM reviews r
             WHERE r.pull_request_id IN ({placeholders})
         )
         SELECT lr.pull_request_id, lr.reviewer_login, lr.state, u.avatar_url
           FROM ranked_reviews lr
           LEFT JOIN users u ON u.login = lr.reviewer_login
          WHERE lr.rn = 1"
    );
    {
        let mut stmt = conn.prepare(&sql_reviews)?;
        let mut rows = stmt.query(params_from_iter(pr_ids.iter()))?;
        while let Some(row) = rows.next()? {
            let pr_id: i64 = row.get(0)?;
            let login: String = row.get(1)?;
            let state_str: String = row.get(2)?;
            let avatar_url: Option<String> = row.get(3)?;
            submitted_logins_by_pr
                .entry(pr_id)
                .or_default()
                .insert(login.clone());
            let Some(state) = map_review_state(&state_str) else {
                continue;
            };
            reviewers_by_pr
                .entry(pr_id)
                .or_default()
                .push(ReviewerEntry {
                    login,
                    state,
                    is_you: false,
                    avatar_url,
                });
        }
    }

    // Requested-but-not-submitted reviewers. A login that already has a
    // submitted review for this PR (any state, including DISMISSED) is
    // excluded so it surfaces once with the submitted state — or not at all
    // if the only state was DISMISSED.
    let sql_requested = format!(
        "SELECT rr.pull_request_id, rr.login, u.avatar_url
           FROM requested_reviewers rr
           LEFT JOIN users u ON u.login = rr.login
          WHERE rr.pull_request_id IN ({placeholders})"
    );
    {
        let mut stmt = conn.prepare(&sql_requested)?;
        let mut rows = stmt.query(params_from_iter(pr_ids.iter()))?;
        let mut seen_pending: HashMap<i64, std::collections::HashSet<String>> = HashMap::new();
        while let Some(row) = rows.next()? {
            let pr_id: i64 = row.get(0)?;
            let login: String = row.get(1)?;
            let avatar_url: Option<String> = row.get(2)?;
            if submitted_logins_by_pr
                .get(&pr_id)
                .is_some_and(|set| set.contains(&login))
            {
                continue;
            }
            // Guard against duplicate `requested_reviewers` rows for the same
            // login (e.g. multiple sync passes layering on team requests).
            if !seen_pending.entry(pr_id).or_default().insert(login.clone()) {
                continue;
            }
            reviewers_by_pr
                .entry(pr_id)
                .or_default()
                .push(ReviewerEntry {
                    login,
                    state: ReviewerState::Pending,
                    is_you: false,
                    avatar_url,
                });
        }
    }

    // Attach to the parent PR rows and compute `is_you`. The marker requires
    // both a login string match and a host match against the PR's owning host
    // (issue #169 - same login on different hosts is a different identity).
    // In the unified path `pr.account_ids` carries every in-scope account
    // for this row; the scan tests against the union of their identities.
    for pr in prs.iter_mut() {
        if let Some(mut entries) = reviewers_by_pr.remove(&pr.id) {
            let pr_host = pr_owner_host_by_pr.get(&pr.id);
            // Collect the (login, host) pairs that share the PR's host; these
            // are the identities a reviewer's login can match to flip `is_you`.
            // Other in-scope accounts (on different hosts) can't match: their
            // login is a different identity even if the string coincides.
            let viewer_logins_on_pr_host: Vec<&str> = pr
                .account_ids
                .iter()
                .filter_map(|id| account_identities.get(id))
                .filter(|(_, viewer_host)| pr_host.is_some_and(|h| h == viewer_host))
                .map(|(login, _)| login.as_str())
                .collect();
            for entry in entries.iter_mut() {
                entry.is_you = viewer_logins_on_pr_host
                    .iter()
                    .any(|login| *login == entry.login);
            }
            pr.reviewers = entries;
        }
    }
    Ok(())
}

/// GraphQL review state strings -> internal `ReviewerState`. Unknown values
/// (`DISMISSED`, `PENDING` from a draft review, future enum values) return
/// `None` so the caller drops the row.
fn map_review_state(state: &str) -> Option<ReviewerState> {
    match state {
        "APPROVED" => Some(ReviewerState::Approved),
        "CHANGES_REQUESTED" => Some(ReviewerState::ChangesRequested),
        "COMMENTED" => Some(ReviewerState::Commented),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    //! In-module tests cover the private helpers (SQL composition and the
    //! review-state mapping). End-to-end behaviour against the public
    //! [`list_pull_requests`] function lives in
    //! `src-tauri/tests/dashboard_query.rs`.

    use super::*;

    #[test]
    fn map_review_state_drops_unknown_strings() {
        assert_eq!(map_review_state("APPROVED"), Some(ReviewerState::Approved));
        assert_eq!(
            map_review_state("CHANGES_REQUESTED"),
            Some(ReviewerState::ChangesRequested)
        );
        assert_eq!(
            map_review_state("COMMENTED"),
            Some(ReviewerState::Commented)
        );
        assert_eq!(map_review_state("DISMISSED"), None);
        assert_eq!(map_review_state("PENDING"), None);
        assert_eq!(map_review_state(""), None);
    }

    /// The partial index on `pull_request_viewer_relations` requires
    /// `is_authored = 1` in the predicate. Assert the SQL contains the
    /// matching clause so a future refactor doesn't drop the planner hint.
    #[test]
    fn authored_query_includes_is_authored_predicate() {
        let (sql, _) = view_query(
            DashboardView::Authored,
            DashboardSort::Updated,
            Some(1),
            &[],
        );
        assert!(
            sql.contains("rel.is_authored = 1"),
            "authored query missing flag predicate; SQL: {sql}"
        );
    }

    #[test]
    fn assigned_query_uses_review_requested_predicate() {
        let (sql, _) = view_query(
            DashboardView::Assigned,
            DashboardSort::Updated,
            Some(1),
            &[],
        );
        assert!(sql.contains("rel.is_review_requested = 1"), "SQL: {sql}");
    }

    #[test]
    fn watching_query_uses_involved_predicate() {
        let (sql, _) = view_query(
            DashboardView::Watching,
            DashboardSort::Updated,
            Some(1),
            &[],
        );
        assert!(sql.contains("rel.is_involved = 1"), "SQL: {sql}");
    }

    #[test]
    fn team_query_uses_repo_team_flag() {
        let (sql, _) = view_query(DashboardView::Team, DashboardSort::Updated, Some(1), &[]);
        assert!(
            sql.contains("r.is_team_tracked = 1"),
            "team query missing repo flag; SQL: {sql}"
        );
    }

    /// M4-C: Team view LEFT JOINs the relations table so the triage projections
    /// (`unread`, `needs_attention`, `mentioned_count_unread`) reflect the
    /// active account. With no account scope the join short-circuits via
    /// `ON 0` so the COALESCE defaults trip.
    #[test]
    fn team_query_left_joins_relations_when_account_scoped() {
        let (sql, _) = view_query(DashboardView::Team, DashboardSort::Updated, Some(1), &[]);
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel"),
            "team query must LEFT JOIN relations when account-scoped; SQL: {sql}"
        );
        assert!(
            sql.contains("rel.account_id = ?1"),
            "team query relation join must filter on account; SQL: {sql}"
        );
    }

    #[test]
    fn team_query_union_left_joins_relations_without_account_predicate() {
        // ADR 0016: the union path keeps the LEFT JOIN to feed the merge
        // aggregations, but drops the `rel.account_id = ?1` predicate so every
        // relation row for the PR contributes. PRs in team-tracked repos
        // without any relation rows still surface (view filter is on
        // `repos.is_team_tracked = 1`); the merge aggregates over zero
        // relation rows and defaults the triage columns via COALESCE.
        let (sql, params) = view_query(DashboardView::Team, DashboardSort::Updated, None, &[]);
        assert!(params.is_empty());
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel\n                     ON rel.pull_request_id = pr.id"),
            "team union path must LEFT JOIN relations without an account filter; SQL: {sql}"
        );
        assert!(
            !sql.contains("LEFT JOIN pull_request_viewer_relations rel ON 0"),
            "team union path must not short-circuit the relation join; SQL: {sql}"
        );
        assert!(
            sql.contains("GROUP BY pr.id"),
            "team union path must dedupe via GROUP BY pr.id; SQL: {sql}"
        );
    }

    #[test]
    fn team_query_left_joins_relations_for_needs_me_sort() {
        let (sql, _) = view_query(DashboardView::Team, DashboardSort::NeedsMe, Some(1), &[]);
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel"),
            "team query missing LEFT JOIN for NeedsMe sort; SQL: {sql}"
        );
        assert!(
            sql.contains("rel.account_id = ?1"),
            "team query missing account scope on the LEFT JOIN; SQL: {sql}"
        );
    }

    #[test]
    fn team_query_left_joins_relations_for_needs_attention_chip() {
        let (sql, _) = view_query(
            DashboardView::Team,
            DashboardSort::Updated,
            Some(1),
            &[ChipKey::NeedsAttention],
        );
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel"),
            "team query missing LEFT JOIN for needs-attention chip; SQL: {sql}"
        );
    }

    #[test]
    fn account_id_none_omits_account_filter() {
        let (sql, params) = view_query(DashboardView::Authored, DashboardSort::Updated, None, &[]);
        assert!(params.is_empty());
        assert!(
            !sql.contains("rel.account_id ="),
            "expected no account predicate; SQL: {sql}"
        );
    }

    #[test]
    fn account_id_some_appends_account_filter_with_param() {
        let (sql, params) = view_query(
            DashboardView::Authored,
            DashboardSort::Updated,
            Some(42),
            &[],
        );
        assert_eq!(params, vec![42]);
        assert!(sql.contains("rel.account_id ="), "SQL: {sql}");
    }

    /// M4-C: the SELECT projects three triage columns derived from the
    /// `pull_request_viewer_relations rel` alias.
    #[test]
    fn projection_includes_triage_columns() {
        let (sql, _) = view_query(
            DashboardView::Authored,
            DashboardSort::Updated,
            Some(1),
            &[],
        );
        assert!(
            sql.contains("AS unread"),
            "expected unread column in projection; SQL: {sql}"
        );
        assert!(
            sql.contains("COALESCE(rel.needs_attention, 0) AS needs_attention"),
            "expected needs_attention column; SQL: {sql}"
        );
        assert!(
            sql.contains("COALESCE(rel.mentioned_count_unread, 0) AS mentioned_count_unread"),
            "expected mentioned_count_unread column; SQL: {sql}"
        );
    }

    #[test]
    fn updated_sort_orders_by_coalesce_lsc_updated() {
        let (sql, _) = view_query(DashboardView::Watching, DashboardSort::Updated, None, &[]);
        assert!(
            sql.contains("ORDER BY COALESCE(pr.latest_status_change_at, pr.updated_at) DESC"),
            "SQL: {sql}"
        );
    }

    #[test]
    fn stale_sort_orders_by_updated_at_ascending() {
        let (sql, _) = view_query(DashboardView::Watching, DashboardSort::Stale, Some(1), &[]);
        assert!(
            sql.contains("ORDER BY pr.updated_at ASC, pr.id DESC"),
            "Stale sort ORDER BY mismatch; SQL: {sql}"
        );
    }

    #[test]
    fn needs_me_sort_orders_by_needs_attention_desc_with_coalesce_tiebreak() {
        let (sql, _) = view_query(
            DashboardView::Watching,
            DashboardSort::NeedsMe,
            Some(1),
            &[],
        );
        assert!(
            sql.contains("ORDER BY COALESCE(rel.needs_attention, 0) DESC"),
            "NeedsMe sort missing relation column; SQL: {sql}"
        );
        assert!(
            sql.contains("COALESCE(pr.latest_status_change_at, pr.updated_at) DESC"),
            "NeedsMe sort missing tie-break; SQL: {sql}"
        );
    }

    #[test]
    fn chip_where_clause_is_empty_when_no_chips_active() {
        assert_eq!(chip_where_clause(&[]), "");
    }

    #[test]
    fn chip_where_clause_and_composes_active_chips() {
        let clause = chip_where_clause(&[ChipKey::Drafts, ChipKey::CiFailing]);
        assert!(
            clause.contains("AND (pr.draft = 1)"),
            "missing drafts: {clause}"
        );
        assert!(
            clause.contains("AND (pr.ci_state IN ('FAILURE', 'ERROR'))"),
            "missing ci_failing: {clause}"
        );
    }

    #[test]
    fn active_chip_predicate_lands_in_view_sql() {
        let (sql, _) = view_query(
            DashboardView::Authored,
            DashboardSort::Updated,
            Some(1),
            &[ChipKey::Drafts],
        );
        assert!(
            sql.contains("AND (pr.draft = 1)"),
            "chip predicate missing; SQL: {sql}"
        );
    }

    // ===== ADR 0016: query-time threads rollup =====

    #[test]
    fn threads_rollup_uses_subquery_left_join_not_pull_requests_columns() {
        let (sql, _) = view_query(
            DashboardView::Authored,
            DashboardSort::Updated,
            Some(1),
            &[],
        );
        assert!(
            sql.contains("LEFT JOIN ("),
            "expected thread_buckets LEFT JOIN subquery; SQL: {sql}"
        );
        assert!(
            !sql.contains("pr.threads_total"),
            "row projection must not read the legacy column; SQL: {sql}"
        );
        assert!(
            sql.contains("COALESCE(tb.total, 0)"),
            "expected COALESCE on the subquery's total; SQL: {sql}"
        );
    }

    #[test]
    fn threads_rollup_in_scope_uses_active_account_when_filtered() {
        let (sql, params) = view_query(
            DashboardView::Watching,
            DashboardSort::Updated,
            Some(7),
            &[],
        );
        assert!(
            sql.contains("a.id = ?1"),
            "single-account threads in-scope must reuse ?1; SQL: {sql}"
        );
        assert!(
            !sql.contains("a.id IN (SELECT id FROM accounts)"),
            "single-account path must not union over every account; SQL: {sql}"
        );
        assert_eq!(params, vec![7]);
    }

    #[test]
    fn threads_rollup_in_scope_unions_every_account_when_unfiltered() {
        let (sql, params) = view_query(DashboardView::Watching, DashboardSort::Updated, None, &[]);
        assert!(params.is_empty());
        assert!(
            sql.contains("a.id IN (SELECT id FROM accounts)"),
            "union path must scope the involvement test across every tracked \
             account; SQL: {sql}"
        );
    }

    /// Team view's account-scoped path uses `?1` twice for the relation join.
    /// The threads rollup reuses the same parameter so the bound vector stays
    /// length-1.
    #[test]
    fn team_query_account_scoped_threads_rollup_reuses_account_parameter() {
        let (sql, params) = view_query(DashboardView::Team, DashboardSort::Updated, Some(3), &[]);
        assert_eq!(params, vec![3], "single bound i64 even though ?1 reappears");
        assert!(
            sql.contains("a.id = ?1"),
            "threads rollup must scope by ?1; SQL: {sql}"
        );
    }
}
