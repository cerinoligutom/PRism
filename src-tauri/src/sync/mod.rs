//! Background sync subsystem.
//!
//! The sync worker proper lands in issue #13. For now this module re-exports
//! the pure derivation function for "latest status change" timestamps, which
//! the worker will call once the REST timeline client (issue #12) feeds it
//! event payloads.

pub mod status_timeline;

pub use status_timeline::{
    latest_status_change, LatestStatusChange, QualifyingEvent, TimelineEvent,
};
