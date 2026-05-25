//! SQL writer / reader pair for the `notifications` table.
//!
//! Used by [`crate::notify::runtime::TauriNotificationSink`] (insert on
//! dispatch) and the [`super::commands`] surface (list / delete / clear).
//!
//! All reads return rows newest first - the inbox view scrolls from the
//! freshest entry down. The `before_id` cursor on [`list`] is the seed for
//! the follow-up paginated load (#379); v1 uses it only via `limit`.

use rusqlite::{params, Connection};

use super::types::{Notification, NotificationInsert};

/// Insert one inbox row.
///
/// Called from the dispatch hook with the same `kind` / `title` / `body` /
/// snapshot the OS toast used. Returns the new row id so test fixtures can
/// assert against it; the production path discards it.
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
    Ok(conn.last_insert_rowid())
}

/// Read up to `limit` inbox rows, newest first.
///
/// `before_id = Some(id)` enables future cursor pagination (#379) by skipping
/// every row with `id >= before_id`. Both args optional: omit `limit` for the
/// full list, omit `before_id` for "from the top".
pub fn list(
    conn: &Connection,
    limit: Option<i64>,
    before_id: Option<i64>,
) -> rusqlite::Result<Vec<Notification>> {
    let mut sql = String::from(
        "SELECT id, kind, account_id, pull_request_id,
                owner, repo, pr_number, pr_node_id, pr_title,
                title, body, created_at
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

/// Read one row by id. Used by tests; the commands surface only exposes the
/// list / delete shape. Returns `None` when the row is missing rather than
/// surfacing rusqlite's `QueryReturnedNoRows`.
#[cfg(test)]
pub fn find(conn: &Connection, id: i64) -> rusqlite::Result<Option<Notification>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, kind, account_id, pull_request_id,
                owner, repo, pr_number, pr_node_id, pr_title,
                title, body, created_at
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
        // before_id seeds the follow-up pagination slice (#379). Asserting
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
}
