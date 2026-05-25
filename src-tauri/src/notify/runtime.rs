//! Production [`NotificationSink`] backed by `tauri-plugin-notification`.
//!
//! Owns three things:
//! * an `AppHandle` so it can address the plugin (the plugin extends every
//!   Tauri runtime handle via `NotificationExt`);
//! * a [`DbHandle`] so it can read the master switch + permission state and
//!   persist the result of a prompt;
//! * a [`PermissionAsker`] so the prompt path is testable without booting a
//!   real Tauri runtime.
//!
//! Permission lifecycle (ADR 0017 decision 5):
//!
//! 1. Read the singleton `app_settings` row.
//! 2. If `notifications_enabled` is OFF (master switch), skip - never touch
//!    permission state. The user hasn't opted in, so there's no reason to
//!    poke the OS prompt.
//! 3. If the master is ON, branch on `notification_permission_state`:
//!    * `Unprompted` -> ask the OS, persist the result, then dispatch iff
//!      granted;
//!    * `Granted` -> dispatch;
//!    * `Denied` -> log and skip (re-prompting after denial would be
//!      browser-grade dark-pattern territory).
//!
//! The trigger -> notification formatting step (issue #192) is the caller's
//! responsibility; the sink only handles dispatch and the OS handshake.

use std::sync::Arc;

use tauri::{AppHandle, Runtime};

use crate::db::DbHandle;
use crate::notifications::{store as inbox_store, NotificationInsert};
use crate::notify::pending::PendingPayloadQueueHandle;
use crate::notify::sink::{NotificationSink, PermissionAsker};
use crate::notify::types::{Notification, NotificationKind, NotificationSnapshot};
use crate::settings::{AppSettings, NotificationPermissionState};

/// Production [`NotificationSink`] used by `lib.rs`.
pub struct TauriNotificationSink<R: Runtime, A: PermissionAsker> {
    app: AppHandle<R>,
    db: DbHandle,
    asker: Arc<A>,
    pending: PendingPayloadQueueHandle,
}

impl<R: Runtime, A: PermissionAsker> TauriNotificationSink<R, A> {
    pub fn new(
        app: AppHandle<R>,
        db: DbHandle,
        asker: Arc<A>,
        pending: PendingPayloadQueueHandle,
    ) -> Self {
        Self {
            app,
            db,
            asker,
            pending,
        }
    }
}

impl<R: Runtime, A: PermissionAsker> NotificationSink for TauriNotificationSink<R, A> {
    fn dispatch(&self, notification: &Notification) {
        if !decide_dispatch(&self.db, self.asker.as_ref()) {
            return;
        }
        // Enqueue the deep-link payload before the toast fires. The
        // window-event hook in `lib.rs` drains the queue on the next
        // `WindowEvent::Focused(true)` and emits `notification://open-pr`
        // for each entry - the OS-native click-activates-app behaviour
        // surfaces as a focus event on every desktop platform, and the
        // `tauri-plugin-notification` v2.3.3 desktop API doesn't expose a
        // per-notification action callback. See `notify::pending` for the
        // TTL bound on stale-focus false positives.
        self.pending.enqueue(notification.payload.clone());

        use tauri_plugin_notification::NotificationExt;
        if let Err(err) = self
            .app
            .notification()
            .builder()
            .title(notification.title.clone())
            .body(notification.body.clone())
            .show()
        {
            tracing::error!(%err, "notify: plugin show failed");
        }

        // Mirror the dispatched toast into the persistent inbox so a missed
        // toast can still be recovered from `/dashboard/notifications`
        // (issue #378). Inbox insertion runs after the OS handshake on
        // purpose: the OS toast and the inbox write have different
        // reliability requirements, and a flaky inbox write must never
        // silence the toast. Failures here log and continue.
        if let Some(snapshot) = notification.snapshot.as_ref() {
            persist_inbox_row(&self.db, notification, snapshot);
        }
    }
}

