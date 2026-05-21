//! Diagnostic activity feed for the sync subsystem (issue #122).
//!
//! `record(buffer, emit, event)` is the single entry point: it appends to a
//! capped in-memory ring buffer AND fires the `sync://activity` Tauri event
//! with the same payload. The frontend's `syncActivity` Pinia store subscribes
//! to that event for live updates and calls `list_recent_activity` on init to
//! hydrate the rolling window.
//!
//! The buffer is in-memory only. State doesn't survive a restart, but that
//! matches what the user expects from a "live cycle" feed - cycle history is
//! diagnostic context for a session, not a persisted log. SQLite persistence
//! is deferred until user demand surfaces.
//!
//! Event IDs are a monotonically increasing `u64` so the frontend can dedupe
//! cleanly and a future "jump to new events" affordance has a stable key.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::github::AccountId;
use crate::sync::events::SYNC_ACTIVITY_EVENT;
use crate::sync::worker::EmitSink;

/// Maximum number of events held in the rolling buffer. Older events are
/// evicted FIFO once the cap is reached. Sized for one or two recent cycles
/// against a typical PRism account (up to ~100 PRs enriched per cycle).
pub const BUFFER_CAP: usize = 200;

/// Severity. Drives the icon + filter chip in the activity panel; mapping is
/// `info` -> default, `warn` -> amber, `error` -> red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityLevel {
    Info,
    Warn,
    Error,
}

/// Structured event payload. The discriminated `kind` field lets the frontend
/// filter and deep-link without parsing `message`; the pre-rendered `message`
/// field is what the panel actually renders in the row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActivityKind {
    /// A new sync cycle began for an account.
    CycleStarted,
    /// A phase boundary inside a cycle - discovery / enrichment / pruning.
    PhaseStarted { phase: SyncPhaseLabel },
    /// Progress within a phase. `current` / `total` are 1-based.
    PhaseProgress {
        phase: SyncPhaseLabel,
        current: u32,
        total: u32,
    },
    /// One PR's detail successfully fetched. Surfaces a deep-link button.
    PrFetched {
        number: i64,
        owner: String,
        name: String,
        url: String,
    },
    /// A phase ended cleanly.
    PhaseCompleted {
        phase: SyncPhaseLabel,
        summary: String,
    },
    /// Cycle finished successfully.
    CycleCompleted { prs_visited: u32, summary: String },
    /// Cycle failed. `error_message` is the underlying error (truncated) and
    /// `error_kind` is a short categorisation (`discovery` / `enrichment` /
    /// `pruning` / `client_build`).
    CycleFailed {
        error_message: String,
        error_kind: String,
    },
    /// Rate limit guard paused the cycle.
    RateLimitPause { reset_in_seconds: u64 },
}

/// Phase label used by the activity feed.
///
/// Distinct from `SyncPhase` because the feed cares about cycle sub-phases
/// (discovery / enrichment / pruning), whereas `SyncPhase` tracks the
/// account's overall state (idle / syncing / synced / error / ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhaseLabel {
    Discovery,
    Enrichment,
    Pruning,
}

impl SyncPhaseLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Enrichment => "enrichment",
            Self::Pruning => "pruning",
        }
    }
}

/// One ring-buffer entry. `id` is monotonically increasing per process so the
/// frontend can dedupe. `timestamp_ms` is unix milliseconds.
///
/// `kind` is flattened so the JSON carries the discriminator (`"kind":
/// "cycle_started"`) and any variant-specific fields alongside the wrapper
/// fields — the frontend reads `event.kind` directly without unwrapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub id: u64,
    pub timestamp_ms: i64,
    pub level: ActivityLevel,
    pub account_id: Option<AccountId>,
    /// Pre-rendered, human-readable line for the panel row. Built at record
    /// time so the frontend doesn't have to mirror the rendering rules.
    pub message: String,
    #[serde(flatten)]
    pub kind: ActivityKind,
}

/// Shared activity buffer handle. Cloneable; the Mutex protects the queue.
pub type ActivityBuffer = Arc<Mutex<VecDeque<ActivityEvent>>>;

/// Build a fresh buffer with capacity hinted at `BUFFER_CAP`.
pub fn new_buffer() -> ActivityBuffer {
    Arc::new(Mutex::new(VecDeque::with_capacity(BUFFER_CAP)))
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Builder helpers for `ActivityEvent`. Avoids 7-arg call sites at the worker
/// instrumentation points.
pub struct ActivityEventBuilder {
    pub account_id: Option<AccountId>,
    pub level: ActivityLevel,
    pub kind: ActivityKind,
    pub message: String,
}

impl ActivityEventBuilder {
    pub fn new(
        level: ActivityLevel,
        account_id: Option<AccountId>,
        kind: ActivityKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            account_id,
            level,
            kind,
            message: message.into(),
        }
    }

    pub fn build(self) -> ActivityEvent {
        ActivityEvent {
            id: next_id(),
            timestamp_ms: unix_millis(),
            level: self.level,
            account_id: self.account_id,
            kind: self.kind,
            message: self.message,
        }
    }
}

