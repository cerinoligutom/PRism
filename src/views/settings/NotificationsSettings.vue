<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";

import PRismCallout from "@/components/ui/PRismCallout.vue";
import PRismSwitch from "@/components/ui/PRismSwitch.vue";
import {
  useAppSettings,
  type NotificationPermissionState,
} from "@/stores/settings";

// Inbox retention bounds (ADR 0028, issue #380). The Rust writer clamps
// to the same range and the migration's CHECK constraint mirrors it; the
// UI clamps here so an out-of-range entry settles to the edge rather than
// bouncing a CHECK failure back through the toast.
const RETENTION_MIN = 50;
const RETENTION_MAX = 5000;

const settingsStore = useAppSettings();

// Tracks an in-flight permission prompt so the master toggle can't be
// double-flipped while waiting for the OS answer.
const askingPermission = ref(false);

const masterChecked = computed<boolean>({
  get: () => settingsStore.notificationsEnabled,
  set: (value) => {
    void handleMasterChange(value);
  },
});

const needsAttentionChecked = computed<boolean>({
  get: () => settingsStore.notifyOnNeedsAttention,
  set: (value) => {
    void persistTriggerChange({ notify_on_needs_attention: value });
  },
});

const permissionDenied = computed(
  () => settingsStore.permissionState === "denied",
);

const isMacPlatform = computed(() =>
  typeof navigator !== "undefined"
    ? /Mac|iPhone|iPad/.test(navigator.platform)
    : false,
);

async function handleMasterChange(next: boolean): Promise<void> {
  if (askingPermission.value) return;

  if (!next) {
    // Turning OFF is unconditional - never prompts the OS, never mutates
    // permission state. Persist the new master flag and the current trigger
    // selection.
    await persistAll({ notifications_enabled: false });
    return;
  }

  // Master flipping ON. ADR 0017 decision 5: ask the OS the first time the
  // user opts in. Subsequent flips after a granted state skip straight to the
  // persist step.
  if (settingsStore.permissionState === "denied") {
    // OS has the door closed. The callout already names the resolution path;
    // the toggle stays OFF without persisting (the visual switch returns to
    // OFF on the next render because the computed getter re-reads from the
    // store).
    return;
  }

  if (settingsStore.permissionState === "unprompted") {
    askingPermission.value = true;
    try {
      const answer = await askForPermission();
      const resolved = mapPermission(answer);
      await settingsStore.setPermissionState(resolved);
      if (resolved !== "granted") {
        // Denied or some other negative answer. Master stays OFF.
        return;
      }
    } catch {
      // The plugin call failed (e.g. the panel is rendered without the Tauri
      // runtime). Treat it as denied so the callout surfaces and the master
      // doesn't flip on under a false sense of capability.
      askingPermission.value = false;
      try {
        await settingsStore.setPermissionState("denied");
      } catch {
        // Already populated lastError in the store.
      }
      return;
    } finally {
      askingPermission.value = false;
    }
  }

  // Permission is now granted (either steady-state or freshly answered).
  await persistAll({ notifications_enabled: true });
}

async function persistTriggerChange(
  patch: Partial<{ notify_on_needs_attention: boolean }>,
): Promise<void> {
  await persistAll(patch);
}

async function persistAll(
  patch: Partial<{
    notifications_enabled: boolean;
    notify_on_needs_attention: boolean;
    notification_retention_max: number;
  }>,
): Promise<void> {
  const current = settingsStore.settings;
  try {
    await settingsStore.update({
      notifications_enabled:
        patch.notifications_enabled ?? current.notifications_enabled,
      notify_on_needs_attention:
        patch.notify_on_needs_attention ?? current.notify_on_needs_attention,
      // The auto-update + auto-archive fields belong to other settings
      // panels; pass them through unchanged so this trigger-toggle write
      // doesn't stomp on the user's other choices. Retention is owned by
      // this panel - the writer below picks up whatever's in the patch
      // or falls back to the current value.
      auto_update_enabled: current.auto_update_enabled,
      auto_update_interval_seconds: current.auto_update_interval_seconds,
      auto_archive_days: current.auto_archive_days,
      notification_retention_max:
        patch.notification_retention_max ?? current.notification_retention_max,
    });
  } catch {
    // Store already reverted optimistic state and populated lastError.
  }
}

