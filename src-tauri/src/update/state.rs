//! Shared updater state stashed in Tauri-managed state.
//!
//! Two pieces of data live here:
//!
//! * `pending`: the most recently observed [`PendingUpdate`] (version +
//!   release notes), if any. Updated by the worker on every successful
//!   check that returns `Some(update)`; cleared when the user starts an
//!   install (the next launch is the new binary). The Settings panel and
//!   the in-app banner both read from this slot.
//! * `install_on_quit`: a flag that the window-close hook in `lib.rs`
//!   consults. When set, the close handler downloads + installs before
//!   exit; otherwise quit is unmodified.
//!
//! Plain `Mutex<...>` is fine for both fields - the contended write paths
//! are bounded by user gestures (a button click) and the 6h poll, so the
//! lock cost never dominates.

use std::sync::{Arc, Mutex};

/// Most recent update record the worker reported. Cleared once an install
/// kicks off so a stale "available" badge can't linger after the user
/// pressed install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUpdate {
    pub version: String,
    pub release_notes: Option<String>,
}

/// Tauri-managed handle to the updater state. Cheap to clone.
pub type UpdateStateHandle = Arc<UpdateState>;

#[derive(Debug, Default)]
pub struct UpdateState {
    pending: Mutex<Option<PendingUpdate>>,
    install_on_quit: Mutex<bool>,
}

impl UpdateState {
    pub fn new() -> UpdateStateHandle {
        Arc::new(Self::default())
    }

    /// Replace the pending-update slot. `None` clears the slot.
    pub fn set_pending(&self, update: Option<PendingUpdate>) {
        let mut guard = self
            .pending
            .lock()
            .expect("update state pending mutex poisoned");
        *guard = update;
    }

    /// Snapshot the pending-update slot.
    pub fn pending(&self) -> Option<PendingUpdate> {
        self.pending
            .lock()
            .expect("update state pending mutex poisoned")
            .clone()
    }

    /// Set or clear the install-on-quit flag. The window-close hook reads
    /// this on the main window's `CloseRequested` event.
    pub fn set_install_on_quit(&self, flag: bool) {
        let mut guard = self
            .install_on_quit
            .lock()
            .expect("update state install_on_quit mutex poisoned");
        *guard = flag;
    }

    pub fn install_on_quit(&self) -> bool {
        *self
            .install_on_quit
            .lock()
            .expect("update state install_on_quit mutex poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_starts_empty() {
        let state = UpdateState::new();
        assert_eq!(state.pending(), None);
    }

    #[test]
    fn set_pending_then_snapshot_round_trips() {
        let state = UpdateState::new();
        let update = PendingUpdate {
            version: "1.2.3".into(),
            release_notes: Some("changes".into()),
        };
        state.set_pending(Some(update.clone()));
        assert_eq!(state.pending(), Some(update));
    }

    #[test]
    fn set_pending_none_clears_slot() {
        let state = UpdateState::new();
        state.set_pending(Some(PendingUpdate {
            version: "1.2.3".into(),
            release_notes: None,
        }));
        state.set_pending(None);
        assert_eq!(state.pending(), None);
    }

    #[test]
    fn install_on_quit_defaults_false() {
        let state = UpdateState::new();
        assert!(!state.install_on_quit());
    }

    #[test]
    fn install_on_quit_toggle_round_trips() {
        let state = UpdateState::new();
        state.set_install_on_quit(true);
        assert!(state.install_on_quit());
        state.set_install_on_quit(false);
        assert!(!state.install_on_quit());
    }
}
