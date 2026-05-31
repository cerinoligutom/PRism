//! Read-only SQL composition for `list_dashboard_pull_requests`.
//!
//! See `docs/contracts/dashboard-data.md` for the DTO contract and the
//! per-view shape this module implements. Each view shares the same outer
//! projection and reviewer hydration; they differ only in the FROM clause
//! that selects the PR set:
//!
//! - Authored / Assigned / Watching read from `pull_request_viewer_relations`
//!   gated by the matching flag column (each flag has a partial index).
//! - Tracked reads `repos.is_tracked = 1` directly; the relations table is
//!   not touched because the Tracked relationship is a repo property.
//!
//! `account_id = None` returns the dedupe-and-merge union across every
//! account (ADR 0016). The relation-backed views GROUP BY `pr.id` so a PR
//! authored by account A and review-requested for account B surfaces as one
//! row whose triage signals are merged (`unread = MAX`,
//! `needs_attention = MAX`) and whose
//! `account_ids` carries every relation owner. The Tracked view's union path
//! keeps the same GROUP BY but joins relations without an account scope so
//! every relation row a PR has feeds the aggregations; PRs in tracked repos
//! with no relation rows still surface (the Tracked filter is the repo's
//! `is_tracked = 1`, not the relations table) with an empty `account_ids`
//! and the default triage values.
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
//! signal that a PR surfaced via the Tracked-view path (or that every
//! relation row for the PR was pruned in the most recent cycle).

use std::collections::HashMap;

use rusqlite::{params_from_iter, Connection, Row};

use crate::dashboard::types::{
    CiSummary, DashboardPullRequest, DashboardSort, DashboardView, DashboardViewCounts,
    MyReviewState, RepoRef, ReviewerEntry, ReviewerState, ThreadsSummary,
};
use crate::triage::query as triage_query;
use crate::triage::types::ChipKey;

/// SQL fragment that selects every column the row projection needs in the
/// single-account-scoped path. Joined to `repos` and `accounts`; each view
/// prepends its own FROM clause to this body.
///
/// The trailing `rel.*` projections (M4-C) read the active account's triage
/// state. Relation-backed views (Authored / Assigned / Watching) already JOIN
/// `pull_request_viewer_relations rel` so the columns flow directly. The
/// Tracked view adds a LEFT JOIN on the same alias - scoped to the active
/// account when one is provided, an inert `ON 0` join otherwise - so the
/// SELECT keeps a stable shape across every view. See
/// `docs/contracts/triage-ux.md` ("Read-state derivation") and ADR 0015.
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
    pr.is_draft,
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
    COALESCE(rel.needs_attention, 0) AS needs_attention
";

/// Unified-mode projection. Adds the per-relation merge aggregations
/// (`MAX(unread)`, `MAX(needs_attention)`) and the comma-separated
/// `account_ids` marker. `host` is read from the repo's
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
    pr.is_draft,
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
    MAX(COALESCE(rel.needs_attention, 0)) AS needs_attention
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
/// `pull_request_viewer_relations` in the Tracked view path).
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
            // `COALESCE` keeps the column NULL-safe for the Tracked view path,
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
        DashboardView::Tracked => tracked_view_query(sort, account_id, active_chips),
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

