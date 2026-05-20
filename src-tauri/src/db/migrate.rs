//! Migration runner for the local SQLite cache.
//!
//! Migrations are forward-only and embedded into the binary via `include_str!`.
//! Add a new migration by creating `src-tauri/migrations/NNNN_*.sql` and listing
//! it in [`MIGRATION_SOURCES`].

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

/// SQL text for each migration, in apply order. Each entry corresponds to a
/// file under `src-tauri/migrations/`.
const MIGRATION_SOURCES: &[&str] = &[
    include_str!("../../migrations/0001_init.sql"),
    include_str!("../../migrations/0002_dashboard_fields.sql"),
    include_str!("../../migrations/0003_accounts_expires_at.sql"),
    include_str!("../../migrations/0004_conversation_depth.sql"),
    include_str!("../../migrations/0005_threads_breakdown.sql"),
    include_str!("../../migrations/0006_users_table.sql"),
];

/// Build the migration set. The underlying `Migrations` is cheap to construct
/// per call and validates lazily.
pub fn migrations() -> Migrations<'static> {
    Migrations::new(MIGRATION_SOURCES.iter().map(|sql| M::up(sql)).collect())
}

/// Apply the canonical pragmas on a freshly-opened connection.
///
/// WAL keeps the single-writer/multi-reader pattern (sync worker writes, UI
/// reads) crash-safe. Foreign keys are enforced per-connection in SQLite.
pub fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    // `journal_mode` returns the new mode as a row; discard it.
    let _: String = conn.query_row("PRAGMA journal_mode = WAL;", [], |row| row.get(0))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

