import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { defineStore } from "pinia";
import { computed, ref } from "vue";

import { useAppSettings } from "@/stores/settings";

/**
 * Auto-update store. ADR-0024, issue #308. Backed by the Rust
 * `update::commands` surface and the events the worker emits on every
 * check.
 *
 * Two consumers share this store: the Settings -> Updates panel (toggle,
 * "Check now" button, "Install" actions) and the AppShell banner that
 * shows when an update is available. Keeping the state in one Pinia
 * store keeps both views in sync without prop-drilling.
 *
 * The settings toggle itself lives in [`useAppSettings`] - the boundary
 * for anything the Rust worker reads (ADR-0020). This store handles the
 * volatile, runtime-only update availability state.
 */

const UPDATE_AVAILABLE_EVENT = "update://available";
const UPDATE_CHECK_EVENT = "update://checked";

interface UpdateAvailablePayload {
  readonly version: string;
  readonly release_notes: string | null;
}

interface UpdateCheckPayload {
  readonly success: boolean;
  readonly failure_message: string | null;
}

interface CheckForUpdateResult {
  readonly updateAvailable: boolean;
  readonly version: string | null;
  readonly releaseNotes: string | null;
}

export const useAutoUpdate = defineStore("auto-update", () => {
  const settings = useAppSettings();

  const availableVersion = ref<string | null>(null);
  const releaseNotes = ref<string | null>(null);
  const bannerDismissed = ref(false);
  const checking = ref(false);
  const installing = ref(false);
  const lastCheckError = ref<string | null>(null);

  let unlistenAvailable: UnlistenFn | null = null;
  let unlistenChecked: UnlistenFn | null = null;
  let listening = false;

  const updateAvailable = computed(() => availableVersion.value !== null);
  const bannerVisible = computed(
    () => updateAvailable.value && !bannerDismissed.value,
  );

  /** Start listening for worker events. Idempotent. */
  async function bindListeners(): Promise<void> {
    if (listening) return;
    listening = true;
    try {
      unlistenAvailable = await listen<UpdateAvailablePayload>(
        UPDATE_AVAILABLE_EVENT,
        (event) => {
          availableVersion.value = event.payload.version;
          releaseNotes.value = event.payload.release_notes;
          // A fresh "available" signal resets the dismiss so the banner
          // re-surfaces on the new version even if the user dismissed an
          // older one.
          bannerDismissed.value = false;
        },
      );
      unlistenChecked = await listen<UpdateCheckPayload>(
        UPDATE_CHECK_EVENT,
        (event) => {
          // Reload the settings row so the panel's "last check" timestamp
          // + failure line stay live without a manual refresh.
          void settings.load();
          if (!event.payload.success) {
            lastCheckError.value = event.payload.failure_message;
          } else {
            lastCheckError.value = null;
          }
        },
      );
    } catch (err) {
      listening = false;
      console.warn("auto-update: listener bind failed", err);
    }
  }

  function unbindListeners(): void {
    unlistenAvailable?.();
    unlistenChecked?.();
    unlistenAvailable = null;
    unlistenChecked = null;
    listening = false;
  }

  /**
   * Manually trigger a check. Surfaces the result inline so the Settings
   * panel can render "You're up to date" or the inline error.
   */
  async function checkNow(): Promise<CheckForUpdateResult> {
    if (checking.value) {
      return {
        updateAvailable: updateAvailable.value,
        version: availableVersion.value,
        releaseNotes: releaseNotes.value,
      };
    }
    checking.value = true;
    lastCheckError.value = null;
    try {
      const result = await invoke<CheckForUpdateResult>(
        "check_for_update_now",
      );
      if (result.updateAvailable && result.version) {
        availableVersion.value = result.version;
        releaseNotes.value = result.releaseNotes;
        bannerDismissed.value = false;
      } else {
        availableVersion.value = null;
        releaseNotes.value = null;
      }
      // Pick up the new last-check timestamp the backend recorded.
      void settings.load();
      return result;
    } catch (err) {
      lastCheckError.value = formatError(err);
      throw new Error(lastCheckError.value);
    } finally {
      checking.value = false;
    }
  }

  /** Queue the install for the next quit. Returns when the flag is set. */
  async function installOnQuit(): Promise<void> {
    try {
      await invoke("install_update_on_quit");
      // The banner stays visible so the user has a path back to "Install
      // now" if they change their mind. The plugin will do the work on
      // the next CloseRequested event.
    } catch (err) {
      lastCheckError.value = formatError(err);
      throw new Error(lastCheckError.value);
    }
  }

  /**
   * Download + install + restart immediately. The happy path doesn't
   * return - the Rust side calls `app.restart()` and the process
   * terminates. Surface any error inline.
   */
  async function installNow(): Promise<void> {
    if (installing.value) return;
    installing.value = true;
    lastCheckError.value = null;
    try {
      await invoke("install_update_now");
      // Unreachable on the happy path; the app has restarted.
    } catch (err) {
      lastCheckError.value = formatError(err);
      throw new Error(lastCheckError.value);
    } finally {
      installing.value = false;
    }
  }

  function dismissBanner(): void {
    bannerDismissed.value = true;
  }

  return {
    availableVersion,
    releaseNotes,
    updateAvailable,
    bannerVisible,
    checking,
    installing,
    lastCheckError,
    bindListeners,
    unbindListeners,
    checkNow,
    installOnQuit,
    installNow,
    dismissBanner,
  };
});

function formatError(raw: unknown): string {
  if (typeof raw === "string") return raw;
  if (raw instanceof Error) return raw.message;
  return "The update check failed.";
}