/// Push an event into the buffer (evicting oldest when full) and emit it on
/// the `sync://activity` Tauri event channel.
///
/// Emission failures are swallowed inside the `EmitSink` impl - the buffer
/// already holds the event, so the panel hydrate path will surface it on the
/// next call to `list_recent_activity`.
pub fn record<E: EmitSink + ?Sized>(buffer: &ActivityBuffer, emit: &E, event: ActivityEvent) {
    {
        let mut guard = buffer.lock().expect("activity buffer poisoned");
        if guard.len() >= BUFFER_CAP {
            guard.pop_front();
        }
        guard.push_back(event.clone());
    }
    let payload = serde_json::to_value(&event).unwrap_or(Value::Null);
    emit.emit(SYNC_ACTIVITY_EVENT, &payload);
}

/// Snapshot the most-recent-first slice of the buffer, optionally filtered
/// by `account_id`. `limit` is applied after filtering.
pub fn snapshot(
    buffer: &ActivityBuffer,
    limit: usize,
    account_id: Option<AccountId>,
) -> Vec<ActivityEvent> {
    let guard = match buffer.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    guard
        .iter()
        .rev()
        .filter(|e| match account_id {
            Some(id) => e.account_id == Some(id),
            None => true,
        })
        .take(limit)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct VecSink {
        emitted: StdMutex<Vec<(String, Value)>>,
    }

    impl EmitSink for VecSink {
        fn emit(&self, event: &str, payload: &Value) {
            self.emitted
                .lock()
                .unwrap()
                .push((event.to_string(), payload.clone()));
        }
    }

    fn make_event(account_id: Option<AccountId>, msg: &str) -> ActivityEvent {
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            account_id,
            ActivityKind::CycleStarted,
            msg,
        )
        .build()
    }

    #[test]
    fn record_appends_and_emits() {
        let buf = new_buffer();
        let sink = VecSink::default();
        record(&buf, &sink, make_event(Some(1), "hello"));
        assert_eq!(buf.lock().unwrap().len(), 1);
        let emitted = sink.emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].0, SYNC_ACTIVITY_EVENT);
    }

    #[test]
    fn buffer_evicts_oldest_at_cap() {
        let buf = new_buffer();
        let sink = VecSink::default();
        for i in 0..BUFFER_CAP + 5 {
            record(&buf, &sink, make_event(Some(1), &format!("e{i}")));
        }
        let guard = buf.lock().unwrap();
        assert_eq!(guard.len(), BUFFER_CAP, "capped at BUFFER_CAP");
        // Oldest entries are evicted: front should be event index 5 (0..4 dropped).
        assert_eq!(guard.front().unwrap().message, "e5");
        assert_eq!(
            guard.back().unwrap().message,
            format!("e{}", BUFFER_CAP + 4)
        );
    }

    #[test]
    fn snapshot_returns_most_recent_first() {
        let buf = new_buffer();
        let sink = VecSink::default();
        record(&buf, &sink, make_event(Some(1), "first"));
        record(&buf, &sink, make_event(Some(1), "second"));
        record(&buf, &sink, make_event(Some(1), "third"));
        let snap = snapshot(&buf, 10, None);
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].message, "third");
        assert_eq!(snap[1].message, "second");
        assert_eq!(snap[2].message, "first");
    }

    #[test]
    fn snapshot_respects_limit() {
        let buf = new_buffer();
        let sink = VecSink::default();
        for i in 0..10 {
            record(&buf, &sink, make_event(Some(1), &format!("e{i}")));
        }
        let snap = snapshot(&buf, 3, None);
        assert_eq!(snap.len(), 3);
        // Most recent three (e9, e8, e7).
        assert_eq!(snap[0].message, "e9");
        assert_eq!(snap[2].message, "e7");
    }

    #[test]
    fn snapshot_filters_by_account() {
        let buf = new_buffer();
        let sink = VecSink::default();
        record(&buf, &sink, make_event(Some(1), "a1"));
        record(&buf, &sink, make_event(Some(2), "b1"));
        record(&buf, &sink, make_event(Some(1), "a2"));
        record(&buf, &sink, make_event(None, "global"));

        let only_a = snapshot(&buf, 10, Some(1));
        assert_eq!(only_a.len(), 2);
        assert_eq!(only_a[0].message, "a2");
        assert_eq!(only_a[1].message, "a1");

        let only_b = snapshot(&buf, 10, Some(2));
        assert_eq!(only_b.len(), 1);
        assert_eq!(only_b[0].message, "b1");
    }

    #[test]
    fn event_ids_are_monotonically_increasing() {
        let buf = new_buffer();
        let sink = VecSink::default();
        record(&buf, &sink, make_event(None, "first"));
        record(&buf, &sink, make_event(None, "second"));
        let guard = buf.lock().unwrap();
        let ids: Vec<u64> = guard.iter().map(|e| e.id).collect();
        assert!(ids[1] > ids[0], "ids increase: {:?}", ids);
    }
}
