//! App-wide settings, currently scoped to M6 notification preferences.
//!
//! The schema is a singleton row in `app_settings` pinned at `id = 1` via a
//! CHECK constraint (see migration 0012). Reads always target that row; writes
//! UPDATE it in place. The settings table is intentionally narrow - one column
//! per toggle - so the typed reader doesn't need a JSON parse. ADR 0017
//! records the shape and the rationale.
//!
//! M6 lands the type definitions and the schema-load helper in this module.
//! The Tauri commands (`get_app_settings` / `update_app_settings`) and the
//! `NotificationSink` plumbing land in the follow-up issues (#191, #192).

pub mod types;

pub use types::{AppSettings, NotificationPermissionState};