/// Convert a dispatched [`Notification`] into the persistent inbox row shape
/// and write it. Failures are logged and dropped - the OS toast still fires,
/// and the user retains the in-app badge as the always-on signal.
fn persist_inbox_row(db: &DbHandle, notification: &Notification, snapshot: &NotificationSnapshot) {
    let insert = NotificationInsert {
        kind: kind_storage(snapshot.kind).to_string(),
        account_id: snapshot.account_id,
        pull_request_id: snapshot.pull_request_id,
        owner: snapshot.owner.clone(),
        repo: snapshot.repo.clone(),
        pr_number: snapshot.pr_number,
        pr_node_id: snapshot.pr_node_id.clone(),
        pr_title: snapshot.pr_title.clone(),
        title: notification.title.clone(),
        body: Some(notification.body.clone()),
    };
    let conn = match db.lock() {
        Ok(g) => g,
        Err(err) => {
            tracing::warn!(%err, "notifications inbox: db lock poisoned, skipping insert");
            return;
        }
    };
    if let Err(err) = inbox_store::insert(&conn, &insert) {
        tracing::warn!(%err, "notifications inbox: insert failed, OS toast still fires");
    }
}

/// String storage for [`NotificationKind`] used by the inbox `kind` column.
/// Matches `#[serde(rename_all = "snake_case")]` on the enum so the inbox
/// rows compare cleanly against the wire form when the frontend reads them.
fn kind_storage(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::NeedsAttention => "needs_attention",
        NotificationKind::Mention => "mention",
    }
}

/// Run the master-switch + permission-state decision and return `true` if
/// the caller should proceed with the OS toast. Side effects: persists the
/// answered permission state when the prompt path runs, logs on rusqlite or
/// plugin failures. Lifted out of [`TauriNotificationSink::dispatch`] so the
/// tests can exercise the decision without constructing an `AppHandle`.
pub(crate) fn decide_dispatch(db: &DbHandle, asker: &dyn PermissionAsker) -> bool {
    let settings = match load_settings(db) {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(%err, "notify: load app_settings failed");
            return false;
        }
    };
    if !settings.notifications_enabled {
        // Master switch off. Don't prompt; don't dispatch. The in-app badge
        // keeps doing its job (ADR 0017 decision drivers).
        return false;
    }
    match settings.notification_permission_state {
        NotificationPermissionState::Granted => true,
        NotificationPermissionState::Unprompted => {
            let answered = asker.request();
            if let Err(err) = persist_permission(db, answered) {
                tracing::error!(%err, "notify: persist permission state failed");
            }
            answered == NotificationPermissionState::Granted
        }
        NotificationPermissionState::Denied => {
            tracing::debug!("notify: skipping dispatch, OS permission denied");
            false
        }
    }
}

/// Production [`PermissionAsker`] backed by the plugin. Wraps an `AppHandle`
/// and maps the plugin's `PermissionState` (which has four variants
/// including Android-specific ones) onto our three-state DB shape.
pub struct PluginPermissionAsker<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> PluginPermissionAsker<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> PermissionAsker for PluginPermissionAsker<R> {
    fn current(&self) -> NotificationPermissionState {
        use tauri_plugin_notification::NotificationExt;
        match self.app.notification().permission_state() {
            Ok(state) => map_plugin_state(state),
            Err(err) => {
                tracing::warn!(%err, "notify: plugin permission_state failed");
                NotificationPermissionState::Denied
            }
        }
    }

    fn request(&self) -> NotificationPermissionState {
        use tauri_plugin_notification::NotificationExt;
        match self.app.notification().request_permission() {
            Ok(state) => map_plugin_state(state),
            Err(err) => {
                // Mapping plugin failures to Denied keeps the user in control
                // of a retry: the sink will persist Denied, the Settings
                // panel will show the "blocked" callout (#195), and the next
                // dispatch won't re-ping the OS.
                tracing::warn!(%err, "notify: plugin request_permission failed");
                NotificationPermissionState::Denied
            }
        }
    }
}

fn map_plugin_state(
    state: tauri_plugin_notification::PermissionState,
) -> NotificationPermissionState {
    use tauri_plugin_notification::PermissionState as P;
    match state {
        P::Granted => NotificationPermissionState::Granted,
        P::Denied => NotificationPermissionState::Denied,
        // Prompt / PromptWithRationale both mean "we haven't asked yet";
        // collapse onto our `Unprompted` so the next dispatch path triggers
        // the OS prompt rather than treating the slot as denied.
        _ => NotificationPermissionState::Unprompted,
    }
}

fn load_settings(db: &DbHandle) -> rusqlite::Result<AppSettings> {
    let conn = db.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
    AppSettings::load(&conn)
}

