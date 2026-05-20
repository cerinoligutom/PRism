//! SQL helpers for the `repos` table.
//!
//! The `repos.is_team_tracked` flag drives the Team view (M2 dashboard). The
//! upsert preserves it across refreshes — re-discovering a repo from GitHub
//! must never reset the user's opt-in state.

use rusqlite::{params, Connection};

use crate::github::rest::RepoNode;
use crate::repos::types::RepoSummary;

/// Read every `repos` row for an account, ordered for stable display.
pub fn list_for_account(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<RepoSummary>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, account_id, owner, name, visibility, is_team_tracked
         FROM repos
         WHERE account_id = ?1
         ORDER BY owner COLLATE NOCASE, name COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map(params![account_id], row_to_summary)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Flip the `is_team_tracked` flag for one repo. Returns the number of rows
/// affected so callers can surface a not-found error if the id is stale.
pub fn set_team_tracked(
    conn: &Connection,
    repo_id: i64,
    tracked: bool,
) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "UPDATE repos SET is_team_tracked = ?1 WHERE id = ?2",
        params![tracked as i64, repo_id],
    )
}

/// Upsert a batch of repos for an account, preserving each repo's existing
/// `is_team_tracked` flag.
///
/// Conflicts are resolved on the `(account_id, owner, name)` unique constraint
/// so two accounts pointing at the same GitHub repo own their opt-in state
/// independently. The autoincrement `repos.id` stays stable across upserts.
///
/// Returns the resulting summaries (ordered the same as `list_for_account`)
/// so the caller can return the post-write state without a second query.
pub fn upsert_for_account(
    conn: &mut Connection,
    account_id: i64,
    repos: &[RepoNode],
) -> Result<Vec<RepoSummary>, rusqlite::Error> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO repos (account_id, owner, name, visibility)
                 VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, owner, name) DO UPDATE SET
                 visibility = excluded.visibility",
        )?;
        for repo in repos {
            stmt.execute(params![
                account_id,
                repo.owner.login,
                repo.name,
                repo.visibility,
            ])?;
        }
    }
    tx.commit()?;
    list_for_account(conn, account_id)
}

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepoSummary> {
    Ok(RepoSummary {
        id: row.get(0)?,
        account_id: row.get(1)?,
        owner: row.get(2)?,
        name: row.get(3)?,
        visibility: row.get(4)?,
        is_team_tracked: row.get::<_, i64>(5)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::migrate;
    use crate::github::rest::RepoOwner;

    fn fresh_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate::run(&mut conn).unwrap();
        // Seed an account so the foreign key holds.
        conn.execute(
            "INSERT INTO accounts (id, label, host, login, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![1, "Work", "github.com", "ada", 0i64],
        )
        .unwrap();
        conn
    }

    fn repo_node(id: i64, owner: &str, name: &str, visibility: &str) -> RepoNode {
        RepoNode {
            id,
            name: name.into(),
            owner: RepoOwner {
                login: owner.into(),
            },
            visibility: visibility.into(),
        }
    }

    #[test]
    fn upsert_inserts_new_repo() {
        let mut conn = fresh_conn();
        let inserted = upsert_for_account(
            &mut conn,
            1,
            &[repo_node(100, "sitemate", "web", "private")],
        )
        .unwrap();

        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].owner, "sitemate");
        assert_eq!(inserted[0].name, "web");
        assert_eq!(inserted[0].visibility, "private");
        assert!(!inserted[0].is_team_tracked);
    }

    #[test]
    fn upsert_preserves_is_team_tracked_across_refreshes() {
        // Discovering a repo a second time must never reset the user's opt-in.
        let mut conn = fresh_conn();
        let inserted = upsert_for_account(
            &mut conn,
            1,
            &[repo_node(100, "sitemate", "web", "private")],
        )
        .unwrap();
        let local_id = inserted[0].id;
        let affected = set_team_tracked(&conn, local_id, true).unwrap();
        assert_eq!(affected, 1);

        // Same repo, slightly different visibility (e.g. user made it public).
        let after =
            upsert_for_account(&mut conn, 1, &[repo_node(100, "sitemate", "web", "public")])
                .unwrap();
        assert_eq!(after.len(), 1);
        assert!(after[0].is_team_tracked, "team-tracked flag must survive");
        assert_eq!(after[0].visibility, "public");
        assert_eq!(after[0].id, local_id, "local repo id must be stable");
    }

    #[test]
    fn upsert_does_not_delete_repos_missing_from_the_payload() {
        // A repo that disappears from GitHub's response must stay in the DB
        // for now: the user may still want to opt it out of Team view
        // explicitly. Pruning is a separate (out-of-scope) concern.
        let mut conn = fresh_conn();
        upsert_for_account(
            &mut conn,
            1,
            &[
                repo_node(1, "ada", "alpha", "public"),
                repo_node(2, "ada", "beta", "public"),
            ],
        )
        .unwrap();
        let after =
            upsert_for_account(&mut conn, 1, &[repo_node(1, "ada", "alpha", "public")]).unwrap();
        assert_eq!(after.len(), 2, "beta must still be present");
    }

    #[test]
    fn list_for_account_orders_by_owner_then_name() {
        let mut conn = fresh_conn();
        upsert_for_account(
            &mut conn,
            1,
            &[
                repo_node(3, "zoe", "alpha", "public"),
                repo_node(1, "ada", "beta", "public"),
                repo_node(2, "ada", "alpha", "public"),
            ],
        )
        .unwrap();
        let list = list_for_account(&conn, 1).unwrap();
        let names: Vec<(&str, &str)> = list
            .iter()
            .map(|r| (r.owner.as_str(), r.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![("ada", "alpha"), ("ada", "beta"), ("zoe", "alpha")]
        );
    }

    #[test]
    fn set_team_tracked_returns_zero_for_unknown_repo() {
        let conn = fresh_conn();
        let affected = set_team_tracked(&conn, 999, true).unwrap();
        assert_eq!(affected, 0);
    }

    #[test]
    fn upsert_handles_empty_input() {
        let mut conn = fresh_conn();
        let got = upsert_for_account(&mut conn, 1, &[]).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn list_for_account_returns_empty_when_no_repos() {
        let conn = fresh_conn();
        let got = list_for_account(&conn, 1).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn list_for_account_filters_by_account_id() {
        let mut conn = fresh_conn();
        // Add a second account so we can prove the filter works.
        conn.execute(
            "INSERT INTO accounts (id, label, host, login, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![2, "Personal", "github.com", "bob", 0i64],
        )
        .unwrap();
        upsert_for_account(&mut conn, 1, &[repo_node(10, "ada", "alpha", "public")]).unwrap();
        upsert_for_account(&mut conn, 2, &[repo_node(20, "bob", "beta", "public")]).unwrap();

        let one = list_for_account(&conn, 1).unwrap();
        let two = list_for_account(&conn, 2).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].owner, "ada");
        assert_eq!(one[0].name, "alpha");
        assert_eq!(two.len(), 1);
        assert_eq!(two[0].owner, "bob");
        assert_eq!(two[0].name, "beta");
    }

    #[test]
    fn two_accounts_sharing_one_github_repo_get_independent_rows() {
        // Multi-account scenario: account A and account B both have access to
        // the same GitHub repo. They must own their `is_team_tracked` flag
        // independently — opting in from A must not toggle B.
        let mut conn = fresh_conn();
        conn.execute(
            "INSERT INTO accounts (id, label, host, login, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![2, "Personal", "github.com", "bob", 0i64],
        )
        .unwrap();
        let a =
            upsert_for_account(&mut conn, 1, &[repo_node(42, "shared", "repo", "public")]).unwrap();
        let b =
            upsert_for_account(&mut conn, 2, &[repo_node(42, "shared", "repo", "public")]).unwrap();
        assert_ne!(a[0].id, b[0].id, "each account must own a distinct row");

        set_team_tracked(&conn, a[0].id, true).unwrap();
        let a_after = list_for_account(&conn, 1).unwrap();
        let b_after = list_for_account(&conn, 2).unwrap();
        assert!(a_after[0].is_team_tracked);
        assert!(!b_after[0].is_team_tracked);
    }
}
