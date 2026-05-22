//! Local SQLite cache for PRism.
//!
//! - [`migrate`] embeds and runs the forward-only schema migrations under
//!   `src-tauri/migrations/`.
//! - [`etag_store`] implements the SQLite-backed `EtagStore` used by the
//!   GitHub clients (see `docs/contracts/github-client.md`).
//!
//! Connections are wrapped in `Arc<Mutex<Connection>>` because
//! `rusqlite::Connection` is `!Sync`. The cache is single-writer (sync worker)
//! / multi-reader (UI); WAL mode keeps reads non-blocking.

pub mod etag_store;
pub mod migrate;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::Connection;
use tauri::{AppHandle, Manager, Runtime};

pub use etag_store::SqliteEtagStore;

/// Filename of the cache database within the app data directory.
pub const DB_FILE_NAME: &str = "prism.sqlite";

/// Acquire the connection lock, mapping a poisoned mutex to a
/// `rusqlite::Error` so helpers that already thread `rusqlite::Error` upwards
/// can `?` the lock failure into their normal error path. A poisoned mutex
/// means a previous holder panicked; the sync cycle treats this as a
/// recoverable failure rather than crashing the worker loop (see issue #238).
pub fn lock_db(db: &DbHandle) -> Result<MutexGuard<'_, Connection>, rusqlite::Error> {
    db.lock().map_err(|_| {
        rusqlite::Error::ToSqlConversionFailure("db connection mutex poisoned".to_string().into())
    })
}

/// Initialisation errors surfaced to `lib.rs` for fail-fast on startup.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("resolve app data dir: {0}")]
    Path(#[from] tauri::Error),
    #[error("create app data dir: {0}")]
    CreateDir(#[from] std::io::Error),
    #[error("open sqlite db: {0}")]
    Open(rusqlite::Error),
    #[error("run migrations: {0}")]
    Migrate(#[from] rusqlite_migration::Error),
}

/// Shared handle to the cache connection. Cheap to clone.
pub type DbHandle = Arc<Mutex<Connection>>;

/// Resolve `<app_data_dir>/prism.sqlite`, creating the directory if needed.
pub fn resolve_db_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, DbError> {
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(DB_FILE_NAME))
}

/// Open a connection at `path`, apply pragmas, and run migrations to latest.
pub fn open_at(path: &Path) -> Result<DbHandle, DbError> {
    let mut conn = Connection::open(path).map_err(DbError::Open)?;
    migrate::run(&mut conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

/// Initialise the cache during `tauri::Builder::setup`. Resolves the data dir,
/// opens the connection, and runs migrations. The returned handle should be
/// stashed in Tauri's managed state so commands can clone it.
pub fn init<R: Runtime>(app: &AppHandle<R>) -> Result<DbHandle, DbError> {
    let path = resolve_db_path(app)?;
    open_at(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_at_creates_file_and_runs_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prism.sqlite");
        let handle = open_at(&path).expect("init db");

        assert!(path.exists(), "db file should be created");

        let conn = handle.lock().unwrap();
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert!(version >= 1, "migrations should bump user_version");
    }

    #[test]
    fn reopening_existing_db_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prism.sqlite");
        let _h1 = open_at(&path).expect("first open");
        let _h2 = open_at(&path).expect("second open");
    }
}
