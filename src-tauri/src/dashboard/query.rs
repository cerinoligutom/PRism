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
//! `account_id = None` returns the union across every account. For the three
//! relation-backed views that means one row per (account, PR) the relation
//! exists for: a PR authored by account A and review-requested for account B
//! shows up once under each. For Team it means every team-tracked repo,
//! grouped by the repo's owning account.

use std::collections::HashMap;

use rusqlite::{params_from_iter, Connection, Row};

use crate::dashboard::types::{
    CiSummary, DashboardPullRequest, DashboardSort, DashboardView, RepoRef, ReviewerEntry,
    ReviewerState, ThreadsSummary,
};
use crate::triage::query as triage_query;
use crate::triage::types::ChipKey;

/// SQL fragment that selects every column the row projection needs, joined to
/// `repos` and `accounts`. Each view prepends its own FROM clause to this body.
///
/// The trailing `rel.*` projections (M4-C) read the active account's triage
/// state. Relation-backed views (Authored / Assigned / Watching) already JOIN
/// `pull_request_viewer_relations rel` so the columns flow directly. The Team
/// view adds a LEFT JOIN on the same alias - scoped to the active account when
/// one is provided, an inert `ON 0` join otherwise - so the SELECT keeps a
/// stable shape across every view. See `docs/contracts/triage-ux.md`
/// ("Read-state derivation") and ADR 0015.
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
    pr.threads_total,
    pr.threads_unresolved_involved,
    pr.threads_unresolved_uninvolved,
    pr.threads_resolved_involved,
    pr.threads_resolved_uninvolved,
    r.id,
    r.owner,
    r.name,
    a.id,
    a.host,
    CASE
        WHEN rel.read_at IS NULL THEN 1
        WHEN pr.updated_at > COALESCE(rel.read_pr_updated_at, 0) THEN 1
        ELSE 0
    END AS unread,
    COALESCE(rel.needs_attention, 0) AS needs_attention,
    COALESCE(rel.mentioned_count_unread, 0) AS mentioned_count_unread
";

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
fn build_sql(from_and_where: &str, chip_clause: &str, sort: DashboardSort) -> String {
    let order_by = match sort {
        DashboardSort::Updated => {
            "ORDER BY COALESCE(pr.latest_status_change_at, pr.updated_at) DESC, pr.id DESC"
        }
        DashboardSort::Stale => "ORDER BY pr.updated_at ASC, pr.id DESC",
        DashboardSort::NeedsMe => {
            // `COALESCE` keeps the column NULL-safe for the Team view path,
            // where the LEFT JOIN against `pull_request_viewer_relations`
            // misses when the active account has no relation row.
            "ORDER BY COALESCE(rel.needs_attention, 0) DESC, \
                      COALESCE(pr.latest_status_change_at, pr.updated_at) DESC, \
                      pr.id DESC"
        }
    };
    format!(
        "SELECT {PR_PROJECTION_COLUMNS}
         {from_and_where}
         {chip_clause}
         {order_by}"
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
    }
}

