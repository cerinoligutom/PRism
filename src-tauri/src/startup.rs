//! Startup-failure surface (issue #239).
//!
//! `lib.rs::run` no longer terminates with `.expect(...)` when Tauri itself
//! or the setup hook returns an error. Instead the error is fed through this
//! module which:
//!
//! * formats the error into a human-readable body (chain walked, plain ASCII);
//! * writes that body to a platform-specific startup log (`startup.log`) so a
//!   non-developer user has something concrete to attach to a bug report;
//! * shows a native `rfd::MessageDialog` titled "PRism failed to start".
//!
//! The dialog uses `rfd` rather than `tauri-plugin-dialog` because the plugin
//! requires the Tauri runtime to be live (it hangs off `AppHandle`) - which is
//! exactly the precondition we cannot assume on a startup error. `rfd`'s
//! synchronous `MessageDialog::show()` works without any Tauri state and falls
//! through to the host OS's native modal.
//!
//! On Linux this requires GTK3 (already a transitive Tauri 2 dep) and the
//! `gtk3` rfd feature; XDG Desktop Portal has no message-dialog API so the
//! portal backend is intentionally not used.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

const DIALOG_TITLE: &str = "PRism failed to start";
const LOG_DIR_NAME: &str = "PRism";
const LOG_FILE_NAME: &str = "startup.log";

/// Walk an error and its `source()` chain into a plain-text body suitable for
/// both the native dialog and the on-disk log.
///
/// The format is:
///
/// ```text
/// <top-level message>
///
/// Caused by:
///   1. <first source>
///   2. <second source>
///   ...
/// ```
///
/// When there is no `source` chain the "Caused by" block is omitted.
pub fn format_error(err: &(dyn Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut sources = Vec::new();
    let mut current = err.source();
    while let Some(src) = current {
        sources.push(src.to_string());
        current = src.source();
    }
    if !sources.is_empty() {
        out.push_str("\n\nCaused by:");
        for (idx, msg) in sources.iter().enumerate() {
            out.push_str(&format!("\n  {}. {msg}", idx + 1));
        }
    }
    out
}

/// Resolve the on-disk location for `startup.log` on the host platform.
///
/// * Linux: `$XDG_CACHE_HOME/PRism/startup.log` (falls back to `$HOME/.cache/PRism/startup.log`).
/// * macOS: `$HOME/Library/Logs/PRism/startup.log`.
/// * Windows: `%LOCALAPPDATA%\PRism\Logs\startup.log`.
///
/// Returns `None` when neither the platform-specific nor the `HOME` fallback
/// env var is set - the caller treats that as "skip the log write" rather than
/// failing the dialog.
pub fn resolve_log_path() -> Option<PathBuf> {
    let dir = resolve_log_dir()?;
    Some(dir.join(LOG_FILE_NAME))
}

fn resolve_log_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        let mut path = PathBuf::from(home);
        path.push("Library");
        path.push("Logs");
        path.push(LOG_DIR_NAME);
        Some(path)
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("LOCALAPPDATA")?;
        let mut path = PathBuf::from(base);
        path.push(LOG_DIR_NAME);
        path.push("Logs");
        Some(path)
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            let mut path = PathBuf::from(xdg);
            path.push(LOG_DIR_NAME);
            return Some(path);
        }
        let home = std::env::var_os("HOME")?;
        let mut path = PathBuf::from(home);
        path.push(".cache");
        path.push(LOG_DIR_NAME);
        Some(path)
    }
}

/// Write the formatted body to the platform startup log.
///
/// Best-effort: any IO failure (no HOME, read-only disk, perms) is swallowed
/// after emitting to stderr. The dialog is the user-facing contract; the log
/// is a hand-off aid for the maintainer.
fn write_startup_log(body: &str) -> Option<PathBuf> {
    let path = resolve_log_path()?;
    if let Some(parent) = path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            tracing::error!(path = %parent.display(), %err, "startup: create log dir failed");
            return None;
        }
    }
    if let Err(err) = fs::write(&path, body) {
        tracing::error!(path = %path.display(), %err, "startup: write log failed");
        return None;
    }
    Some(path)
}

/// Show a native error dialog. Returns once the user dismisses it.
fn show_failure_dialog(body: &str) {
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(DIALOG_TITLE)
        .set_description(body)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Format the error, write it to the startup log, and surface a native
/// dialog. The dialog blocks the calling thread until dismissed.
///
/// Call from `lib.rs` on either a setup-hook error (re-raised through the
/// builder) or a `tauri::Builder::run` error. Either way, the caller exits
/// after this returns.
pub fn report_failure(err: &(dyn Error + 'static)) {
    let body = format_error(err);
    tracing::error!(%body, "PRism failed to start");
    let dialog_body = match write_startup_log(&body) {
        Some(path) => format!("{body}\n\nDetails written to:\n  {}", path.display()),
        None => body,
    };
    show_failure_dialog(&dialog_body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct StubError {
        message: String,
        source: Option<Box<StubError>>,
    }

    impl StubError {
        fn leaf(msg: &str) -> Self {
            Self {
                message: msg.to_string(),
                source: None,
            }
        }

        fn wrap(msg: &str, source: StubError) -> Self {
            Self {
                message: msg.to_string(),
                source: Some(Box::new(source)),
            }
        }
    }

    impl fmt::Display for StubError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl Error for StubError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source.as_deref().map(|e| e as &(dyn Error + 'static))
        }
    }

    #[test]
    fn format_error_returns_message_when_no_source() {
        let err = StubError::leaf("db locked");
        assert_eq!(format_error(&err), "db locked");
    }

    #[test]
    fn format_error_appends_numbered_source_chain() {
        let inner = StubError::leaf("database is locked");
        let middle = StubError::wrap("open sqlite db", inner);
        let outer = StubError::wrap("init db", middle);

        let formatted = format_error(&outer);

        assert_eq!(
            formatted,
            "init db\n\nCaused by:\n  1. open sqlite db\n  2. database is locked"
        );
    }

    #[test]
    fn format_error_walks_single_source() {
        let inner = StubError::leaf("permission denied");
        let outer = StubError::wrap("create app data dir", inner);

        let formatted = format_error(&outer);

        assert_eq!(
            formatted,
            "create app data dir\n\nCaused by:\n  1. permission denied"
        );
    }
}