/// Apply pragmas and run all pending migrations on `conn`.
pub fn run(conn: &mut Connection) -> Result<(), rusqlite_migration::Error> {
    apply_pragmas(conn).map_err(|err| rusqlite_migration::Error::RusqliteError {
        query: "PRAGMA setup".to_string(),
        err,
    })?;
    migrations().to_latest(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        run(&mut conn).expect("run migrations");
        conn
    }

    #[test]
    fn migration_set_validates() {
        migrations().validate().expect("migrations validate");
    }

    #[test]
    fn migrations_to_latest_on_fresh_db() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, MIGRATION_SOURCES.len() as i64);
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        // Second call is a no-op when the schema is already at latest.
        run(&mut conn).unwrap();
    }

    #[test]
    fn foreign_keys_pragma_is_on() {
        let conn = fresh();
        let fk: i64 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys must be ON");
    }

    #[test]
    fn expected_tables_exist() {
        let conn = fresh();
        let expected = [
            "accounts",
            "repos",
            "pull_requests",
            "reviews",
            "review_threads",
            "review_comments",
            "issue_comments",
            "timeline_events",
            "check_runs",
            "etags",
            "requested_reviewers",
            "pull_request_viewer_relations",
            "users",
        ];
        for name in expected {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing table: {name}");
        }
    }

    #[test]
    fn expected_indexes_exist() {
        let conn = fresh();
        let expected = [
            "idx_pull_requests_repo_state",
            "idx_pull_requests_author_state",
            "idx_pull_requests_latest_status_change_at",
            "idx_review_threads_pr_resolved",
            "idx_requested_reviewers_pr",
            "idx_pull_request_viewer_relations_account_authored",
            "idx_pull_request_viewer_relations_account_review_requested",
            "idx_pull_request_viewer_relations_account_involved",
            // 0004 conversation_depth.
            "idx_review_threads_node_id",
            "idx_review_threads_pr_active",
            "idx_review_comments_node_id",
            "idx_review_comments_thread",
            "idx_issue_comments_node_id",
            "idx_issue_comments_pr",
            "idx_reviews_node_id",
            "idx_reviews_pr_submitted_at",
        ];
        for name in expected {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing index: {name}");
        }
    }

    #[test]
    fn foreign_keys_cascade_on_delete() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 'me', 0, 0, 'main', 'feat');",
        )
        .unwrap();

        conn.execute("DELETE FROM accounts WHERE id = 1", [])
            .unwrap();

        let repos: i64 = conn
            .query_row("SELECT COUNT(*) FROM repos", [], |row| row.get(0))
            .unwrap();
        let prs: i64 = conn
            .query_row("SELECT COUNT(*) FROM pull_requests", [], |row| row.get(0))
            .unwrap();
        assert_eq!(repos, 0, "repos should cascade from accounts");
        assert_eq!(prs, 0, "pull_requests should cascade from repos");
    }

    #[test]
    fn dashboard_columns_exist_on_pull_requests() {
        let conn = fresh();
        let expected = [
            "mergeable",
            "review_decision",
            "additions",
            "deletions",
            "changed_files",
            "ci_state",
            "ci_total",
            "ci_passing",
        ];
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('pull_requests')")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for col in expected {
            assert!(
                names.iter().any(|n| n == col),
                "missing pull_requests column: {col}"
            );
        }
    }

    #[test]
    fn conversation_columns_exist_on_review_threads() {
        let conn = fresh();
        let expected = [
            "node_id",
            "is_outdated",
            "created_at",
            "resolved_at",
            "last_reply_at",
            "reply_count",
            "head_comment_author_login",
            "head_comment_body_text",
            "head_comment_created_at",
            "line",
            "start_line",
        ];
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('review_threads')")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for col in expected {
            assert!(
                names.iter().any(|n| n == col),
                "missing review_threads column: {col}"
            );
        }
    }

    #[test]
    fn conversation_rollup_columns_exist_on_pull_requests() {
        let conn = fresh();
        let expected = [
            "threads_total",
            "threads_unresolved_involved",
            "threads_unresolved_uninvolved",
            "threads_resolved_involved",
            "threads_resolved_uninvolved",
            "issue_comments_count",
        ];
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('pull_requests')")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for col in expected {
            assert!(
                names.iter().any(|n| n == col),
                "missing pull_requests column: {col}"
            );
        }
    }

    #[test]
    fn retired_threads_rollup_columns_are_gone() {
        // 0005 drops the v4 `threads_unresolved` and `threads_involved`
        // columns. Assert they no longer exist so a regression that resurrects
        // them under a stale migration doesn't slip past CI.
        let conn = fresh();
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('pull_requests')")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for retired in ["threads_unresolved", "threads_involved"] {
            assert!(
                !names.iter().any(|n| n == retired),
                "retired column resurrected: {retired}"
            );
        }
    }

    #[test]
    fn threads_rollup_defaults_to_zero() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 'me', 0, 0, 'main', 'feat');",
        )
        .unwrap();

        let (total, ui, uu, ri, ru, issue_count): (i64, i64, i64, i64, i64, i64) = conn
            .query_row(
                "SELECT threads_total,
                        threads_unresolved_involved,
                        threads_unresolved_uninvolved,
                        threads_resolved_involved,
                        threads_resolved_uninvolved,
                        issue_comments_count
                   FROM pull_requests WHERE id = 100",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(total, 0);
        assert_eq!(ui, 0);
        assert_eq!(uu, 0);
        assert_eq!(ri, 0);
        assert_eq!(ru, 0);
        assert_eq!(issue_count, 0);
    }

    #[test]
    fn is_team_tracked_defaults_to_zero() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');",
        )
        .unwrap();
        let tracked: i64 = conn
            .query_row("SELECT is_team_tracked FROM repos WHERE id = 10", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tracked, 0);
    }

    #[test]
    fn viewer_relations_cascade_on_account_delete() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 'me', 0, 0, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, last_seen_at)
                VALUES (1, 100, 1, 0, 1, 0);",
        )
        .unwrap();

        conn.execute("DELETE FROM accounts WHERE id = 1", [])
            .unwrap();

        let relations: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pull_request_viewer_relations",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(relations, 0, "relations should cascade from accounts");
    }
}
