//! Background sync subsystem.
//!
//! - [`status_timeline`] is the pure derivation function for "latest status
//!   change" (issue #14).
//! - [`scheduler`] holds the poll-interval config (ADR 0004).
//! - [`state`] is the shared per-account state map the worker writes and the
//!   commands read.
//! - [`events`] declares the Tauri event names + payload shapes.
//! - [`worker`] is the per-account polling loop (issue #13).
//! - [`commands`] are the Tauri commands the frontend invokes.

pub mod commands;
pub mod events;
pub mod scheduler;
pub mod state;
pub mod status_timeline;
pub mod worker;

pub use commands::{
    get_sync_status, refresh_now, set_sync_interval, RefreshNowInput, RefreshNowResult,
    SetIntervalInput, SetIntervalResult, SyncStatusSnapshot,
};
pub use events::{
    SyncErrorPayload, SyncRateLimitPayload, SyncStatusPayload, SYNC_ERROR_EVENT,
    SYNC_RATE_LIMIT_EVENT, SYNC_STATUS_EVENT,
};
pub use scheduler::{
    SchedulerConfig, DEFAULT_INTERVAL_SECS, MAX_INTERVAL_SECS, MIN_INTERVAL_SECS,
    RATE_BUDGET_GUARD_PCT,
};
pub use state::{AccountSyncState, SyncPhase, SyncStateMap};
pub use status_timeline::{
    latest_status_change, LatestStatusChange, QualifyingEvent, TimelineEvent,
};
pub use worker::{
    spawn_worker, AppHandleEmitter, AppHandleReauth, ClientFactory, CycleOutcome,
    DefaultClientFactory, EmitSink, ReauthNotifier, SkipReason, SyncCycleReport, WorkerContext,
    WorkerHandle,
};
