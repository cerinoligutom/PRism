//! Pending notification payload queue for the click-to-open contract
//! (ADR 0017 decision 4, issue #201).
//!
//! ## Why a queue
//!
//! `tauri-plugin-notification` v2.3.3 ships a desktop builder that returns
//! `Result<()>` from `show()`. There's no `NotificationHandle`, no
//! `on_action` callback, and the underlying `notify-rust` `wait_for_action`
//! API is only wired for XDG (Linux) per its support matrix. The plugin's
//! global init also doesn't take a click hook. There's no way to register a
//! per-toast or global click handler through the v2.3.3 API on macOS or
//! Windows.
//!
//! What the OS does do reliably on every desktop platform: clicking a toast
//! activates the originating app (focus event on the main window). So the
//! sink enqueues each dispatched payload here, and a `WindowEvent::Focused`
//! hook in `lib.rs` drains the queue and emits `notification://open-pr` for
//! each entry. The frontend's `useNotificationRouter` composable listens on
//! the event and routes.
//!
//! ## False-positive bounding
//!
//! Focus events also fire when the user clicks the dock icon, alt-tabs in,
//! or returns from another window. To bound the "you didn't actually click a
//! notification but we routed anyway" window, each pending entry carries an
//! enqueue timestamp; [`PendingPayloadQueue::drain_fresh`] drops entries
//! older than [`PENDING_TTL`] before returning. A 30-second TTL covers the
//! typical "see the toast, switch contexts, click" sequence while keeping a
//! reasonable cap on stale fires.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Maximum age a pending payload can have before [`PendingPayloadQueue::drain_fresh`]
/// drops it. A focus event landing past this window won't replay the deep
/// link - the user almost certainly returned to the app for an unrelated
/// reason.
const PENDING_TTL: Duration = Duration::from_secs(30);

/// One queued payload waiting on a window-focus event to fire its deep link.
#[derive(Debug, Clone)]
struct PendingEntry {
    payload: serde_json::Value,
    enqueued_at: Instant,
}

/// Thread-safe queue of payloads awaiting their window-focus replay.
///
/// Shared between the [`super::runtime::TauriNotificationSink`] (enqueues on
/// every dispatched toast) and the `lib.rs` window-event hook (drains on
/// focus). Cloning the [`PendingPayloadQueueHandle`] is cheap (`Arc` clone).
#[derive(Debug, Default)]
pub struct PendingPayloadQueue {
    inner: Mutex<VecDeque<PendingEntry>>,
}

/// Tauri-managed handle to the shared [`PendingPayloadQueue`].
pub type PendingPayloadQueueHandle = Arc<PendingPayloadQueue>;

impl PendingPayloadQueue {
    /// Build an empty queue wrapped for `Arc`-sharing.
    pub fn new() -> PendingPayloadQueueHandle {
        Arc::new(Self::default())
    }

    /// Enqueue a payload to be replayed on the next window-focus event.
    /// Tagged with a wall-clock timestamp for the TTL check.
    pub fn enqueue(&self, payload: serde_json::Value) {
        let entry = PendingEntry {
            payload,
            enqueued_at: Instant::now(),
        };
        if let Ok(mut guard) = self.inner.lock() {
            guard.push_back(entry);
        }
    }

    /// Drain every payload whose age is within [`PENDING_TTL`]; drop the rest.
    /// Returns the surviving payloads in enqueue order.
    pub fn drain_fresh(&self) -> Vec<serde_json::Value> {
        self.drain_fresh_at(Instant::now())
    }

    /// TTL-driven drain against an explicit "now". Tests inject a controlled
    /// reference instant so the staleness check is deterministic.
    fn drain_fresh_at(&self, now: Instant) -> Vec<serde_json::Value> {
        let Ok(mut guard) = self.inner.lock() else {
            return Vec::new();
        };
        let mut fresh = Vec::with_capacity(guard.len());
        while let Some(entry) = guard.pop_front() {
            if now.duration_since(entry.enqueued_at) <= PENDING_TTL {
                fresh.push(entry.payload);
            }
        }
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enqueue_and_drain_returns_payloads_in_order() {
        let queue = PendingPayloadQueue::new();
        queue.enqueue(json!({ "account_id": 1, "pull_request_id": 100 }));
        queue.enqueue(json!({ "account_id": 2, "pull_request_id": 200 }));

        let drained = queue.drain_fresh();

        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0]["pull_request_id"], 100);
        assert_eq!(drained[1]["pull_request_id"], 200);
    }

    #[test]
    fn drain_empties_the_queue() {
        let queue = PendingPayloadQueue::new();
        queue.enqueue(json!({ "account_id": 1, "pull_request_id": 100 }));

        let first = queue.drain_fresh();
        let second = queue.drain_fresh();

        assert_eq!(first.len(), 1);
        assert!(second.is_empty(), "drain must empty the queue");
    }

    #[test]
    fn stale_payloads_are_dropped_past_the_ttl() {
        let queue = PendingPayloadQueue::new();
        queue.enqueue(json!({ "account_id": 1, "pull_request_id": 100 }));

        let future = Instant::now() + PENDING_TTL + Duration::from_secs(1);
        let drained = queue.drain_fresh_at(future);

        assert!(drained.is_empty(), "stale entries must be dropped");
    }

    #[test]
    fn fresh_entries_survive_when_some_are_stale() {
        let queue = PendingPayloadQueue::new();
        // The stale entry was enqueued "now"; we drain at "now + TTL + epsilon"
        // to force the staleness check. The follow-up enqueue lands inside the
        // TTL window relative to the drain reference.
        queue.enqueue(json!({ "stale": true }));
        let drain_at = Instant::now() + PENDING_TTL + Duration::from_secs(1);
        // Manually splice a fresh entry whose `enqueued_at` is at the drain
        // reference, so it survives the TTL check while the earlier entry
        // doesn't. Using the public API would also work but the TTL is long
        // enough that sleeping the test for it is not viable.
        {
            let mut guard = queue.inner.lock().unwrap();
            guard.push_back(PendingEntry {
                payload: json!({ "fresh": true }),
                enqueued_at: drain_at,
            });
        }

        let drained = queue.drain_fresh_at(drain_at);

        assert_eq!(drained.len(), 1, "only the fresh entry survives");
        assert_eq!(drained[0]["fresh"], true);
    }
}