fn persist_permission(db: &DbHandle, state: NotificationPermissionState) -> rusqlite::Result<()> {
    let conn = db.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
    let stored = match state {
        NotificationPermissionState::Unprompted => "unprompted",
        NotificationPermissionState::Granted => "granted",
        NotificationPermissionState::Denied => "denied",
    };
    conn.execute(
        "UPDATE app_settings
            SET notification_permission_state = ?1,
                updated_at = strftime('%s', 'now')
          WHERE id = 1",
        rusqlite::params![stored],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Inbox-write coverage for the dispatch hook (issue #378).
    //!
    //! `decide_dispatch` and the permission flow are tested in `notify::tests`;
    //! these tests exercise the persistence side without going through the
    //! Tauri runtime. `persist_inbox_row` is `pub(super)`-equivalent (a free
    //! function in the same module) so the tests reach it directly.
    use std::sync::{Arc, Mutex};

    use rusqlite::Connection;
    use serde_json::json;

    use super::*;
    use crate::notifications::store as inbox_store;
    use crate::notify::types::{Notification, NotificationKind, NotificationSnapshot};

    fn fresh_db() -> DbHandle {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        // Seed an account + repo + PR so the inbox FK on
        // `pull_request_id` resolves. The dispatch hook is only ever
        // exercised against an existing PR (the recompute trigger emits
        // an id from a row it just observed).
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'web', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 42, 'Add a thing', 'open', 'bob',
                        0, 0, 'main', 'feat');",
        )
        .unwrap();
        Arc::new(Mutex::new(conn))
    }

    fn sample_notification(kind: NotificationKind) -> Notification {
        Notification {
            title: "Needs your attention".to_string(),
            body: "owner/web #42 - Add a thing".to_string(),
            payload: json!({ "account_id": 1, "pull_request_id": 100 }),
            snapshot: Some(NotificationSnapshot {
                kind,
                account_id: 1,
                pull_request_id: Some(100),
                owner: "owner".to_string(),
                repo: "web".to_string(),
                pr_number: 42,
                pr_node_id: None,
                pr_title: "Add a thing".to_string(),
            }),
        }
    }

    #[test]
    fn persist_inbox_row_writes_snapshot_with_serialised_kind() {
        // The dispatch hook stores the kind as snake_case so the wire form
        // round-trips cleanly through the renderer. Asserting on the column
        // catches a future enum-variant rename that forgets the storage map.
        let db = fresh_db();
        let n = sample_notification(NotificationKind::NeedsAttention);
        let snapshot = n.snapshot.as_ref().unwrap();
        persist_inbox_row(&db, &n, snapshot);

        let conn = db.lock().unwrap();
        let rows = inbox_store::list(&conn, None, None).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.kind, "needs_attention");
        assert_eq!(row.account_id, 1);
        assert_eq!(row.pull_request_id, Some(100));
        assert_eq!(row.owner, "owner");
        assert_eq!(row.repo, "web");
        assert_eq!(row.pr_number, 42);
        assert_eq!(row.pr_title, "Add a thing");
        assert_eq!(row.title, "Needs your attention");
        assert_eq!(row.body.as_deref(), Some("owner/web #42 - Add a thing"));
    }

    #[test]
    fn persist_inbox_row_translates_mention_kind() {
        let db = fresh_db();
        let n = sample_notification(NotificationKind::Mention);
        let snapshot = n.snapshot.as_ref().unwrap();
        persist_inbox_row(&db, &n, snapshot);

        let conn = db.lock().unwrap();
        let rows = inbox_store::list(&conn, None, None).unwrap();
        assert_eq!(rows[0].kind, "mention");
    }

    #[test]
    fn persist_inbox_row_swallows_db_error_without_panicking() {
        // The inbox is best-effort: even a missing accounts row (FK
        // violation) must not bubble up to the dispatch caller. The OS
        // toast is the always-on signal.
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        // Skip seeding the accounts row so the FK violates.
        let db: DbHandle = Arc::new(Mutex::new(conn));
        let mut n = sample_notification(NotificationKind::NeedsAttention);
        // Force a FK miss: account_id 99 doesn't exist.
        if let Some(ref mut s) = n.snapshot {
            s.account_id = 99;
        }
        let snapshot = n.snapshot.clone().unwrap();
        // The call must complete without panicking; the row simply doesn't
        // land. The OS toast in production still fires regardless.
        persist_inbox_row(&db, &n, &snapshot);

        let conn = db.lock().unwrap();
        let rows = inbox_store::list(&conn, None, None).unwrap();
        assert!(rows.is_empty(), "FK miss must drop the row, not panic");
    }
}
