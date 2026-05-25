//! OS notification dispatch pipeline.
//!
//! This module ships the plumbing layer for ADR 0017:
//!
//! * [`types::NotificationTrigger`] - the identity-only event the recompute
//!   emitter will produce in issue #192;
//! * [`types::Notification`] - the formatted dispatch unit;
//! * [`sink::NotificationSink`] - the narrow fire-and-forget trait the
//!   emitter talks to (mirrors `ReauthSink` in `sync/worker.rs:135`);
//! * [`sink::PermissionAsker`] - a tiny seam over the
//!   `tauri-plugin-notification` permission surface so the gating + persist
//!   flow is testable without a live Tauri runtime;
//! * [`runtime::TauriNotificationSink`] - production sink wired to the
//!   plugin, persisting permission state to `app_settings`;
//! * [`pending::PendingPayloadQueue`] - shared queue the sink uses to stage
//!   deep-link payloads ahead of the OS toast firing. The `lib.rs`
//!   window-event hook drains it on the next main-window focus event and
//!   emits `notification://open-pr` per entry, since the plugin's desktop
//!   v2.3.3 API exposes no per-notification or global click callback
//!   (ADR 0017 decision 4, issue #201).
//!
//! The macOS dock badge lives alongside the toast plumbing in [`badge`]:
//! same OS-signal surface area, same `AppHandle` dependency, so colocating
//! keeps the M6 notification module cohesive.

pub mod badge;
pub mod formatter;
pub mod pending;
pub mod runtime;
pub mod sink;
pub mod types;

pub use badge::{count_global_unread, refresh_from_db, update_badge, AppHandleBadge, BadgeSink};
pub use formatter::format_trigger;
pub use pending::{PendingPayloadQueue, PendingPayloadQueueHandle};
pub use runtime::{PluginPermissionAsker, TauriNotificationSink};
pub use sink::{NotificationSink, NotificationSinkHandle, PermissionAsker};
pub use types::{Notification, NotificationKind, NotificationSnapshot, NotificationTrigger};

#[cfg(test)]
mod tests {
    //! Tests for the gating + persistence flow.
    //!
    //! We exercise [`runtime::decide_dispatch`] directly (the same function
    //! `TauriNotificationSink::dispatch` calls into) so we can assert the
    //! state machine without booting Tauri. A test-only
    //! `NotificationSink` impl records dispatch calls when we want to assert
    //! that the trait surface stays narrow enough for a recording substitute
    //! to work end-to-end.
    //!
    //! ADR 0017 decision 5 is the contract these tests cover.
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use rusqlite::Connection;
    use serde_json::json;

    use super::runtime::decide_dispatch;
    use super::sink::{NotificationSink, PermissionAsker};
    use super::types::Notification;
    use crate::db::DbHandle;
    use crate::settings::{AppSettings, NotificationPermissionState};

    /// Test helper: open an in-memory DB with the v12 migration applied. The
    /// seeded `app_settings` row matches the production defaults
    /// (master ON, both triggers ON, permission Unprompted).
    fn fresh_db() -> DbHandle {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        Arc::new(Mutex::new(conn))
    }

    /// Test helper: UPDATE the singleton row in place. Mirrors the writer
    /// the future `update_app_settings` command will use, just inlined to
    /// keep the test setup self contained.
    fn write_settings(db: &DbHandle, settings: &AppSettings) {
        let conn = db.lock().unwrap();
        let perm = match settings.notification_permission_state {
            NotificationPermissionState::Unprompted => "unprompted",
            NotificationPermissionState::Granted => "granted",
            NotificationPermissionState::Denied => "denied",
        };
        conn.execute(
            "UPDATE app_settings
                SET notifications_enabled = ?1,
                    notify_on_needs_attention = ?2,
                    notify_on_mention = ?3,
                    notification_permission_state = ?4,
                    updated_at = strftime('%s', 'now')
              WHERE id = 1",
            rusqlite::params![
                settings.notifications_enabled as i64,
                settings.notify_on_needs_attention as i64,
                settings.notify_on_mention as i64,
                perm,
            ],
        )
        .unwrap();
    }

