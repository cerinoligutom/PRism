import { ref, type Ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/**
 * Mirror of `crate::app_metadata::AppMetadata` in the Rust backend.
 * snake_case via the default Serialize impl, matching the convention used by
 * `AppSettings` and the dashboard surfaces. ADR-0022 documents how each
 * field is sourced (Cargo / build.rs / std::env::consts).
 */
export interface AppMetadata {
  readonly version: string;
  /** First 6 chars of the build commit, or "unknown" when git was absent. */
  readonly commit_sha: string;
  /** UTC RFC 3339 timestamp at compile time, or "unknown" on clock failure. */
  readonly build_date: string;
  /** "release" or "debug" (or "unknown" when cargo didn't set PROFILE). */
  readonly profile: string;
  /** Host OS short name ("macos", "windows", "linux", ...). */
  readonly os: string;
  /** Host CPU arch ("x86_64", "aarch64", ...). */
  readonly arch: string;
}

// Module-level singleton: build metadata never changes during the session,
// so the StatusBar pill + the About panel share one fetch.
const metadata = ref<AppMetadata | null>(null);
let inFlight: Promise<void> | null = null;

function load(): Promise<void> {
  if (inFlight !== null) return inFlight;
  inFlight = (async () => {
    try {
      metadata.value = await invoke<AppMetadata>("get_app_metadata");
    } catch (err) {
      // Tauri unreachable (Vite-in-browser preview, transport error). Leaving
      // `metadata` null lets consumers render placeholder copy without
      // crashing the page.
      console.warn("get_app_metadata failed:", err);
    }
  })();
  return inFlight;
}

/**
 * Reactive accessor for the running build's metadata. Resolves on first call
 * and caches for the rest of the session. Consumers should guard on `null`
 * to handle the not-yet-resolved + Vite-browser cases.
 */
export function useAppMetadata(): { metadata: Ref<AppMetadata | null> } {
  if (metadata.value === null && inFlight === null) {
    void load();
  }
  return { metadata };
}
