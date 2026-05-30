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
    include_str!("../../migrations/0007_review_thread_url.sql"),
    include_str!("../../migrations/0008_comment_urls.sql"),
    include_str!("../../migrations/0009_comment_body_html.sql"),
    include_str!("../../migrations/0010_triage_state.sql"),
    include_str!("../../migrations/0011_review_url.sql"),
    include_str!("../../migrations/0012_archive_and_settings.sql"),
    include_str!("../../migrations/0013_rename_team_tracked.sql"),
    include_str!("../../migrations/0014_diff_hunk.sql"),
    include_str!("../../migrations/0015_index_review_comments_author_login.sql"),
    include_str!("../../migrations/0016_rename_pull_requests_draft.sql"),
    include_str!("../../migrations/0017_rename_relation_last_seen_at.sql"),
    include_str!("../../migrations/0018_last_seen_version.sql"),
    include_str!("../../migrations/0019_auto_update_settings.sql"),
    include_str!("../../migrations/0020_auto_archive_days.sql"),
    include_str!("../../migrations/0021_notifications.sql"),
    include_str!("../../migrations/0022_notifications_read_at.sql"),
    include_str!("../../migrations/0023_notification_retention.sql"),
    include_str!("../../migrations/0024_drop_review_thread_head_denorm.sql"),
    include_str!("../../migrations/0025_conversation_unit_read_state.sql"),
];