    /// Read the singleton row's `notification_permission_state` directly.
    fn read_perm(db: &DbHandle) -> NotificationPermissionState {
        let conn = db.lock().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT notification_permission_state FROM app_settings WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        NotificationPermissionState::from_storage(&raw)
    }

    /// `PermissionAsker` that records `request` invocations and returns a
    /// pre-programmed state. Cloning is cheap (Arc inside).
    struct ScriptedAsker {
        request_calls: AtomicUsize,
        current_calls: AtomicUsize,
        on_request: NotificationPermissionState,
    }

    impl ScriptedAsker {
        fn new(on_request: NotificationPermissionState) -> Self {
            Self {
                request_calls: AtomicUsize::new(0),
                current_calls: AtomicUsize::new(0),
                on_request,
            }
        }
    }

    impl PermissionAsker for ScriptedAsker {
        fn current(&self) -> NotificationPermissionState {
            self.current_calls.fetch_add(1, Ordering::SeqCst);
            NotificationPermissionState::Unprompted
        }
        fn request(&self) -> NotificationPermissionState {
            self.request_calls.fetch_add(1, Ordering::SeqCst);
            self.on_request
        }
    }

    /// In-process `NotificationSink` that records the notifications passed
    /// to `dispatch`. Used to confirm the trait surface stays narrow enough
    /// for the recompute emitter to be unit-tested against a fake sink in
    /// the follow-up issue (#192).
    #[derive(Default)]
    struct RecordingSink {
        dispatched: Mutex<Vec<Notification>>,
    }

    impl NotificationSink for RecordingSink {
        fn dispatch(&self, notification: &Notification) {
            self.dispatched.lock().unwrap().push(notification.clone());
        }
    }

    fn sample_notification() -> Notification {
        Notification {
            title: "alice opened PR #42".to_string(),
            body: "Mentioned you in a comment".to_string(),
            payload: json!({ "account_id": 1, "pull_request_id": 100 }),
            // The recording sink only asserts on title / body / payload, so
            // we leave the inbox snapshot empty here; persistence wiring is
            // covered by `notify::runtime` against a real DB.
            snapshot: None,
        }
    }

    #[test]
    fn dispatch_skipped_when_master_switch_off() {
        // ADR 0017 decision 5: master OFF means no dispatch, no prompt, no
        // mutation of the permission state. The user hasn't opted in.
        // The seeded master defaults to ON pre-v1, so we flip it OFF
        // explicitly here to set up the scenario under test.
        let db = fresh_db();
        write_settings(
            &db,
            &AppSettings {
                notifications_enabled: false,
                notify_on_needs_attention: true,
                notify_on_mention: true,
                notification_permission_state: NotificationPermissionState::Unprompted,
                last_seen_version: None,
                auto_update_enabled: false,
                auto_update_interval_seconds: 21600,
                auto_update_last_check_at: None,
                auto_update_last_failure_message: None,
                auto_archive_days: 30,
                notification_retention_max: 500,
                updated_at: 0,
            },
        );
        let asker = ScriptedAsker::new(NotificationPermissionState::Granted);

        let dispatched = decide_dispatch(&db, &asker);

        assert!(!dispatched, "master OFF must skip dispatch");
        assert_eq!(
            asker.request_calls.load(Ordering::SeqCst),
            0,
            "master OFF must not prompt"
        );
        assert_eq!(
            read_perm(&db),
            NotificationPermissionState::Unprompted,
            "permission state must not flip when the master is OFF"
        );
    }

    #[test]
    fn unprompted_triggers_prompt_and_persists_grant() {
        // ADR 0017 decision 5 happy path: master ON, OS answers Granted,
        // the state flips to Granted and the dispatch proceeds.
        let db = fresh_db();
        write_settings(
            &db,
            &AppSettings {
                notifications_enabled: true,
                notify_on_needs_attention: true,
                notify_on_mention: true,
                notification_permission_state: NotificationPermissionState::Unprompted,
                last_seen_version: None,
                auto_update_enabled: false,
                auto_update_interval_seconds: 21600,
                auto_update_last_check_at: None,
                auto_update_last_failure_message: None,
                auto_archive_days: 30,
                notification_retention_max: 500,
                updated_at: 0,
            },
        );
        let asker = ScriptedAsker::new(NotificationPermissionState::Granted);

        let dispatched = decide_dispatch(&db, &asker);

        assert!(dispatched, "granted prompt response must dispatch");
        assert_eq!(asker.request_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            read_perm(&db),
            NotificationPermissionState::Granted,
            "answered Granted must persist"
        );
    }

