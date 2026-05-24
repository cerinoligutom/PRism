<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
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

const mentionChecked = computed<boolean>({
  get: () => settingsStore.notifyOnMention,
  set: (value) => {
    void persistTriggerChange({ notify_on_mention: value });
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
  patch: Partial<{
    notify_on_needs_attention: boolean;
    notify_on_mention: boolean;
  }>,
): Promise<void> {
  await persistAll(patch);
}

async function persistAll(
  patch: Partial<{
    notifications_enabled: boolean;
    notify_on_needs_attention: boolean;
    notify_on_mention: boolean;
  }>,
): Promise<void> {
  const current = settingsStore.settings;
  try {
    await settingsStore.update({
      notifications_enabled:
        patch.notifications_enabled ?? current.notifications_enabled,
      notify_on_needs_attention:
        patch.notify_on_needs_attention ?? current.notify_on_needs_attention,
      notify_on_mention:
        patch.notify_on_mention ?? current.notify_on_mention,
      // The notifications panel doesn't touch the auto-update or
      // auto-archive fields; pass through the current values so the
      // writer round-trips them unchanged.
      auto_update_enabled: current.auto_update_enabled,
      auto_update_interval_seconds: current.auto_update_interval_seconds,
      auto_archive_days: current.auto_archive_days,
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

onMounted(() => {
  void settingsStore.load();
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
          Get a notification when a pull request needs you. Pick which moments
          warrant the interruption - the in-app sidebar dots stay on either way.
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
            <div class="set-row__name">A PR starts needing you</div>
            <div class="set-row__desc">
              The moment a pull request first lands in your needs-attention list.
            </div>
          </div>
          <PRismSwitch
            v-model="needsAttentionChecked"
            :disabled="!masterChecked"
            aria-label="Toast when a PR newly needs your attention"
          />
        </div>

        <div class="set-row" :class="{ 'set-row--muted': !masterChecked }">
          <div>
            <div class="set-row__name">Someone @-mentions you</div>
            <div class="set-row__desc">
              Each new mention in a pull request you're following.
            </div>
          </div>
          <PRismSwitch
            v-model="mentionChecked"
            :disabled="!masterChecked"
            aria-label="Toast when you're mentioned"
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
</style>
