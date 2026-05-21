//! Read-only SQL composition for the triage Tauri commands.
//!
//! Wave 1 lands the empty module shell. Wave 2-D (`list_filter_chip_counts`)
//! fills in the per-chip count queries against the existing
//! `pull_request_viewer_relations` + `pull_requests` join surface. The
//! per-PR write helpers (`mark_pr_read`, `mark_pr_unread`) live alongside
//! their command bodies in `commands.rs` rather than here because they
//! mutate state.
//!
//! See `docs/contracts/triage-ux.md` and ADR 0015 for the contract this
//! module implements.