async function askForPermission(): Promise<NotificationPermission> {
  // `NotificationPermission` here is the lib.dom global type
  // (`"default" | "granted" | "denied"`) that the plugin's `requestPermission`
  // re-uses; no plugin-specific import needed.
  // Some platforms (macOS, Windows) return the steady-state value via
  // `isPermissionGranted` without surfacing the OS prompt. Re-check first to
  // avoid asking when permission has already been granted at the OS layer
  // without our DB knowing about it (e.g. a fresh install on a system that
  // previously had PRism connected).
  if (await isPermissionGranted()) return "granted";
  return await requestPermission();
}

function mapPermission(
  answer: NotificationPermission,
): NotificationPermissionState {
  if (answer === "granted") return "granted";
  if (answer === "denied") return "denied";
  return "unprompted";
}

// Inbox retention input. Mirrors the auto-archive numeric-input pattern in
// SyncSettings.vue: a local buffer string so the user can type "" / "12"
// without the watcher snapping back, committed on blur or Enter. The
// store re-syncs the buffer only when the persist isn't in flight.
const retentionBuffer = ref<string>(String(settingsStore.notificationRetentionMax));
const retentionPersisting = ref(false);
const retentionError = ref<string | null>(null);

watch(
  () => settingsStore.notificationRetentionMax,
  (next) => {
    if (!retentionPersisting.value) {
      retentionBuffer.value = String(next);
    }
  },
);

function clampRetention(raw: string): number {
  const parsed = Number.parseInt(raw, 10);
  if (Number.isNaN(parsed)) return settingsStore.notificationRetentionMax;
  if (parsed < RETENTION_MIN) return RETENTION_MIN;
  if (parsed > RETENTION_MAX) return RETENTION_MAX;
  return parsed;
}

async function commitRetention(): Promise<void> {
  if (retentionPersisting.value) return;
  const next = clampRetention(retentionBuffer.value);
  retentionBuffer.value = String(next);
  if (next === settingsStore.notificationRetentionMax) return;
  retentionPersisting.value = true;
  retentionError.value = null;
  try {
    await persistAll({ notification_retention_max: next });
  } catch (caught) {
    retentionError.value =
      caught instanceof Error ? caught.message : String(caught);
  } finally {
    retentionPersisting.value = false;
  }
}

function onRetentionKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") {
    event.preventDefault();
    void commitRetention();
  }
}

onMounted(async () => {
  try {
    await settingsStore.load();
    retentionBuffer.value = String(settingsStore.notificationRetentionMax);
  } catch {
    // The store populates `lastError`; the placeholder value stays.
  }
});
</script>

