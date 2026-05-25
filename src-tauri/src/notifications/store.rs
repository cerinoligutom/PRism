//! SQL writer / reader pair for the `notifications` table.
//!
//! Used by [`crate::notify::runtime::TauriNotificationSink`] (insert on
//! dispatch) and the [`super::commands`] surface (list / delete / clear).
//!
//! All reads return rows newest first - the inbox view scrolls from the
//! freshest entry down. The `before_id` cursor on [`list`] seeds a future
//! paginated load; v1 uses it only via `limit`.

use rusqlite::{params, Connection};

use super::types::{Notification, NotificationInsert};

/// Compile-time fallback for the retention cap, mirroring the migration's
/// default. Read by [`insert`] when [`load_retention_cap`] fails so a
/// transient settings-read error never silently disables pruning.
const DEFAULT_RETENTION_MAX: i64 = 500;

/// Insert one inbox row, then prune the table to the configured retention
/// cap (ADR 0028, issue #380).
///
/// Called from the dispatch hook with the same `kind` / `title` / `body` /
/// snapshot the OS toast used. Returns the new row id so test fixtures can
/// assert against it; the production path discards it.
///
/// Retention pruning runs once per insert and uses
/// `app_settings.notification_retention_max`. If the settings read fails the
/// pruner falls back to [`DEFAULT_RETENTION_MAX`] and logs - the insert still
/// succeeds so the OS toast never gets out of sync with the inbox.
pub fn insert(conn: &Connection, n: &NotificationInsert) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO notifications
            (kind, account_id, pull_request_id,
             owner, repo, pr_number, pr_node_id, pr_title,
             title, body)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            n.kind,
            n.account_id,
            n.pull_request_id,
            n.owner,
            n.repo,
            n.pr_number,
            n.pr_node_id,
            n.pr_title,
            n.title,
            n.body,
        ],
    )?;
    let id = conn.last_insert_rowid();
    let cap = load_retention_cap(conn).unwrap_or_else(|err| {
        tracing::warn!(
            %err,
            "notifications: retention cap read failed, falling back to default {}",
            DEFAULT_RETENTION_MAX,
        );
        DEFAULT_RETENTION_MAX
    });
    if let Err(err) = prune_to_cap(conn, cap) {
        tracing::warn!(%err, "notifications: prune_to_cap failed, inbox over cap until next insert");
    }
    Ok(id)
}

/// Read the configured retention cap from the singleton `app_settings` row.
/// The migration's CHECK constraint guarantees the value is in `[50, 5000]`
/// at rest, so callers never have to re-clamp.
fn load_retention_cap(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT notification_retention_max FROM app_settings WHERE id = 1",
        [],
        |row| row.get(0),
    )
}

/// Drop every row except the newest `cap` (ordered by `created_at DESC, id
/// DESC` so ties on the same second still get a stable head). Returns the
/// number of rows removed for test assertion.
///
/// The `id NOT IN (subquery)` shape mirrors ADR 0028's SQL and reads cleanly
/// against the existing `idx_notifications_created_at` index. Cheap enough
/// to run once per dispatch even at the upper cap.
fn prune_to_cap(conn: &Connection, cap: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM notifications
          WHERE id NOT IN (
              SELECT id FROM notifications
              ORDER BY created_at DESC, id DESC
              LIMIT ?1
          )",
        params![cap],
    )
}

/// Read up to `limit` inbox rows, newest first.
///
/// `before_id = Some(id)` enables future cursor pagination by skipping every
/// row with `id >= before_id`. Both args optional: omit `limit` for the
/// full list, omit `before_id` for "from the top".
pub fn list(
    conn: &Connection,
    limit: Option<i64>,
    before_id: Option<i64>,
) -> rusqlite::Result<Vec<Notification>> {
    let mut sql = String::from(
        "SELECT id, kind, account_id, pull_request_id,
                owner, repo, pr_number, pr_node_id, pr_title,
                title, body, created_at, read_at
           FROM notifications",
    );
    let mut binds: Vec<rusqlite::types::Value> = Vec::with_capacity(2);
    if let Some(id) = before_id {
        sql.push_str(" WHERE id < ?1");
        binds.push(rusqlite::types::Value::Integer(id));
    }
    sql.push_str(" ORDER BY id DESC");
    if let Some(n) = limit {
        let placeholder = if binds.is_empty() { "?1" } else { "?2" };
        sql.push_str(&format!(" LIMIT {placeholder}"));
        binds.push(rusqlite::types::Value::Integer(n));
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(binds.iter()), map_row)?;
    rows.collect()
}

