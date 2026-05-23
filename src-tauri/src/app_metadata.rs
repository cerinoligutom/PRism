//! Build-time metadata surface for the in-app "About" panel and the
//! StatusBar version pill. ADR-0022 documents the canonical-version +
//! sync-script + build.rs pipeline that produces these values.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AppMetadata {
    /// SemVer string, e.g. "0.1.0". Cargo populates this from `Cargo.toml`
    /// at compile time; `scripts/bump-version.ts` keeps `Cargo.toml`,
    /// `package.json`, and `tauri.conf.json` aligned.
    pub version: String,
    /// First 6 chars of the build commit, or "unknown" when git was
    /// unavailable at compile time (source tarball install, no `.git/`).
    pub commit_sha: String,
    /// UTC RFC 3339 timestamp at compile time, or "unknown" if the clock
    /// read failed (defensive - the time crate's `now_utc()` doesn't fail
    /// on any supported platform).
    pub build_date: String,
    /// "release" or "debug" (or "unknown" if cargo failed to set PROFILE).
    pub profile: String,
    /// Host OS short name from `std::env::consts::OS`
    /// ("macos", "windows", "linux", ...).
    pub os: String,
    /// Host CPU arch from `std::env::consts::ARCH`
    /// ("x86_64", "aarch64", ...).
    pub arch: String,
}

impl AppMetadata {
    /// Read every field at call time. Cheap - the env vars are baked at
    /// compile time and `std::env::consts` is `&'static str`, so this is
    /// pure const access with a few `String` allocations.
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit_sha: env!("PRISM_GIT_SHA").to_string(),
            build_date: env!("PRISM_BUILD_DATE").to_string(),
            profile: env!("PRISM_PROFILE").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }
}

/// Tauri command consumed by the frontend `useAppMetadata` composable.
/// Async so the renderer can `await` without thinking; the body is purely
/// synchronous reads against baked-in consts, so there is no yield point.
#[tauri::command]
pub async fn get_app_metadata() -> AppMetadata {
    AppMetadata::current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_populates_every_field() {
        let m = AppMetadata::current();
        // `version` resolves to CARGO_PKG_VERSION, which Cargo populates
        // from `Cargo.toml` - never empty in any real build.
        assert!(!m.version.is_empty(), "version must be populated");
        // commit_sha + build_date + profile come from the build.rs surface;
        // when git is missing they fall back to "unknown" (still non-empty).
        assert!(!m.commit_sha.is_empty(), "commit_sha must be populated");
        assert!(!m.build_date.is_empty(), "build_date must be populated");
        assert!(!m.profile.is_empty(), "profile must be populated");
        assert!(!m.os.is_empty(), "os must be populated");
        assert!(!m.arch.is_empty(), "arch must be populated");
    }

    #[test]
    fn serializes_with_snake_case_field_names() {
        // Frontend bindings (ADR-0021, manual through v1) consume the same
        // snake_case names the Rust struct declares. AppSettings + the
        // dashboard surfaces use the same convention.
        let m = AppMetadata {
            version: "0.1.0".to_string(),
            commit_sha: "abc123".to_string(),
            build_date: "2026-05-23T12:34:56Z".to_string(),
            profile: "release".to_string(),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
        };
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(v["version"], "0.1.0");
        assert_eq!(v["commit_sha"], "abc123");
        assert_eq!(v["build_date"], "2026-05-23T12:34:56Z");
        assert_eq!(v["profile"], "release");
        assert_eq!(v["os"], "macos");
        assert_eq!(v["arch"], "aarch64");
    }
}
