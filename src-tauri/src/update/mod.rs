//! Auto-update subsystem (ADR-0024).
//!
//! Consumes the manifest published by the `update-manifest.yml` workflow on
//! the `release: published` event and surfaces update availability through
//! the Tauri command surface in [`commands`]. The [`worker`] runs a 6h
//! interval check while the user has opted in via the Settings -> Updates
//! toggle.
//!
//! Module shape:
//!
//! * [`commands`] - Tauri commands (`check_for_update_now`,
//!   `install_update_now`, `install_update_on_quit`).
//! * [`worker`] - the background interval task that hydrates its cadence
//!   from `app_settings` (mirroring `sync::read_persisted_interval`) and
//!   records every check via `record_update_check`.
//! * [`state`] - shared Tauri-managed state holding the install-on-quit
//!   flag plus the most recent update record.
//!
//! Failures are persisted to the singleton row's
//! `auto_update_last_failure_message` column and surfaced only by the
//! Settings -> Updates panel - no toast, no banner. The user-visible
//! "update available" banner sits in the renderer, fed by the events the
//! worker emits when a check succeeds and an update is reported.

pub mod commands;
pub mod state;
pub mod worker;

pub use state::{PendingUpdate, UpdateState, UpdateStateHandle};
pub use worker::{spawn_worker, UpdateWorker, UpdateWorkerHandle};