/// Delete one row by id. Returns the number of rows actually removed; the
/// caller can use a `0` to surface a "this row was already cleared" hint,
/// though the v1 UI just refetches the list either way.
pub fn delete_one(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM notifications WHERE id = ?1", params![id])
}

/// Wipe every inbox row. Returns the rows removed so the caller can avoid
/// the round-trip refetch when the list was already empty.
pub fn delete_all(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM notifications", [])
}

/// Mark one row read, stamping `read_at` with the current epoch seconds.
///
/// Idempotent: rows whose `read_at` is already non-NULL are skipped via the
/// `WHERE read_at IS NULL` predicate, so a double-click on the same row
/// keeps the original read time. Returns the rows actually updated (0 or 1).
pub fn mark_read(conn: &Connection, id: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE notifications
            SET read_at = strftime('%s', 'now')
          WHERE id = ?1 AND read_at IS NULL",
        params![id],
    )
}

/// Mark every unread row read in one transaction.
///
/// Returns the rows actually updated so the caller can avoid a redundant
/// refetch when the list was already fully read.
pub fn mark_all_read(conn: &Connection) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE notifications
            SET read_at = strftime('%s', 'now')
          WHERE read_at IS NULL",
        [],
    )
}

/// Count unread rows. Backed by the partial index on `read_at IS NULL` so the
/// query stays cheap as the table approaches the count cap (#380).
pub fn count_unread(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM notifications WHERE read_at IS NULL",
        [],
        |row| row.get(0),
    )
}