/// Build the SQL for the three relation-backed views (Authored / Assigned /
/// Watching). `flag_column` must be one of `is_authored`,
/// `is_review_requested`, `is_involved`. Never user-supplied, so safe to
/// interpolate.
fn relation_view_query(
    flag_column: &str,
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> (String, Vec<i64>) {
    let mut from_and_where = format!(
        "FROM pull_request_viewer_relations rel
         JOIN pull_requests pr ON pr.id = rel.pull_request_id
         JOIN repos r ON r.id = pr.repo_id
         JOIN accounts a ON a.id = rel.account_id
         LEFT JOIN users author_u ON author_u.login = pr.author_login
         WHERE rel.{flag_column} = 1"
    );
    let mut params: Vec<i64> = Vec::new();
    if let Some(id) = account_id {
        from_and_where.push_str(" AND rel.account_id = ?1");
        params.push(id);
    }
    let chip_clause = chip_where_clause(active_chips);
    (build_sql(&from_and_where, &chip_clause, sort), params)
}

/// Team view: PRs in repos opted into team tracking. The relation row is read
/// account-scoped via a LEFT JOIN so the triage projections (`unread`,
/// `needs_attention`, `mentioned_count_unread`) reflect the active account.
/// Without an account filter (the union case) the join short-circuits via
/// `ON 0` so the SQL keeps a stable shape and the COALESCE defaults trip.
/// The LEFT JOIN is unconditional because `PR_PROJECTION_COLUMNS` reads
/// `rel.*` for every row; the M4-D chip / sort gating folded into this path
/// because the projection requires `rel` in scope regardless.
fn team_view_query(
    sort: DashboardSort,
    account_id: Option<i64>,
    active_chips: &[ChipKey],
) -> (String, Vec<i64>) {
    let mut from_and_where = String::from(
        "FROM pull_requests pr
         JOIN repos r ON r.id = pr.repo_id
         JOIN accounts a ON a.id = r.account_id
         LEFT JOIN users author_u ON author_u.login = pr.author_login",
    );
    let mut params: Vec<i64> = Vec::new();
    if let Some(id) = account_id {
        from_and_where.push_str(
            " LEFT JOIN pull_request_viewer_relations rel
                 ON rel.pull_request_id = pr.id AND rel.account_id = ?1
              WHERE r.is_team_tracked = 1
                AND r.account_id = ?1",
        );
        params.push(id);
    } else {
        from_and_where.push_str(
            " LEFT JOIN pull_request_viewer_relations rel ON 0
              WHERE r.is_team_tracked = 1",
        );
    }
    let chip_clause = chip_where_clause(active_chips);
    (build_sql(&from_and_where, &chip_clause, sort), params)
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

    // The migration sets the threads_* columns to `NOT NULL DEFAULT 0`, so a
    // freshly-discovered PR before its first enrichment reads as zeros across
    // the board. Emit `None` for that case so the frontend can render the
    // muted em-dash state (per the contract's "Dashboard rollup" section)
    // rather than an all-zeros summary.
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
    let account_id: i64 = row.get(28)?;
    let account_host: String = row.get(29)?;

    // M4-C: triage projections from `pull_request_viewer_relations rel`.
    // `unread` is computed in SQL via CASE; COALESCE handles missing relation
    // rows (Team-view union case) by defaulting to false / 0. See ADR 0015
    // ("Read-state storage") and `docs/contracts/triage-ux.md`
    // ("Read-state derivation").
    let unread: i64 = row.get(30)?;
    let needs_attention: i64 = row.get(31)?;
    let mentioned_count_unread: i64 = row.get(32)?;

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
        account_id,
        unread: unread != 0,
        needs_attention: needs_attention != 0,
        mentioned_count_unread,
    })
}

/// Reviewer entries for one PR: one row per unique reviewer login, picking the
/// latest submitted review per login (with a state-priority tie-break), then
/// filling in pending entries for requested reviewers who have never
/// submitted. The viewer marker `is_you` is true when the reviewer login
/// matches the PR's owning-account login. Rows whose latest state doesn't map
/// to a [`ReviewerState`] (e.g. `DISMISSED`) are dropped; a login dropped this
/// way does not re-appear as `Pending` from `requested_reviewers` because the
/// login still counts as having a submitted review.
fn hydrate_reviewers(
    conn: &Connection,
    prs: &mut [DashboardPullRequest],
) -> Result<(), rusqlite::Error> {
    // Build the account-login lookup keyed on the row's projected account_id.
    let mut account_logins: HashMap<i64, String> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT id, login FROM accounts")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let login: String = row.get(1)?;
            account_logins.insert(id, login);
        }
    }

    // Bucket the PR ids so we can fetch reviewers in one round trip per source
    // table (reviews + requested_reviewers).
    let pr_ids: Vec<i64> = prs.iter().map(|pr| pr.id).collect();
    let placeholders = vec!["?"; pr_ids.len()].join(",");

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

    // Attach to the parent PR rows and compute `is_you`.
    for pr in prs.iter_mut() {
        if let Some(mut entries) = reviewers_by_pr.remove(&pr.id) {
            if let Some(viewer_login) = account_logins.get(&pr.account_id) {
                for entry in entries.iter_mut() {
                    entry.is_you = entry.login == *viewer_login;
                }
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
    fn team_query_short_circuits_relation_join_without_account() {
        let (sql, params) = view_query(DashboardView::Team, DashboardSort::Updated, None, &[]);
        assert!(params.is_empty());
        assert!(
            sql.contains("LEFT JOIN pull_request_viewer_relations rel ON 0"),
            "team query without account must short-circuit the relation join; SQL: {sql}"
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
}
