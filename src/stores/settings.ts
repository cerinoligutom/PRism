import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/**
 * Mirror of `crate::settings::types::NotificationPermissionState` in the Rust
 * backend. Serialised lowercase via `#[serde(rename_all = "lowercase")]`.
 *
 * `unprompted` - the OS has not been asked yet. Flipping the master switch ON
 *                triggers the prompt.
 * `granted`    - the OS granted permission; toasts may fire when triggers allow.
 * `denied`     - the OS denied permission. Master switch stays OFF and the
 *                Settings panel renders the "blocked" callout.
 */
export type NotificationPermissionState = "unprompted" | "granted" | "denied";

/**
 * Mirror of `crate::settings::types::AppSettings`. Read via `get_app_settings`
 * and written via `update_app_settings`. The permission state is read here but
 * written through `set_notification_permission_state` (ADR 0017 decision 5).
 * The `last_seen_version` cursor follows the same shape, written through
 * `set_last_seen_version` from the "What's new" dialog (ADR 0025). The
 * `auto_update_last_*` columns follow the same shape, written by the
 * backend updater worker via `record_update_check` (ADR-0024).
 */
export interface AppSettings {
  readonly notifications_enabled: boolean;
  readonly notify_on_needs_attention: boolean;
  readonly notify_on_mention: boolean;
  readonly notification_permission_state: NotificationPermissionState;
  /**
   * Last app version the user dismissed the in-app "What's new" dialog
   * against. `null` means the cursor has never been written (fresh install).
   * Written by the launch hook on first run, then by the dialog dismiss
   * handler on every subsequent version transition. ADR 0025.
   */
  readonly last_seen_version: string | null;
  /** Auto-update toggle (opt-in per ADR-0024). Defaults to `false`. */
  readonly auto_update_enabled: boolean;
  /** Auto-update poll cadence in seconds. Defaults to 21600 (6 hours). */
  readonly auto_update_interval_seconds: number;
  /**
   * Unix seconds of the last update check attempt (success or failure).
   * `null` means no check has ever run. Read-only from the Settings panel.
   */
  readonly auto_update_last_check_at: number | null;
  /**
   * Short error from the last failed check, or `null` when the last check
   * succeeded. The Settings panel surfaces "Last check failed: <message>"
   * iff this is set. Read-only from the Settings panel.
   */
  readonly auto_update_last_failure_message: string | null;
  /** Unix seconds. Advanced server-side on every write. */
  readonly updated_at: number;
}

/**
 * Default state used when the Tauri backend isn't reachable (e.g. running the
 * Vite dev server in a plain browser). Matches the migration's seeded row.
 */
const DEFAULT_SETTINGS: AppSettings = {
  notifications_enabled: true,
  notify_on_needs_attention: true,
  notify_on_mention: true,
  notification_permission_state: "unprompted",
  last_seen_version: null,
  auto_update_enabled: false,
  auto_update_interval_seconds: 21600,
  auto_update_last_check_at: null,
  auto_update_last_failure_message: null,
  updated_at: 0,
};

/**
 * Subset of [`AppSettings`] the writer command accepts. The permission
 * state, `last_seen_version`, and the two `auto_update_last_*` columns
 * are intentionally excluded (the backend ignores them on
 * `update_app_settings`).
 */
export interface AppSettingsUpdate {
  readonly notifications_enabled: boolean;
  readonly notify_on_needs_attention: boolean;
  readonly notify_on_mention: boolean;
  readonly auto_update_enabled: boolean;
  readonly auto_update_interval_seconds: number;
}