/// Tracked view: PRs in repos opted into Tracked. The relation row is read
/// account-scoped via a LEFT JOIN so the triage projections (`unread`,
/// `needs_attention`) reflect the active account.
/// Without an account filter (the union case) the LEFT JOIN drops the
/// per-account predicate so every relation row for the PR feeds the merge
/// aggregations; PRs in tracked repos with no relation rows still surface
/// (the view filter is `repos.is_tracked = 1`, not the relations table)
/// with `account_ids = []` and the default triage values.
fn tracked_view_query(
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> (String, Vec<i64>) {
    let thread_buckets = thread_buckets_subquery(thread_buckets_in_scope_predicate(account_id));
    let chip_clause = chip_where_clause(active_chips);
    match account_id {
        Some(id) => {
            // ADR 0018, decision 5: Tracked view also hides archived rows.
            // The relation join is a LEFT JOIN (PRs in tracked repos surface
            // even without a relation row), so the archive predicate sits on
            // the ON clause - an archived relation row drops to NULL during
            // the join and the PR keeps surfacing with the default triage
            // values (same shape as a Tracked-view PR the viewer has no
            // relation to). The repo-flag filter remains the row-set
            // predicate; archive is a per-account viewer signal layered on
            // top.
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
                  WHERE r.is_tracked = 1
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
            // ADR 0018 unified-scope Tracked view: hide a PR iff it has at
            // least one relation AND every relation is archived. A tracked PR
            // with no relations stays visible (the Tracked filter is
            // repo-based; there is nothing to archive). A PR with mixed
            // archive states stays visible via the unarchived relation.
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
                  WHERE r.is_tracked = 1
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
/// / `is_involved` / `repos.is_tracked`; archive is global across every
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

/// Count-friendly FROM + WHERE for the named view + account scope. Mirrors the
/// view predicates [`relation_view_query`], [`tracked_view_query`], and
/// [`archive_view_query`] use, stripped of the projection-only joins
/// (`repos`, `accounts`, `users`, `thread_buckets`) so the planner only walks
/// what the COUNT(*) needs.
///
/// The single-account path always parameterises the account predicate to `?1`;
/// the union path returns no parameters. Callers must alias `?1` consistently
/// across the five scope fragments when composing them into a single combined
/// SELECT (see [`list_view_counts`]).
///
/// Excludes the projection / hydration the row-returning path needs because
/// none of it affects the row count: `repos.id`, `accounts.host`, the user
/// avatar joins, and the `thread_buckets` subquery all live in the SELECT
/// projection, not the WHERE.
fn view_row_scope_sql(view: DashboardView, account_id: Option<i64>) -> (String, Vec<i64>) {
    match view {
        DashboardView::Authored => relation_view_row_scope("is_authored", account_id),
        DashboardView::Assigned => relation_view_row_scope("is_review_requested", account_id),
        DashboardView::Watching => relation_view_row_scope("is_involved", account_id),
        DashboardView::Tracked => tracked_view_row_scope(account_id),
        DashboardView::Archive => archive_view_row_scope(account_id),
    }
}

/// Row-scope FROM + WHERE for the three relation-backed default views. Shapes:
///
/// - Single-account: `pull_request_viewer_relations rel` -> `pull_requests pr`
///   with the matching flag, active account, unarchived, and open-state
///   predicates. One row per qualifying (account, PR) pair, which is one row
///   per PR in this view since the active account is fixed.
/// - Union: `pull_requests pr` with an EXISTS over relations carrying the
///   matching flag and unarchived state. One row per PR.
///
/// `flag_column` must be `is_authored`, `is_review_requested`, or `is_involved`
/// (never user-supplied, safe to interpolate). Predicate text mirrors
/// [`relation_view_query`] verbatim so a row admitted there is admitted here.
fn relation_view_row_scope(flag_column: &str, account_id: Option<i64>) -> (String, Vec<i64>) {
    match account_id {
        Some(id) => (
            format!(
                "FROM pull_request_viewer_relations rel
                 JOIN pull_requests pr ON pr.id = rel.pull_request_id
                 WHERE rel.{flag_column} = 1
                   AND rel.account_id = ?1
                   AND rel.archived_at IS NULL
                   AND pr.state = 'open'"
            ),
            vec![id],
        ),
        None => (
            format!(
                "FROM pull_requests pr
                 WHERE pr.state = 'open'
                   AND EXISTS (
                       SELECT 1 FROM pull_request_viewer_relations rel_filter
                        WHERE rel_filter.pull_request_id = pr.id
                          AND rel_filter.{flag_column} = 1
                          AND rel_filter.archived_at IS NULL
                   )"
            ),
            Vec::new(),
        ),
    }
}

/// Row-scope FROM + WHERE for the Tracked view. Single-account scopes by
/// `repos.account_id`; the union path drops a PR iff every relation owner has
/// archived it (mirroring [`tracked_view_query`]).
fn tracked_view_row_scope(account_id: Option<i64>) -> (String, Vec<i64>) {
    match account_id {
        Some(id) => (
            "FROM pull_requests pr
             JOIN repos r ON r.id = pr.repo_id
             WHERE r.is_tracked = 1
               AND r.account_id = ?1
               AND pr.state = 'open'"
                .to_string(),
            vec![id],
        ),
        None => (
            "FROM pull_requests pr
             JOIN repos r ON r.id = pr.repo_id
             WHERE r.is_tracked = 1
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
            .to_string(),
            Vec::new(),
        ),
    }
}

/// Row-scope FROM + WHERE for the Archive view. Single-account selects the
/// active account's archived relations; the union path admits a PR iff at
/// least one tracked account has archived it. Mirrors [`archive_view_query`].
fn archive_view_row_scope(account_id: Option<i64>) -> (String, Vec<i64>) {
    match account_id {
        Some(id) => (
            "FROM pull_request_viewer_relations rel
             JOIN pull_requests pr ON pr.id = rel.pull_request_id
             WHERE rel.account_id = ?1
               AND rel.archived_at IS NOT NULL"
                .to_string(),
            vec![id],
        ),
        None => (
            "FROM pull_requests pr
             WHERE EXISTS (
                SELECT 1 FROM pull_request_viewer_relations rel_filter
                 WHERE rel_filter.pull_request_id = pr.id
                   AND rel_filter.archived_at IS NOT NULL
             )"
            .to_string(),
            Vec::new(),
        ),
    }
}

/// Project the five view counts for the active account scope in one SQL
/// round-trip. Each count equals the length of the matching
/// [`list_pull_requests`] call - the row-scope helpers mirror the same view
/// predicates so the chip and the list agree row-for-row.
///
/// The combined SELECT bundles five scalar sub-queries into one prepared
/// statement; the single-account path binds `?1` once (every sub-query reuses
/// the same parameter), the union path binds nothing.
pub fn list_view_counts(
    conn: &Connection,
    account_id: Option<i64>,
) -> Result<DashboardViewCounts, rusqlite::Error> {
    let (authored, params) = view_row_scope_sql(DashboardView::Authored, account_id);
    let (assigned, _) = view_row_scope_sql(DashboardView::Assigned, account_id);
    let (watching, _) = view_row_scope_sql(DashboardView::Watching, account_id);
    let (tracked, _) = view_row_scope_sql(DashboardView::Tracked, account_id);
    let (archive, _) = view_row_scope_sql(DashboardView::Archive, account_id);
    let sql = format!(
        "SELECT
            (SELECT COUNT(*) {authored}) AS authored,
            (SELECT COUNT(*) {assigned}) AS assigned,
            (SELECT COUNT(*) {watching}) AS watching,
            (SELECT COUNT(*) {tracked})  AS tracked,
            (SELECT COUNT(*) {archive})  AS archive"
    );
    conn.query_row(&sql, params_from_iter(params.iter()), |row| {
        Ok(DashboardViewCounts {
            authored: row.get(0)?,
            assigned: row.get(1)?,
            watching: row.get(2)?,
            tracked: row.get(3)?,
            archive: row.get(4)?,
        })
    })
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
    //   (Tracked-view PR with no relations).
    let account_ids_csv: String = row.get(28)?;
    let account_ids = parse_account_ids_csv(&account_ids_csv);
    let account_host: String = row.get(29)?;

    // M4-C: triage projections from `pull_request_viewer_relations rel`.
    // `unread` is computed in SQL via CASE; COALESCE handles missing relation
    // rows (Tracked-view union case) by defaulting to false / 0. See
    // ADR 0015 ("Read-state storage") and `docs/contracts/triage-ux.md`
    // ("Read-state derivation").
    //
    // In the unified path the column carries `MAX(...)` over every relation row
    // the GROUP BY folded together, so the same scalar read works for both
    // shapes. When no relation joined, MAX returns NULL and the outer COALESCE
    // in the SQL pins the default to 0.
    let unread: i64 = row.get(30)?;
    let needs_attention: i64 = row.get(31)?;

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
        // Defaults to `None`; `hydrate_reviewers` recomputes it from the
        // viewer's authoring / request / submitted-review relationship once the
        // reviewer pass has the in-scope identities and per-PR host (ADR 0031).
        my_review_state: MyReviewState::None,
        repo: RepoRef {
            id: repo_id,
            owner: repo_owner,
            name: repo_name,
        },
        account_ids,
        unread: unread != 0,
        needs_attention: needs_attention != 0,
    })
}

/// Parse the comma-separated `account_ids` projection into a sorted vector.
/// Empty input -> empty vector (Tracked-view PR with no relations).
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
    // Every requested-reviewer login per PR, recorded before the pending-entry
    // suppression. `my_review_state` reads this directly: a re-requested review
    // (in `requested_reviewers` even after a prior submitted review) must read
    // as `requested`, which outranks the stale submitted state in the ADR 0031
    // precedence - so the derivation can't reuse the suppressed `Pending` entry.
    let mut requested_logins_by_pr: HashMap<i64, std::collections::HashSet<String>> =
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
            // Record the raw request membership for the my-review-state
            // derivation before any suppression below.
            requested_logins_by_pr
                .entry(pr_id)
                .or_default()
                .insert(login.clone());
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

        if let Some(mut entries) = reviewers_by_pr.remove(&pr.id) {
            for entry in entries.iter_mut() {
                entry.is_you = viewer_logins_on_pr_host
                    .iter()
                    .any(|login| *login == entry.login);
            }
            pr.reviewers = entries;
        }

        // ADR 0031 my-review-state. Precedence (highest wins):
        // author > requested > changes-requested > approved > commented > none.
        // Reads raw data (author_login, requested-reviewer membership, the
        // viewer's own submitted-review entry) rather than the suppressed
        // reviewer list, so a re-requested review still reads as `requested`.
        // Host-gating is inherited from `viewer_logins_on_pr_host`.
        let requested = requested_logins_by_pr.get(&pr.id);
        pr.my_review_state = derive_my_review_state(
            &pr.author_login,
            &viewer_logins_on_pr_host,
            requested,
            &pr.reviewers,
        );
    }
    Ok(())
}

