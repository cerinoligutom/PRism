//! Background sync subsystem.
//!
//! - [`status_timeline`] is the pure derivation function for "latest status
//!   change" (issue #14).
//! - [`scheduler`] holds the poll-interval config (ADR 0004).
//! - [`state`] is the shared per-account state map the worker writes and the
//!   commands read.
//! - [`events`] declares the Tauri event names + payload shapes.
//! - [`activity`] is the diagnostic event buffer that backs the status-bar
//!   activity panel (issue #122).
//! - [`worker`] is the per-account polling loop (issue #13).
//! - [`discovery`] runs the Search-API discovery phase that seeds the four
//!   sidebar views (ADR 0009, issue #37).
//! - [`commands`] are the Tauri commands the frontend invokes.

pub mod activity;
pub mod commands;
pub mod discovery;
pub mod events;
pub mod scheduler;
pub mod state;
pub mod status_timeline;
pub mod worker;

pub use activity::{
    new_buffer as new_activity_buffer, record as record_activity, ActivityBuffer, ActivityEvent,
    ActivityEventBuilder, ActivityKind, ActivityLevel, SyncPhaseLabel, BUFFER_CAP,
};
pub use commands::{
    get_sync_status, list_recent_activity, refresh_now, set_sync_interval, ListRecentActivityInput,
    RefreshNowInput, RefreshNowResult, SetIntervalInput, SetIntervalResult, SyncStatusSnapshot,
};
pub use discovery::{
    discover_account, prune_stale_relations_for_account, DiscoveredPr, DiscoveryError,
    DiscoveryRelation, DiscoveryReport,
};
pub use events::{
    SyncErrorPayload, SyncRateLimitPayload, SyncStatusPayload, SYNC_ACTIVITY_EVENT,
    SYNC_ERROR_EVENT, SYNC_RATE_LIMIT_EVENT, SYNC_STATUS_EVENT,
};
pub use scheduler::{
    clamp_interval_secs, read_persisted_interval, write_persisted_interval, SchedulerConfig,
    DEFAULT_INTERVAL_SECS, MANUAL_INTERVAL_SECS, MAX_INTERVAL_SECS, MIN_INTERVAL_SECS,
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
