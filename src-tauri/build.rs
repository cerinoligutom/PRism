use std::process::Command;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn main() {
    // Tauri's own build hook stays first - it generates the Tauri context
    // that `tauri::generate_context!()` consumes.
    tauri_build::build();

    emit_build_metadata();
}

/// Bakes three env vars that the `app_metadata` module reads via `env!()`:
///
/// * `PRISM_GIT_SHA`    - first 6 chars of `git rev-parse HEAD`, or "unknown"
///   when git is unavailable (source tarball install, no `.git/` directory).
/// * `PRISM_BUILD_DATE` - UTC RFC 3339 timestamp at compile time, or "unknown"
///   when the clock read fails (extremely unlikely; emitted defensively).
/// * `PRISM_PROFILE`    - cargo's `PROFILE` env var ("debug" or "release").
fn emit_build_metadata() {
    let git_sha = git_short_sha().unwrap_or_else(|| "unknown".to_string());
    let build_date = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=PRISM_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=PRISM_BUILD_DATE={build_date}");
    println!("cargo:rustc-env=PRISM_PROFILE={profile}");

    // Re-run the build script when HEAD or any branch ref moves. This covers
    // branch switches and new commits on the current branch in the common
    // case; detached-HEAD release builds always run fresh on CI so the
    // best-effort hint is sufficient.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
}

fn git_short_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=6", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}