/// Resolve [`MyReviewState`] for one PR from the viewer's authoring /
/// review-request / submitted-review relationship.
///
/// - `viewer_logins_on_pr_host` is the set of the viewer's in-scope logins
///   that sit on the PR's owning host (the host gate). An empty slice means no
///   in-scope identity on the host, so the result is always `None`.
/// - `requested_logins` is every login in `requested_reviewers` for the PR
///   (unsuppressed), used to detect the `requested` obligation directly.
/// - `reviewers` is the hydrated entry list; the viewer's own `is_you` entry
///   carries the latest submitted state (`Approved` / `ChangesRequested` /
///   `Commented`).
///
/// Precedence (highest wins): author > requested > changes-requested >
/// approved > commented > none. A viewer who authored the PR reads `author`
/// even if they also reviewed; a re-requested reviewer reads `requested` even
/// if they have a stale submitted review.
fn derive_my_review_state(
    author_login: &str,
    viewer_logins_on_pr_host: &[&str],
    requested_logins: Option<&std::collections::HashSet<String>>,
    reviewers: &[ReviewerEntry],
) -> MyReviewState {
    if viewer_logins_on_pr_host.contains(&author_login) {
        return MyReviewState::Author;
    }
    let is_requested = requested_logins.is_some_and(|set| {
        viewer_logins_on_pr_host
            .iter()
            .any(|login| set.contains(*login))
    });
    if is_requested {
        return MyReviewState::Requested;
    }
    // The viewer's own submitted-review entry, if any. There is at most one
    // per PR (the hydration dedupes to one row per (PR, login)). A `Pending`
    // entry here is a requested reviewer that wasn't already caught above
    // (no submitted review), which still maps to `Requested`.
    let own = reviewers.iter().find(|entry| entry.is_you);
    match own.map(|entry| entry.state) {
        Some(ReviewerState::Pending) => MyReviewState::Requested,
        Some(ReviewerState::ChangesRequested) => MyReviewState::ChangesRequested,
        Some(ReviewerState::Approved) => MyReviewState::Approved,
        Some(ReviewerState::Commented) => MyReviewState::Commented,
        None => MyReviewState::None,
    }
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
    fn tracked_query_uses_repo_tracked_flag() {
        let (sql, _) = view_query(DashboardView::Tracked, DashboardSort::Updated, Some(1), &[]);
        assert!(
            sql.contains("r.is_tracked = 1"),
            "tracked query missing repo flag; SQL: {sql}"
        );
    }

    /// M4-C: Tracked view LEFT JOINs the relations table so the triage
    /// projections (`unread`, `needs_attention`) reflect the active account.
    /// With no account scope the join short-circuits via `ON 0` so the
    /// COALESCE defaults trip.
    #[test]
    fn tracked_query_left_joins_relations_when_account_scoped() {
        let (sql, _) = view_query(DashboardView::Tracked, DashboardSort::Updated, Some(1), &[]);
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel"),
            "tracked query must LEFT JOIN relations when account-scoped; SQL: {sql}"
        );
        assert!(
            sql.contains("rel.account_id = ?1"),
            "tracked query relation join must filter on account; SQL: {sql}"
        );
    }

    #[test]
    fn tracked_query_union_left_joins_relations_without_account_predicate() {
        // ADR 0016: the union path keeps the LEFT JOIN to feed the merge
        // aggregations, but drops the `rel.account_id = ?1` predicate so every
        // relation row for the PR contributes. PRs in tracked repos without
        // any relation rows still surface (view filter is on
        // `repos.is_tracked = 1`); the merge aggregates over zero relation
        // rows and defaults the triage columns via COALESCE.
        let (sql, params) = view_query(DashboardView::Tracked, DashboardSort::Updated, None, &[]);
        assert!(params.is_empty());
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel\n                     ON rel.pull_request_id = pr.id"),
            "tracked union path must LEFT JOIN relations without an account filter; SQL: {sql}"
        );
        assert!(
            !sql.contains("LEFT JOIN pull_request_viewer_relations rel ON 0"),
            "tracked union path must not short-circuit the relation join; SQL: {sql}"
        );
        assert!(
            sql.contains("GROUP BY pr.id"),
            "tracked union path must dedupe via GROUP BY pr.id; SQL: {sql}"
        );
    }

    #[test]
    fn tracked_query_left_joins_relations_for_needs_me_sort() {
        let (sql, _) = view_query(DashboardView::Tracked, DashboardSort::NeedsMe, Some(1), &[]);
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel"),
            "tracked query missing LEFT JOIN for NeedsMe sort; SQL: {sql}"
        );
        assert!(
            sql.contains("rel.account_id = ?1"),
            "tracked query missing account scope on the LEFT JOIN; SQL: {sql}"
        );
    }

    #[test]
    fn tracked_query_left_joins_relations_for_needs_attention_chip() {
        let (sql, _) = view_query(
            DashboardView::Tracked,
            DashboardSort::Updated,
            Some(1),
            &[ChipKey::NeedsAttention],
        );
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel"),
            "tracked query missing LEFT JOIN for needs-attention chip; SQL: {sql}"
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

    /// M4-C: the SELECT projects the triage columns derived from the
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
            clause.contains("AND (pr.is_draft = 1)"),
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
            sql.contains("AND (pr.is_draft = 1)"),
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

    /// Tracked view's account-scoped path uses `?1` twice for the relation
    /// join. The threads rollup reuses the same parameter so the bound vector
    /// stays length-1.
    #[test]
    fn tracked_query_account_scoped_threads_rollup_reuses_account_parameter() {
        let (sql, params) =
            view_query(DashboardView::Tracked, DashboardSort::Updated, Some(3), &[]);
        assert_eq!(params, vec![3], "single bound i64 even though ?1 reappears");
        assert!(
            sql.contains("a.id = ?1"),
            "threads rollup must scope by ?1; SQL: {sql}"
        );
    }

    // ===== M7 perf: review_comments.author_login index (issue #231) =====

    /// Seed enough review_comments rows that the planner can prefer an
    /// index-driven path over a SCAN. With a tiny `review_comments` (a few
    /// rows) SQLite picks SCAN as the cheaper option regardless of available
    /// indexes; the production DB carries thousands of rows where the planner
    /// flips to SEARCH. Padding the fixture to ~200 rows is enough to mirror
    /// that shape so the test reflects the production plan, not the toy one.
    fn seed_for_explain(conn: &mut Connection) {
        crate::db::migrate::run(conn).unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO accounts (id, label, host, login, created_at) VALUES
                (1, 'alice', 'github.com', 'alice', 0);
            INSERT INTO repos (id, account_id, owner, name, visibility) VALUES
                (10, 1, 'alice', 'web', 'public');
            INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref) VALUES
                (100, 10, 1, 'x', 'open', 'alice', 0, 100, 'main', 'feat');
            INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at) VALUES
                (1, 100, 1, 0, 1, 0);
            INSERT INTO review_threads
                (id, pull_request_id, is_resolved, is_outdated, node_id) VALUES
                (1001, 100, 0, 0, 'RT_1');
            "#,
        )
        .unwrap();
        let tx = conn.transaction().unwrap();
        // ~200 review_comments rows spread across distinct author_logins so
        // the new index is selective enough for the planner to pick it.
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO review_comments
                        (id, review_thread_id, author_login, body, created_at)
                     VALUES (?1, 1001, ?2, 'body', ?3)",
                )
                .unwrap();
            for i in 0..200i64 {
                let login = format!("user_{i}");
                stmt.execute(rusqlite::params![3000 + i, login, i]).unwrap();
            }
        }
        tx.commit().unwrap();
        conn.execute_batch("ANALYZE;").unwrap();
    }

    /// Issue #231 acceptance: with `idx_review_comments_author_login` in place
    /// (migration 0015), the dashboard query's `thread_buckets` involvement
    /// EXISTS clauses must SEARCH `review_comments` via the new index rather
    /// than SCAN the table. SQLite's `EXPLAIN QUERY PLAN` emits a `detail`
    /// column per loop whose text either starts with `SEARCH <name> USING
    /// INDEX <idx>` (index-driven) or `SCAN <name>` (full table walk), where
    /// `<name>` is the alias when one is set. The thread-buckets subquery
    /// aliases `review_comments` as `c`, so the negative guard checks no plan
    /// row equals `SCAN c` and the positive one checks at least one row
    /// references `idx_review_comments_author_login`.
    fn assert_review_comments_uses_author_index(conn: &Connection, sql: &str, params: &[i64]) {
        let explain = format!("EXPLAIN QUERY PLAN {sql}");
        let mut stmt = conn.prepare(&explain).unwrap();
        let plan: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let full_plan = plan.join("\n");
        for detail in &plan {
            // `SCAN c` is the alias-driven full scan that the index must
            // replace; `SCAN review_comments` is the fallback shape if the
            // alias ever gets dropped. Either form fails the guard.
            assert!(
                detail.trim() != "SCAN c" && !detail.contains("SCAN review_comments"),
                "dashboard query must not full-scan review_comments after \
                 migration 0015; plan row: {detail}\nFull plan:\n{full_plan}",
            );
        }
        assert!(
            full_plan.contains("idx_review_comments_author_login"),
            "expected at least one plan row to drive review_comments via \
             idx_review_comments_author_login; full plan:\n{full_plan}",
        );
    }

    #[test]
    fn dashboard_query_does_not_scan_review_comments_under_single_account_scope() {
        let mut conn = Connection::open_in_memory().unwrap();
        seed_for_explain(&mut conn);
        let (sql, params) = view_query(
            DashboardView::Watching,
            DashboardSort::Updated,
            Some(1),
            &[],
        );
        assert_review_comments_uses_author_index(&conn, &sql, &params);
    }

    #[test]
    fn dashboard_query_does_not_scan_review_comments_under_union_scope() {
        let mut conn = Connection::open_in_memory().unwrap();
        seed_for_explain(&mut conn);
        let (sql, params) = view_query(DashboardView::Watching, DashboardSort::Updated, None, &[]);
        assert_review_comments_uses_author_index(&conn, &sql, &params);
    }
}
