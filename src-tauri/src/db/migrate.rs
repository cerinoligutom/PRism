//! Migration runner for the local SQLite cache.
//!
//! Migrations are forward-only and embedded into the binary via `include_str!`.
//! Add a new migration by creating `src-tauri/migrations/NNNN_*.sql` and listing
//! it in [`MIGRATION_SOURCES`].

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

/// SQL text for each migration, in apply order. Each entry corresponds to a
/// file under `src-tauri/migrations/`.
const MIGRATION_SOURCES: &[&str] = &[include_str!("../../migrations/0001_init.sql")];

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
}