/// Migration filenames in apply order, kept in lockstep with
/// [`MIGRATION_SOURCES`]. `include_str!` discards the path, so the sequence-
/// integrity test reads the `NNNN_` prefixes from here instead. A length
/// mismatch between the two lists fails the test, catching an entry added to
/// one but not the other.
#[cfg(test)]
const MIGRATION_FILENAMES: &[&str] = &[
    "0001_init.sql",
    "0002_dashboard_fields.sql",
    "0003_accounts_expires_at.sql",
    "0004_conversation_depth.sql",
    "0005_threads_breakdown.sql",
    "0006_users_table.sql",
    "0007_review_thread_url.sql",
    "0008_comment_urls.sql",
    "0009_comment_body_html.sql",
    "0010_triage_state.sql",
    "0011_review_url.sql",
    "0012_archive_and_settings.sql",
    "0013_rename_team_tracked.sql",
    "0014_diff_hunk.sql",
    "0015_index_review_comments_author_login.sql",
    "0016_rename_pull_requests_draft.sql",
    "0017_rename_relation_last_seen_at.sql",
    "0018_last_seen_version.sql",
    "0019_auto_update_settings.sql",
    "0020_auto_archive_days.sql",
    "0021_notifications.sql",
    "0022_notifications_read_at.sql",
    "0023_notification_retention.sql",
    "0024_drop_review_thread_head_denorm.sql",
    "0025_conversation_unit_read_state.sql",
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
            // 0012 archive + settings foundation.
            "app_settings",
            // 0021 persistent notifications inbox.
            "notifications",
            // 0025 conversation-unit read-state (ADR 0031, issue #431).
            "thread_read_state",
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
            // 0010 triage_state.
            "idx_pr_viewer_relations_attention",
            // 0012 archive + settings.
            "idx_relations_archived_at",
            // 0015 dashboard thread_buckets involvement (ADR 0016, issue #231).
            "idx_review_comments_author_login",
            // 0021 persistent notifications inbox (issue #378).
            "idx_notifications_created_at",
            // 0022 read/unread state (issue #379).
            "idx_notifications_unread",
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
            "line",
            "start_line",
            "url",
            "diff_hunk",
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
    fn migration_0024_drops_review_thread_head_denorm_columns() {
        let conn = fresh();
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('review_threads')")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for dropped in [
            "head_comment_author_login",
            "head_comment_body_text",
            "head_comment_created_at",
        ] {
            assert!(
                !names.iter().any(|n| n == dropped),
                "review_threads still carries dropped column: {dropped}"
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
    fn comment_url_columns_exist_after_migration_0008() {
        // Issue #115: the thread permalink is derived from the head comment's
        // url, and per-comment "Open in GitHub" actions read these columns.
        let conn = fresh();
        for (table, column) in [("review_comments", "url"), ("issue_comments", "url")] {
            let sql = format!("SELECT name FROM pragma_table_info('{table}')");
            let mut stmt = conn.prepare(&sql).unwrap();
            let names: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert!(
                names.iter().any(|n| n == column),
                "missing {table}.{column} column"
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
    fn is_tracked_defaults_to_zero() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');",
        )
        .unwrap();
        let tracked: i64 = conn
            .query_row("SELECT is_tracked FROM repos WHERE id = 10", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tracked, 0);
    }

    /// Migration 0013 renames `is_team_tracked` to `is_tracked`. SQLite's
    /// `ALTER TABLE ... RENAME COLUMN` is in-place, so any rows opted in
    /// before the rename keep their flag under the new name.
    #[test]
    fn rename_team_tracked_to_tracked_preserves_row_data() {
        // Replay the migrations up to but not including 0013, write a row
        // with `is_team_tracked = 1`, then run the rename and read back
        // through `is_tracked` to prove the bit survived. Index 12 is
        // 0013_rename_team_tracked.sql (entries are zero-indexed, NNNN
        // numbers start at 0001) so `take(12)` lands every migration up
        // through 0012 and stops before the rename.
        const PRE_RENAME_PREFIX: usize = 12;
        let mut conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();
        let pre_rename = Migrations::new(
            MIGRATION_SOURCES
                .iter()
                .take(PRE_RENAME_PREFIX)
                .map(|sql| M::up(sql))
                .collect(),
        );
        pre_rename.to_latest(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility, is_team_tracked)
                VALUES (10, 1, 'owner', 'repo', 'public', 1);",
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let tracked: i64 = conn
            .query_row("SELECT is_tracked FROM repos WHERE id = 10", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(tracked, 1, "opt-in must survive the column rename");
    }

    /// Migration 0016 renames `pull_requests.draft` to `pull_requests.is_draft`.
    /// SQLite's `ALTER TABLE ... RENAME COLUMN` is in-place, so any PR row
    /// marked draft before the rename must keep the flag under the new name.
    #[test]
    fn rename_draft_to_is_draft_preserves_row_data() {
        // 0016 sits at zero-index 15 (NNNN numbers start at 0001), so
        // `take(15)` lands every migration up through 0015 and stops before
        // the rename.
        const PRE_RENAME_PREFIX: usize = 15;
        let mut conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();
        let pre_rename = Migrations::new(
            MIGRATION_SOURCES
                .iter()
                .take(PRE_RENAME_PREFIX)
                .map(|sql| M::up(sql))
                .collect(),
        );
        pre_rename.to_latest(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 1, 'me', 0, 0, 'main', 'feat');",
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let is_draft: i64 = conn
            .query_row(
                "SELECT is_draft FROM pull_requests WHERE id = 100",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(is_draft, 1, "draft flag must survive the column rename");
    }

    /// Migration 0017 renames `pull_request_viewer_relations.last_seen_at`
    /// to `relation_observed_at`. Replays migrations up through 0016,
    /// seeds a relation with the old column name, runs 0017, and reads
    /// back via the new name to prove the timestamp survived.
    #[test]
    fn rename_relation_last_seen_at_preserves_row_data() {
        // 0017 sits at zero-index 16 (NNNN numbers start at 0001).
        const PRE_RENAME_PREFIX: usize = 16;
        let mut conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();
        let pre_rename = Migrations::new(
            MIGRATION_SOURCES
                .iter()
                .take(PRE_RENAME_PREFIX)
                .map(|sql| M::up(sql))
                .collect(),
        );
        pre_rename.to_latest(&mut conn).unwrap();
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
                VALUES (1, 100, 1, 0, 0, 12345);",
        )
        .unwrap();
        migrations().to_latest(&mut conn).unwrap();
        let observed_at: i64 = conn
            .query_row(
                "SELECT relation_observed_at FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(observed_at, 12345, "timestamp must survive the rename");
    }

    /// Migration 0015 adds an index over `review_comments.author_login`. It
    /// must apply cleanly on top of an existing post-M6 schema that already
    /// holds review_comments rows (the same shape every installed binary
    /// will see when it upgrades). Replays up through 0014, seeds two
    /// comments, runs the migration to latest, and reads back via the new
    /// index to prove both the CREATE INDEX and the underlying rows survive.
    #[test]
    fn index_review_comments_author_login_applies_against_post_m6_schema() {
        // 0015 sits at zero-index 14 (NNNN numbers start at 0001), so
        // `take(14)` lands every migration up through 0014.
        const PRE_INDEX_PREFIX: usize = 14;
        let mut conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();
        let pre_index = Migrations::new(
            MIGRATION_SOURCES
                .iter()
                .take(PRE_INDEX_PREFIX)
                .map(|sql| M::up(sql))
                .collect(),
        );
        pre_index.to_latest(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 'me', 0, 0, 'main', 'feat');
             INSERT INTO review_threads (id, pull_request_id, is_resolved, node_id)
                VALUES (1001, 100, 0, 'RT_1');
             INSERT INTO review_comments
                (id, review_thread_id, author_login, body, created_at)
                VALUES (2001, 1001, 'alice', 'a', 1),
                       (2002, 1001, 'bob',   'b', 2);",
        )
        .unwrap();

        migrations().to_latest(&mut conn).unwrap();

        let index_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type = 'index' AND name = 'idx_review_comments_author_login'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index_exists, 1, "0015 must create the new index");

        let alice_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM review_comments WHERE author_login = 'alice'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alice_count, 1, "existing rows must remain readable");
    }

    #[test]
    fn triage_columns_exist_on_pull_request_viewer_relations() {
        let conn = fresh();
        let expected = [
            "read_at",
            "read_pr_updated_at",
            "mentioned_count_unread",
            "mention_scan_watermark_at",
            "needs_attention",
        ];
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('pull_request_viewer_relations')")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for col in expected {
            assert!(
                names.iter().any(|n| n == col),
                "missing pull_request_viewer_relations column: {col}"
            );
        }
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
                 is_involved, relation_observed_at)
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

    #[test]
    fn archive_column_exists_on_pull_request_viewer_relations() {
        let conn = fresh();
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('pull_request_viewer_relations')")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            names.iter().any(|n| n == "archived_at"),
            "missing pull_request_viewer_relations.archived_at column"
        );
    }

    #[test]
    fn app_settings_singleton_seeded_with_one_row() {
        let conn = fresh();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM app_settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "app_settings must hold exactly one row after migration"
        );
        let id: i64 = conn
            .query_row("SELECT id FROM app_settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(id, 1, "the seeded singleton must be keyed id = 1");
    }

    #[test]
    fn auto_archive_days_column_defaults_to_thirty() {
        // Migration 0020 (issue #333) adds `auto_archive_days` with a default
        // of 30 to preserve ADR-0018's behaviour on every existing install.
        let conn = fresh();
        let days: i64 = conn
            .query_row(
                "SELECT auto_archive_days FROM app_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(days, 30);
    }

    #[test]
    fn auto_archive_days_check_rejects_negative_value() {
        // The migration adds `CHECK (auto_archive_days BETWEEN 0 AND 365)` so
        // a write that bypasses the writer's clamp can't smuggle a negative
        // window into the column.
        let conn = fresh();
        let err = conn
            .execute(
                "UPDATE app_settings SET auto_archive_days = -1 WHERE id = 1",
                [],
            )
            .expect_err("CHECK must reject -1");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("check") || msg.contains("constraint"),
            "expected CHECK failure, got: {err}"
        );
    }

    #[test]
    fn auto_archive_days_check_rejects_over_cap_value() {
        let conn = fresh();
        let err = conn
            .execute(
                "UPDATE app_settings SET auto_archive_days = 366 WHERE id = 1",
                [],
            )
            .expect_err("CHECK must reject 366 (above 365 cap)");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("check") || msg.contains("constraint"),
            "expected CHECK failure, got: {err}"
        );
    }

    #[test]
    fn auto_archive_days_accepts_boundary_values() {
        let conn = fresh();
        // Lower boundary: 0 disables the sweep per #333.
        conn.execute(
            "UPDATE app_settings SET auto_archive_days = 0 WHERE id = 1",
            [],
        )
        .expect("0 is in range");
        // Upper boundary: 365 is the documented cap.
        conn.execute(
            "UPDATE app_settings SET auto_archive_days = 365 WHERE id = 1",
            [],
        )
        .expect("365 is in range");
    }

    #[test]
    fn notifications_table_columns_exist_after_migration_0021() {
        // Issue #378: the inbox snapshot must survive a PR row prune. Sanity-
        // check the schema exposes every column the store + commands depend
        // on so a regression in 0021 trips here rather than mid-feature.
        let conn = fresh();
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('notifications')")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for col in [
            "id",
            "kind",
            "account_id",
            "pull_request_id",
            "owner",
            "repo",
            "pr_number",
            "pr_node_id",
            "pr_title",
            "title",
            "body",
            "created_at",
            // 0022 read/unread state (issue #379).
            "read_at",
        ] {
            assert!(
                names.iter().any(|n| n == col),
                "missing notifications column: {col}"
            );
        }
    }

    #[test]
    fn notifications_set_null_on_pr_prune() {
        // ADR-0017 / issue #378: the inbox row must outlive the source PR
        // so a user who missed the toast can still see the snapshot. The
        // FK on `pull_request_id` is ON DELETE SET NULL to express that.
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 42, 't', 'open', 'bob',
                        0, 0, 'main', 'feat');
             INSERT INTO notifications
                (id, kind, account_id, pull_request_id,
                 owner, repo, pr_number, pr_title, title, body)
                VALUES (1, 'needs_attention', 1, 100,
                        'owner', 'repo', 42, 't',
                        'Needs your attention', 'owner/repo #42 - t');",
        )
        .unwrap();

        conn.execute("DELETE FROM pull_requests WHERE id = 100", [])
            .unwrap();

        let pr_link: Option<i64> = conn
            .query_row(
                "SELECT pull_request_id FROM notifications WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            pr_link, None,
            "pull_request_id must be set to NULL when the source PR is pruned",
        );
        let snapshot: String = conn
            .query_row("SELECT pr_title FROM notifications WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(snapshot, "t", "snapshot must survive the prune");
    }

    #[test]
    fn notifications_cascade_on_account_delete() {
        // Removing an account drops the inbox copy with it: the OS toast
        // already fired in real time, and the snapshot has no useful
        // surface once its account is gone.
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO notifications
                (id, kind, account_id,
                 owner, repo, pr_number, pr_title, title, body)
                VALUES (1, 'mention', 1,
                        'owner', 'repo', 42, 't',
                        'New mention in owner/repo #42', 'note');",
        )
        .unwrap();

        conn.execute("DELETE FROM accounts WHERE id = 1", [])
            .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "inbox rows must cascade with their account");
    }

    #[test]
    fn notifications_read_at_defaults_to_null_after_migration_0022() {
        // Issue #379: the read/unread slice adds `read_at` as nullable. Newly
        // inserted rows must default to NULL so the dispatch path doesn't have
        // to know about the column. The partial index keys off `read_at IS
        // NULL`, so a non-NULL default would silently break the unread count.
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO notifications
                (kind, account_id, owner, repo, pr_number, pr_title, title)
                VALUES ('mention', 1, 'owner', 'repo', 42, 't', 'tt');",
        )
        .unwrap();
        let read_at: Option<i64> = conn
            .query_row("SELECT read_at FROM notifications", [], |r| r.get(0))
            .unwrap();
        assert_eq!(read_at, None, "read_at must default to NULL on insert");
    }

    #[test]
    fn idx_notifications_unread_is_partial_on_read_at_is_null() {
        // The partial index keeps the unread count cheap. Assert the WHERE
        // clause is present in `sqlite_master.sql` so a regression that drops
        // the predicate (turning it into a full index) trips here.
        let conn = fresh();
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master
                  WHERE type = 'index' AND name = 'idx_notifications_unread'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let normalised = sql.to_lowercase();
        assert!(
            normalised.contains("where") && normalised.contains("read_at is null"),
            "expected partial index WHERE read_at IS NULL, got: {sql}",
        );
    }

    #[test]
    fn notification_retention_max_column_defaults_to_500() {
        // Migration 0023 (issue #380) adds `notification_retention_max` with
        // a default of 500 to match ADR 0028's count-cap decision. Every
        // existing install lands on the same starting cap.
        let conn = fresh();
        let cap: i64 = conn
            .query_row(
                "SELECT notification_retention_max FROM app_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cap, 500);
    }

    #[test]
    fn notification_retention_max_check_rejects_below_floor() {
        // The migration adds `CHECK (notification_retention_max BETWEEN 50
        // AND 5000)` so a write that bypasses the writer's clamp can't
        // smuggle a smaller value into the column.
        let conn = fresh();
        let err = conn
            .execute(
                "UPDATE app_settings SET notification_retention_max = 49 WHERE id = 1",
                [],
            )
            .expect_err("CHECK must reject 49 (below 50 floor)");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("check") || msg.contains("constraint"),
            "expected CHECK failure, got: {err}"
        );
    }

    #[test]
    fn notification_retention_max_check_rejects_above_cap() {
        let conn = fresh();
        let err = conn
            .execute(
                "UPDATE app_settings SET notification_retention_max = 5001 WHERE id = 1",
                [],
            )
            .expect_err("CHECK must reject 5001 (above 5000 cap)");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("check") || msg.contains("constraint"),
            "expected CHECK failure, got: {err}"
        );
    }

    #[test]
    fn notification_retention_max_accepts_boundary_values() {
        let conn = fresh();
        conn.execute(
            "UPDATE app_settings SET notification_retention_max = 50 WHERE id = 1",
            [],
        )
        .expect("50 is in range");
        conn.execute(
            "UPDATE app_settings SET notification_retention_max = 5000 WHERE id = 1",
            [],
        )
        .expect("5000 is in range");
    }

    #[test]
    fn app_settings_check_constraint_blocks_a_second_row() {
        // The migration pins the singleton with `CHECK (id = 1)`. Attempting
        // to INSERT a second row must fail so accidental writes can't fork
        // the settings state.
        let conn = fresh();
        let err = conn
            .execute("INSERT INTO app_settings (id) VALUES (2)", [])
            .expect_err("second row must be rejected by the CHECK constraint");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("check") || msg.contains("constraint"),
            "expected CHECK constraint failure, got: {err}"
        );
    }

    /// Migration filenames must form a gapless `1..=N` sequence with no
    /// duplicate version number, and `N` must equal `MIGRATION_SOURCES.len()`
    /// (the invariant `rusqlite_migration` encodes in `user_version`). This
    /// catches a skipped number, a re-used number from two PRs mid-flight, or
    /// a `MIGRATION_FILENAMES` entry that drifts out of step with
    /// `MIGRATION_SOURCES`.
    #[test]
    fn migration_sources_are_a_gapless_unique_sequence() {
        assert_eq!(
            MIGRATION_FILENAMES.len(),
            MIGRATION_SOURCES.len(),
            "MIGRATION_FILENAMES and MIGRATION_SOURCES must stay in lockstep"
        );

        let versions: Vec<u32> = MIGRATION_FILENAMES
            .iter()
            .map(|name| {
                let prefix = name
                    .split('_')
                    .next()
                    .unwrap_or_else(|| panic!("migration filename has no prefix: {name}"));
                prefix
                    .parse::<u32>()
                    .unwrap_or_else(|_| panic!("migration filename prefix is not numeric: {name}"))
            })
            .collect();

        for (index, version) in versions.iter().enumerate() {
            let expected = index as u32 + 1;
            assert_eq!(
                *version, expected,
                "migration at position {index} must be version {expected}, found {version} \
                 (gap, duplicate, or out-of-order migration number)"
            );
        }

        assert_eq!(
            versions.len(),
            MIGRATION_SOURCES.len(),
            "version count must equal MIGRATION_SOURCES.len()"
        );
    }

    /// Migration 0025 (ADR 0031, issue #431) adds the conversation-unit
    /// read-state foundation. Assert the new table and every new column land
    /// on a fresh DB so a regression in 0025 trips here rather than mid-feature.
    #[test]
    fn migration_0025_adds_conversation_unit_read_state_schema() {
        let conn = fresh();

        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type = 'table' AND name = 'thread_read_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1, "0025 must create thread_read_state");

        let added_columns = [
            ("pull_request_viewer_relations", "general_stream_seen_at"),
            ("pull_request_viewer_relations", "last_emitted_activity_at"),
            ("notifications", "unit_kind"),
            ("notifications", "unit_ref"),
            ("notifications", "deep_link_url"),
            ("review_comments", "mentions_viewer"),
            ("issue_comments", "mentions_viewer"),
        ];
        for (table, column) in added_columns {
            let sql = format!("SELECT name FROM pragma_table_info('{table}')");
            let mut stmt = conn.prepare(&sql).unwrap();
            let names: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert!(
                names.iter().any(|n| n == column),
                "0025 must add {table}.{column}"
            );
        }
    }

    /// `thread_read_state` cascades with its account: the watermark has no
    /// meaning once the viewer identity is gone, matching the FK the ADR 0031
    /// schema declares. Mirrors the other `*_cascade_on_account_delete` tests.
    #[test]
    fn thread_read_state_cascades_on_account_delete() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'me', 0);
             INSERT INTO thread_read_state
                (account_id, review_thread_node_id, seen_at)
                VALUES (1, 'RT_1', 5);",
        )
        .unwrap();

        conn.execute("DELETE FROM accounts WHERE id = 1", [])
            .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM thread_read_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "thread_read_state must cascade with its account");
    }

    /// 0025 must apply cleanly on top of a populated post-0024 schema: it is
    /// additive, so existing `pull_request_viewer_relations` and
    /// `notifications` rows survive and the new columns default. Replays up
    /// through 0024, seeds a relation and an inbox row, runs 0025, then reads
    /// both back to prove the rows persisted with NULL defaults on the new
    /// nullable columns.
    #[test]
    fn migration_0025_preserves_existing_rows_and_defaults_new_columns() {
        // 0025 sits at zero-index 24 (NNNN numbers start at 0001), so
        // `take(24)` lands every migration up through 0024 and stops before
        // 0025.
        const PRE_0025_PREFIX: usize = 24;
        let mut conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();
        let pre_0025 = Migrations::new(
            MIGRATION_SOURCES
                .iter()
                .take(PRE_0025_PREFIX)
                .map(|sql| M::up(sql))
                .collect(),
        );
        pre_0025.to_latest(&mut conn).unwrap();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 42, 't', 'open', 'bob', 0, 0, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, is_authored, is_review_requested,
                 is_involved, relation_observed_at)
                VALUES (1, 100, 1, 0, 1, 7);
             INSERT INTO notifications
                (id, kind, account_id, pull_request_id,
                 owner, repo, pr_number, pr_title, title, body)
                VALUES (1, 'needs_attention', 1, 100,
                        'owner', 'repo', 42, 't',
                        'Needs your attention', 'owner/repo #42 - t');",
        )
        .unwrap();

        migrations().to_latest(&mut conn).unwrap();

        // The relation row survives and its new watermark columns default NULL.
        let (observed_at, general_seen, last_emitted): (i64, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT relation_observed_at, general_stream_seen_at, last_emitted_activity_at
                   FROM pull_request_viewer_relations
                  WHERE account_id = 1 AND pull_request_id = 100",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(observed_at, 7, "pre-0025 relation row must survive");
        assert_eq!(
            general_seen, None,
            "general_stream_seen_at must default NULL"
        );
        assert_eq!(
            last_emitted, None,
            "last_emitted_activity_at must default NULL"
        );

        // The inbox row survives and its new unit columns default NULL.
        let (pr_title, unit_kind, unit_ref, deep_link): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT pr_title, unit_kind, unit_ref, deep_link_url
                   FROM notifications WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(pr_title, "t", "pre-0025 inbox row must survive");
        assert_eq!(unit_kind, None, "unit_kind must default NULL");
        assert_eq!(unit_ref, None, "unit_ref must default NULL");
        assert_eq!(deep_link, None, "deep_link_url must default NULL");
    }
}