export const useAppSettings = defineStore("app-settings", () => {
  const settings = ref<AppSettings>({ ...DEFAULT_SETTINGS });
  const loading = ref(false);
  const lastError = ref<string | null>(null);

  const notificationsEnabled = computed(() => settings.value.notifications_enabled);
  const notifyOnNeedsAttention = computed(
    () => settings.value.notify_on_needs_attention,
  );
  const notifyOnMention = computed(() => settings.value.notify_on_mention);
  const permissionState = computed(() => settings.value.notification_permission_state);
  const lastSeenVersion = computed(() => settings.value.last_seen_version);
  const autoUpdateEnabled = computed(() => settings.value.auto_update_enabled);
  const autoUpdateLastCheckAt = computed(
    () => settings.value.auto_update_last_check_at,
  );
  const autoUpdateLastFailureMessage = computed(
    () => settings.value.auto_update_last_failure_message,
  );

  async function load(): Promise<void> {
    loading.value = true;
    lastError.value = null;
    try {
      settings.value = await invoke<AppSettings>("get_app_settings");
    } catch (err) {
      lastError.value = formatError(err);
    } finally {
      loading.value = false;
    }
  }

  /**
   * Optimistic write. Flips the local state first so the toggle feels instant,
   * then reconciles with the backend's authoritative reply. On error the
   * pre-write snapshot is restored and `lastError` is populated.
   */
  async function update(prefs: AppSettingsUpdate): Promise<void> {
    const previous = settings.value;
    settings.value = {
      ...previous,
      notifications_enabled: prefs.notifications_enabled,
      notify_on_needs_attention: prefs.notify_on_needs_attention,
      notify_on_mention: prefs.notify_on_mention,
      auto_update_enabled: prefs.auto_update_enabled,
      auto_update_interval_seconds: prefs.auto_update_interval_seconds,
    };
    lastError.value = null;
    try {
      // Echo the full `AppSettings` shape because the Rust command
      // deserialises into the same struct. The writer ignores the
      // permission state, the last-seen-version cursor, and the two
      // last-check columns server-side; only the dedicated commands
      // (`set_notification_permission_state`, `set_last_seen_version`,
      // `record_update_check`) write those.
      settings.value = await invoke<AppSettings>("update_app_settings", {
        prefs: {
          notifications_enabled: prefs.notifications_enabled,
          notify_on_needs_attention: prefs.notify_on_needs_attention,
          notify_on_mention: prefs.notify_on_mention,
          notification_permission_state: previous.notification_permission_state,
          last_seen_version: previous.last_seen_version,
          auto_update_enabled: prefs.auto_update_enabled,
          auto_update_interval_seconds: prefs.auto_update_interval_seconds,
          auto_update_last_check_at: previous.auto_update_last_check_at,
          auto_update_last_failure_message:
            previous.auto_update_last_failure_message,
          updated_at: 0,
        },
      });
    } catch (err) {
      settings.value = previous;
      lastError.value = formatError(err);
      throw new Error(lastError.value);
    }
  }

  /**
   * Persist the OS-reported permission state answered by an explicit panel
   * gesture. The frontend invokes the plugin's `requestPermission()` and
   * forwards the result here so the DB stays the single source of truth.
   */
  async function setPermissionState(
    state: NotificationPermissionState,
  ): Promise<void> {
    lastError.value = null;
    try {
      settings.value = await invoke<AppSettings>(
        "set_notification_permission_state",
        { state },
      );
    } catch (err) {
      lastError.value = formatError(err);
      throw new Error(lastError.value);
    }
  }

  /**
   * Advance the in-app "What's new" version cursor (ADR 0025). Called from
   * two places: the launch hook on first run (so a fresh install records the
   * current version silently and suppresses the dialog), and the dialog
   * dismiss handler (so the next launch on the same binary doesn't re-show
   * the changelog).
   */
  async function setLastSeenVersion(version: string): Promise<void> {
    lastError.value = null;
    try {
      settings.value = await invoke<AppSettings>("set_last_seen_version", {
        version,
      });
    } catch (err) {
      lastError.value = formatError(err);
      throw new Error(lastError.value);
    }
  }

  function clearError(): void {
    lastError.value = null;
  }

  return {
    settings,
    loading,
    lastError,
    notificationsEnabled,
    notifyOnNeedsAttention,
    notifyOnMention,
    permissionState,
    lastSeenVersion,
    autoUpdateEnabled,
    autoUpdateLastCheckAt,
    autoUpdateLastFailureMessage,
    load,
    update,
    setPermissionState,
    setLastSeenVersion,
    clearError,
  };
});

function formatError(raw: unknown): string {
  if (typeof raw === "string") return raw;
  if (raw instanceof Error) return raw.message;
  return "Couldn't reach the settings backend.";
}
