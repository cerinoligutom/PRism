//! Persistent notifications inbox (issue #378).
//!
//! Companion to [`crate::notify`]: where `notify` owns the OS toast pipeline
//! (ADR 0017), this module owns the in-app inbox row. Every dispatched toast
//! is mirrored here so a user who missed the transient OS notification can
//! recover from `/dashboard/notifications`.
//!
//! Split into four files:
//! * [`types`]  - the `Notification` row + `NotificationInsert` snapshot.
//! * [`store`]  - the SQL writers + readers used by both the dispatch hook
//!   and the Tauri command surface.
//! * [`commands`] - the renderer-facing `list_notifications`,
//!   `delete_notification`, `clear_all_notifications`.
//!
//! The dispatch hook lives in [`crate::notify::runtime`]; the sink inserts a
//! row via [`store::insert`] before the OS toast fires. An insert failure
//! logs and continues so a flaky DB never silences the OS toast.

pub mod commands;
pub mod store;
pub mod types;

pub use types::{Notification, NotificationInsert};
