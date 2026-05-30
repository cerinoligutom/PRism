//! Notification dispatch: format the per-PR triggers produced by the post-write
//! triage recompute into [`crate::notify::Notification`] payloads and hand them
//! to the sink. The sink's [`crate::notify::runtime::decide_dispatch`] gates
//! every dispatch on the master switch, then the `notify_on_needs_attention`
//! toggle, then the OS permission state (ADR 0017 decision 5, ADR 0031); the
//! worker only formats and forwards.

use crate::db::DbHandle;
use crate::notify::{format_trigger, NotificationSinkHandle, NotificationTrigger};

/// Format every trigger against the current DB state and dispatch it to the
/// notification sink. Empty input is a fast no-op. A poisoned DB mutex is
/// logged and swallowed - notifications are advisory, and the next cycle's
/// recompute will re-derive the same triggers.
pub(super) fn dispatch_triggers(
    db: &DbHandle,
    sink: &NotificationSinkHandle,
    triggers: &[NotificationTrigger],
) {
    if triggers.is_empty() {
        return;
    }
    let formatted: Vec<_> = {
        let conn = match db.lock() {
            Ok(g) => g,
            Err(err) => {
                tracing::error!(%err, "notify dispatch: db poisoned");
                return;
            }
        };
        triggers
            .iter()
            .map(|t| (t, format_trigger(&conn, t)))
            .collect()
    };
    for (trigger, notification) in formatted {
        match notification {
            Some(n) => {
                tracing::debug!(
                    kind = ?trigger.kind,
                    account_id = trigger.account_id,
                    pull_request_id = trigger.pull_request_id,
                    "notify dispatch",
                );
                sink.dispatch(&n);
            }
            None => tracing::debug!(
                account_id = trigger.account_id,
                pull_request_id = trigger.pull_request_id,
                "notify dispatch: skipping, PR row missing",
            ),
        }
    }
}