<template>
  <div class="notifications-panel">
    <header class="notifications-panel__header">
      <h1 class="notifications-panel__title">Notifications</h1>
    </header>

    <PRismCallout
      v-if="permissionDenied"
      variant="danger"
      class="notifications-panel__callout"
    >
      <template #icon>
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M8 3v6M8 11v.5" />
          <circle cx="8" cy="8" r="6.5" />
        </svg>
      </template>
      Notifications are blocked by your operating system. Enable them in
      <strong>System Settings &rarr; Notifications &rarr; PRism</strong>.
    </PRismCallout>

    <section class="notifications-panel__section">
      <div class="notifications-panel__section-head">
        <h3 class="notifications-panel__section-title">Desktop notifications</h3>
        <span class="notifications-panel__section-desc">
          Get a notification when a pull request needs you - the in-app sidebar
          dots stay on either way.
        </span>
      </div>

      <div class="notifications-panel__row-list">
        <div class="set-row">
          <div>
            <div class="set-row__name">Enable desktop notifications</div>
            <div class="set-row__desc">
              When on, PRism can pop a system notification. Your OS will ask
              for permission the first time.
            </div>
          </div>
          <PRismSwitch
            v-model="masterChecked"
            :disabled="askingPermission || permissionDenied"
            aria-label="Enable desktop notifications"
          />
        </div>

        <div class="set-row" :class="{ 'set-row--muted': !masterChecked }">
          <div>
            <div class="set-row__name">Notify when a PR needs me</div>
            <div class="set-row__desc">
              When a conversation you're in moves, or you're asked to review.
            </div>
          </div>
          <PRismSwitch
            v-model="needsAttentionChecked"
            :disabled="!masterChecked"
            aria-label="Notify when a PR needs me"
          />
        </div>
      </div>

      <p v-if="!isMacPlatform" class="notifications-panel__platform-note">
        The unread count badge on the app icon is macOS-only for now.
      </p>

      <p v-if="settingsStore.lastError" class="notifications-panel__error">
        {{ settingsStore.lastError }}
      </p>
    </section>

    <section class="notifications-panel__section">
      <div class="notifications-panel__section-head">
        <h3 class="notifications-panel__section-title">Inbox retention</h3>
        <span class="notifications-panel__section-desc">
          How many notifications to keep before the oldest are dropped.
        </span>
      </div>

      <div class="notifications-panel__retention">
        <label class="notifications-panel__retention-field">
          <span class="notifications-panel__retention-label">Keep most recent</span>
          <span class="notifications-panel__retention-input">
            <input
              v-model="retentionBuffer"
              class="input"
              type="number"
              inputmode="numeric"
              :min="RETENTION_MIN"
              :max="RETENTION_MAX"
              step="50"
              :disabled="retentionPersisting"
              aria-label="Keep most recent notifications"
              @blur="commitRetention"
              @keydown="onRetentionKeydown"
            />
            <span class="notifications-panel__retention-unit">notifications</span>
          </span>
        </label>
        <p class="notifications-panel__retention-hint">
          Older notifications are dropped automatically when this limit is reached.
        </p>
        <p class="notifications-panel__retention-help">
          Minimum {{ RETENTION_MIN }}, maximum {{ RETENTION_MAX }}.
        </p>
        <p v-if="retentionError" class="notifications-panel__error">{{ retentionError }}</p>
      </div>
    </section>
  </div>
</template>

<style scoped>
.notifications-panel__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  margin-bottom: var(--s-6);
}

.notifications-panel__title {
  margin: 0;
  font-size: var(--fs-24);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}

.notifications-panel__sub {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  letter-spacing: 0.5px;
}

.notifications-panel__callout {
  margin-bottom: var(--s-6);
}

.notifications-panel__section {
  margin-bottom: var(--s-7);
}

.notifications-panel__section-head {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: var(--s-4);
}

.notifications-panel__section-title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
}

.notifications-panel__section-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.notifications-panel__row-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
  background: var(--border-1);
  border-radius: var(--r-3);
  overflow: hidden;
}

.notifications-panel__platform-note {
  margin-top: var(--s-4);
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.notifications-panel__error {
  margin-top: var(--s-4);
  padding: 10px 14px;
  border-radius: var(--r-2);
  background: var(--danger-bg);
  color: var(--danger);
  font-size: var(--fs-12);
}

/* `.set-row` base styles live in primitives.css. `--muted` is the panel-
 * specific modifier that dims the secondary toggles while the master is OFF;
 * the underlying switch is also `disabled` so the dim is reinforced by the
 * switch primitive's own disabled state. */
.set-row--muted {
  opacity: 0.55;
}

.notifications-panel__retention {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.notifications-panel__retention-field {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.notifications-panel__retention-label {
  font-size: var(--fs-12);
  color: var(--text-mute);
  font-weight: 500;
}

.notifications-panel__retention-input {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
}

.notifications-panel__retention-input .input {
  width: 96px;
  text-align: right;
}

.notifications-panel__retention-unit {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.notifications-panel__retention-hint {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text);
  line-height: 1.45;
}

.notifications-panel__retention-help {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: 1.45;
}
</style>