/// Read one row by id. Used by tests; the commands surface only exposes the
/// list / delete shape. Returns `None` when the row is missing rather than
/// surfacing rusqlite's `QueryReturnedNoRows`.
#[cfg(test)]
pub fn find(conn: &Connection, id: i64) -> rusqlite::Result<Option<Notification>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, kind, account_id, pull_request_id,
                owner, repo, pr_number, pr_node_id, pr_title,
                title, body, created_at, read_at
           FROM notifications
          WHERE id = ?1",
        params![id],
        map_row,
    )
    .optional()
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Notification> {
    Ok(Notification {
        id: row.get(0)?,
        kind: row.get(1)?,
        account_id: row.get(2)?,
        pull_request_id: row.get(3)?,
        owner: row.get(4)?,
        repo: row.get(5)?,
        pr_number: row.get(6)?,
        pr_node_id: row.get(7)?,
        pr_title: row.get(8)?,
        title: row.get(9)?,
        body: row.get(10)?,
        created_at: row.get(11)?,
        read_at: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);",
        )
        .unwrap();
        conn
    }

    fn sample(owner: &str, repo: &str, number: i64, title: &str) -> NotificationInsert {
        NotificationInsert {
            kind: "needs_attention".to_string(),
            account_id: 1,
            pull_request_id: None,
            owner: owner.to_string(),
            repo: repo.to_string(),
            pr_number: number,
            pr_node_id: None,
            pr_title: title.to_string(),
            title: "Needs your attention".to_string(),
            body: Some(format!("{owner}/{repo} #{number} - {title}")),
        }
    }

    #[test]
    fn insert_then_find_returns_the_row() {
        let conn = fresh();
        let id = insert(&conn, &sample("owner", "web", 42, "Add a thing")).unwrap();
        let row = find(&conn, id).unwrap().expect("row");
        assert_eq!(row.kind, "needs_attention");
        assert_eq!(row.owner, "owner");
        assert_eq!(row.pr_number, 42);
        assert_eq!(row.pr_title, "Add a thing");
        assert!(row.created_at > 0, "created_at must seed from now()");
    }

    #[test]
    fn list_returns_newest_first() {
        let conn = fresh();
        insert(&conn, &sample("o", "a", 1, "first")).unwrap();
        insert(&conn, &sample("o", "a", 2, "second")).unwrap();
        insert(&conn, &sample("o", "a", 3, "third")).unwrap();
        let rows = list(&conn, None, None).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].pr_number, 3, "newest row sits at the head");
        assert_eq!(rows[2].pr_number, 1, "oldest sits at the tail");
    }

    #[test]
    fn list_honours_limit() {
        let conn = fresh();
        for n in 1..=5 {
            insert(&conn, &sample("o", "a", n, "t")).unwrap();
        }
        let rows = list(&conn, Some(2), None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pr_number, 5);
        assert_eq!(rows[1].pr_number, 4);
    }

    #[test]
    fn list_honours_before_id_cursor() {
        // before_id seeds the future cursor pagination path. Asserting
        // the predicate now means the contract is locked even before the UI
        // calls it.
        let conn = fresh();
        let mut ids = Vec::new();
        for n in 1..=5 {
            ids.push(insert(&conn, &sample("o", "a", n, "t")).unwrap());
        }
        // Skip rows whose id >= the third-from-newest id; expect the two
        // earliest survive.
        let rows = list(&conn, None, Some(ids[2])).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, ids[1]);
        assert_eq!(rows[1].id, ids[0]);
    }

    #[test]
    fn delete_one_removes_a_single_row() {
        let conn = fresh();
        let a = insert(&conn, &sample("o", "a", 1, "t")).unwrap();
        let b = insert(&conn, &sample("o", "a", 2, "t")).unwrap();
        let removed = delete_one(&conn, a).unwrap();
        assert_eq!(removed, 1);
        let rows = list(&conn, None, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, b);
    }

    #[test]
    fn delete_one_with_unknown_id_is_a_zero_row_noop() {
        let conn = fresh();
        let removed = delete_one(&conn, 999).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn delete_all_clears_the_table() {
        let conn = fresh();
        for n in 1..=3 {
            insert(&conn, &sample("o", "a", n, "t")).unwrap();
        }
        let removed = delete_all(&conn).unwrap();
        assert_eq!(removed, 3);
        assert!(list(&conn, None, None).unwrap().is_empty());
    }

    #[test]
    fn insert_carries_the_pull_request_id_link() {
        let conn = fresh();
        conn.execute_batch(
            "INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'web', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 42, 'Add a thing', 'open', 'bob',
                        0, 0, 'main', 'feat');",
        )
        .unwrap();
        let mut payload = sample("owner", "web", 42, "Add a thing");
        payload.pull_request_id = Some(100);
        let id = insert(&conn, &payload).unwrap();
        let row = find(&conn, id).unwrap().expect("row");
        assert_eq!(row.pull_request_id, Some(100));
    }

    #[test]
    fn fresh_row_starts_unread() {
        let conn = fresh();
        let id = insert(&conn, &sample("o", "a", 1, "t")).unwrap();
        let row = find(&conn, id).unwrap().expect("row");
        assert_eq!(row.read_at, None, "new rows default to unread");
    }

    #[test]
    fn mark_read_stamps_read_at_on_an_unread_row() {
        let conn = fresh();
        let id = insert(&conn, &sample("o", "a", 1, "t")).unwrap();
        let updated = mark_read(&conn, id).unwrap();
        assert_eq!(updated, 1);
        let row = find(&conn, id).unwrap().expect("row");
        let read_at = row.read_at.expect("read_at populated");
        assert!(read_at > 0, "read_at carries an epoch timestamp");
    }

    #[test]
    fn mark_read_is_idempotent_on_already_read_rows() {
        // ADR 0028 decision 3: clicking a read row a second time keeps the
        // original `read_at` rather than overwriting it. The UPDATE skips
        // via `WHERE read_at IS NULL`.
        let conn = fresh();
        let id = insert(&conn, &sample("o", "a", 1, "t")).unwrap();
        mark_read(&conn, id).unwrap();
        let first = find(&conn, id).unwrap().expect("row").read_at;
        let updated = mark_read(&conn, id).unwrap();
        assert_eq!(updated, 0, "no rows touched on the second call");
        let second = find(&conn, id).unwrap().expect("row").read_at;
        assert_eq!(first, second, "original read_at preserved");
    }

    #[test]
    fn mark_read_with_unknown_id_is_a_zero_row_noop() {
        let conn = fresh();
        let updated = mark_read(&conn, 999).unwrap();
        assert_eq!(updated, 0);
    }

    #[test]
    fn mark_all_read_returns_only_the_just_marked_count() {
        let conn = fresh();
        let a = insert(&conn, &sample("o", "a", 1, "t")).unwrap();
        let _b = insert(&conn, &sample("o", "a", 2, "t")).unwrap();
        let _c = insert(&conn, &sample("o", "a", 3, "t")).unwrap();
        mark_read(&conn, a).unwrap();
        let updated = mark_all_read(&conn).unwrap();
        assert_eq!(updated, 2, "only the two unread rows are touched");
        assert_eq!(count_unread(&conn).unwrap(), 0);
    }

    #[test]
    fn count_unread_reflects_inserts_and_marks() {
        let conn = fresh();
        assert_eq!(count_unread(&conn).unwrap(), 0, "empty table");
        let a = insert(&conn, &sample("o", "a", 1, "t")).unwrap();
        insert(&conn, &sample("o", "a", 2, "t")).unwrap();
        assert_eq!(count_unread(&conn).unwrap(), 2);
        mark_read(&conn, a).unwrap();
        assert_eq!(count_unread(&conn).unwrap(), 1);
        mark_all_read(&conn).unwrap();
        assert_eq!(count_unread(&conn).unwrap(), 0);
    }

    #[test]
    fn list_surfaces_read_at_on_each_row() {
        let conn = fresh();
        let a = insert(&conn, &sample("o", "a", 1, "t")).unwrap();
        insert(&conn, &sample("o", "a", 2, "t")).unwrap();
        mark_read(&conn, a).unwrap();
        let rows = list(&conn, None, None).unwrap();
        let read_row = rows.iter().find(|r| r.id == a).expect("row");
        let unread_row = rows.iter().find(|r| r.id != a).expect("row");
        assert!(read_row.read_at.is_some(), "marked row carries read_at");
        assert_eq!(unread_row.read_at, None, "unread row carries None");
    }

    /// Bulk-insert `count` rows under the same account/repo coordinates so
    /// the retention tests don't repeat the seed boilerplate. Each row gets
    /// a unique `pr_number` derived from the row index plus an explicit
    /// `created_at` that climbs monotonically - the migration default of
    /// `strftime('%s', 'now')` collapses too many rows onto the same second
    /// in this loop, and the prune predicate orders by `created_at DESC, id
    /// DESC` so ties on the second still survive on id, but climbing the
    /// timestamp keeps the ordering legible for the assertions.
    fn seed_many(conn: &Connection, count: i64) -> Vec<i64> {
        let mut ids = Vec::with_capacity(count as usize);
        for n in 1..=count {
            conn.execute(
                "INSERT INTO notifications
                    (kind, account_id, pull_request_id,
                     owner, repo, pr_number, pr_node_id, pr_title,
                     title, body, created_at)
                    VALUES ('needs_attention', 1, NULL,
                            'o', 'a', ?1, NULL, 't',
                            'Needs your attention', NULL, ?2)",
                params![n, n],
            )
            .unwrap();
            ids.push(conn.last_insert_rowid());
        }
        ids
    }

    #[test]
    fn prune_to_cap_drops_oldest_rows_beyond_the_cap() {
        // Issue #380: insert > cap rows, prune, assert the count lands on
        // the cap and the dropped rows are the oldest ones. The fixture
        // inserts 510 rows under a cap of 500.
        let conn = fresh();
        let ids = seed_many(&conn, 510);
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
                .unwrap(),
            510
        );

        let removed = prune_to_cap(&conn, 500).unwrap();
        assert_eq!(removed, 10, "exactly the overflow rows must go");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 500, "the table must settle exactly on the cap");

        // The oldest 10 rows (by created_at ASC, id ASC) were ids[0..10];
        // none of them must survive.
        let placeholders = (1..=10)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT COUNT(*) FROM notifications WHERE id IN ({placeholders})");
        let survivors: i64 = conn
            .query_row(
                &sql,
                rusqlite::params_from_iter(ids[..10].iter().copied()),
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(survivors, 0, "the oldest 10 rows must be pruned");
    }

    #[test]
    fn prune_to_cap_is_a_noop_when_under_the_cap() {
        let conn = fresh();
        seed_many(&conn, 100);
        let removed = prune_to_cap(&conn, 500).unwrap();
        assert_eq!(removed, 0);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 100);
    }

    #[test]
    fn insert_keeps_table_bounded_by_app_settings_cap() {
        // End-to-end: drop the cap to the floor of 50 via the settings row,
        // then push 55 notifications through `insert`. The table must never
        // exceed 50 and the survivors are the newest ones.
        let conn = fresh();
        conn.execute(
            "UPDATE app_settings SET notification_retention_max = 50 WHERE id = 1",
            [],
        )
        .unwrap();
        for n in 1..=55 {
            insert(&conn, &sample("o", "a", n, "t")).unwrap();
        }
        let rows = list(&conn, None, None).unwrap();
        assert_eq!(rows.len(), 50);
        // Newest first: the freshest pr_number must be 55, the oldest 6.
        assert_eq!(rows[0].pr_number, 55);
        assert_eq!(rows[49].pr_number, 6);
    }
}