    #[test]
    fn unprompted_persists_denial_and_skips_dispatch() {
        // ADR 0017 decision 5: a denial during the deferred ask flips the
        // permission state so subsequent dispatches short-circuit without
        // re-prompting.
        let db = fresh_db();
        write_settings(
            &db,
            &AppSettings {
                notifications_enabled: true,
                notify_on_needs_attention: true,
                notify_on_mention: true,
                notification_permission_state: NotificationPermissionState::Unprompted,
                last_seen_version: None,
                auto_update_enabled: false,
                auto_update_interval_seconds: 21600,
                auto_update_last_check_at: None,
                auto_update_last_failure_message: None,
                auto_archive_days: 30,
                notification_retention_max: 500,
                updated_at: 0,
            },
        );
        let asker = ScriptedAsker::new(NotificationPermissionState::Denied);

        let dispatched = decide_dispatch(&db, &asker);

        assert!(!dispatched, "denied prompt response must skip dispatch");
        assert_eq!(asker.request_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            read_perm(&db),
            NotificationPermissionState::Denied,
            "answered Denied must persist"
        );
    }

    #[test]
    fn denied_state_skips_without_prompting() {
        // ADR 0017 decision 5: never re-prompt after denial. The Settings
        // panel renders the "blocked" callout; the sink quietly skips.
        let db = fresh_db();
        write_settings(
            &db,
            &AppSettings {
                notifications_enabled: true,
                notify_on_needs_attention: true,
                notify_on_mention: true,
                notification_permission_state: NotificationPermissionState::Denied,
                last_seen_version: None,
                auto_update_enabled: false,
                auto_update_interval_seconds: 21600,
                auto_update_last_check_at: None,
                auto_update_last_failure_message: None,
                auto_archive_days: 30,
                notification_retention_max: 500,
                updated_at: 0,
            },
        );
        let asker = ScriptedAsker::new(NotificationPermissionState::Granted);

        let dispatched = decide_dispatch(&db, &asker);

        assert!(!dispatched, "Denied state must short-circuit");
        assert_eq!(
            asker.request_calls.load(Ordering::SeqCst),
            0,
            "Denied state must not re-prompt"
        );
        assert_eq!(
            read_perm(&db),
            NotificationPermissionState::Denied,
            "Denied state must not flip"
        );
    }

    #[test]
    fn granted_state_dispatches_without_prompting() {
        // Steady-state path: once Granted is persisted, subsequent dispatches
        // skip the OS prompt and proceed straight to the toast.
        let db = fresh_db();
        write_settings(
            &db,
            &AppSettings {
                notifications_enabled: true,
                notify_on_needs_attention: true,
                notify_on_mention: true,
                notification_permission_state: NotificationPermissionState::Granted,
                last_seen_version: None,
                auto_update_enabled: false,
                auto_update_interval_seconds: 21600,
                auto_update_last_check_at: None,
                auto_update_last_failure_message: None,
                auto_archive_days: 30,
                notification_retention_max: 500,
                updated_at: 0,
            },
        );
        let asker = ScriptedAsker::new(NotificationPermissionState::Denied);

        let dispatched = decide_dispatch(&db, &asker);

        assert!(dispatched, "Granted must dispatch directly");
        assert_eq!(
            asker.request_calls.load(Ordering::SeqCst),
            0,
            "Granted must not re-prompt"
        );
    }

    #[test]
    fn recording_sink_captures_dispatched_notification() {
        // Confirms the trait surface is narrow enough for the future
        // recompute emitter (issue #192) to be unit-tested against a fake
        // sink. The Tauri-backed sink is exercised end-to-end at app run
        // time; here we only need the type-level guarantee.
        let sink = RecordingSink::default();
        let n = sample_notification();
        sink.dispatch(&n);
        let captured = sink.dispatched.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0], n);
    }
}
