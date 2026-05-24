<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { RadioGroupIndicator, RadioGroupItem, RadioGroupRoot } from "reka-ui";

import { useSyncStore } from "@/stores/sync";
import { useAppSettings } from "@/stores/settings";

interface IntervalOption {
  readonly value: number;
  readonly label: string;
  readonly hint: string;
}

const INTERVAL_OPTIONS: readonly IntervalOption[] = [
  { value: 30, label: "Every 30 seconds", hint: "Fastest updates. Best when you're actively reviewing." },
  { value: 60, label: "Every minute", hint: "Recommended. Quick to notice changes without being chatty." },
  { value: 300, label: "Every 5 minutes", hint: "Easy on your network and battery." },
  { value: 600, label: "Every 10 minutes", hint: "Light background polling." },
  { value: 1800, label: "Every 30 minutes", hint: "Low-touch. Catches changes a few times an hour." },
  { value: 3600, label: "Every hour", hint: "Quietest auto setting. Great for long sessions." },
  { value: 0, label: "Manual", hint: "No automatic checks. Use Cmd+R or the Refresh button to sync." },
];

const AUTO_ARCHIVE_MIN = 0;
const AUTO_ARCHIVE_MAX = 365;

const sync = useSyncStore();
const settings = useAppSettings();

const persisting = ref(false);
const error = ref<string | null>(null);

// Holds the user's in-flight click so the radio doesn't flash back to the
// store value while the invoke is mid-round-trip. Cleared once the store
// updates (success) or the persist call rejects (revert).
const optimistic = ref<number | null>(null);

const selectedValue = computed<number>({
  get: () => optimistic.value ?? sync.intervalSeconds,
  set: (next) => {
    void persistInterval(next);
  },
});

// Local edit buffer for the auto-archive input. Mirrors the store but lets
// the user type "0", "-2", or "" without the watcher re-snapping back. The
// buffer is committed on blur or Enter.
const autoArchiveBuffer = ref<string>(String(settings.autoArchiveDays));
const archivePersisting = ref(false);
const archiveError = ref<string | null>(null);

watch(
  () => settings.autoArchiveDays,
  (next) => {
    if (!archivePersisting.value) {
      autoArchiveBuffer.value = String(next);
    }
  },
);

const autoArchiveHint = computed(() => {
  if (settings.autoArchiveDays === 0) {
    return "Auto-archive is off. Only manual archive flips a PR.";
  }
  return `Closed or merged PRs auto-archive after ${settings.autoArchiveDays} days of inactivity.`;
});

async function persistInterval(next: number): Promise<void> {
  if (persisting.value) return;
  if (next === sync.intervalSeconds) return;
  optimistic.value = next;
  persisting.value = true;
  error.value = null;
  try {
    await sync.setIntervalSeconds(next);
  } catch (caught) {
    error.value = caught instanceof Error ? caught.message : String(caught);
  } finally {
    optimistic.value = null;
    persisting.value = false;
  }
}

function clampArchiveDays(raw: string): number {
  const parsed = Number.parseInt(raw, 10);
  if (Number.isNaN(parsed)) return settings.autoArchiveDays;
  if (parsed < AUTO_ARCHIVE_MIN) return AUTO_ARCHIVE_MIN;
  if (parsed > AUTO_ARCHIVE_MAX) return AUTO_ARCHIVE_MAX;
  return parsed;
}

async function commitArchiveDays(): Promise<void> {
  if (archivePersisting.value) return;
  const next = clampArchiveDays(autoArchiveBuffer.value);
  autoArchiveBuffer.value = String(next);
  if (next === settings.autoArchiveDays) return;
  archivePersisting.value = true;
  archiveError.value = null;
  try {
    await settings.update({
      notifications_enabled: settings.settings.notifications_enabled,
      notify_on_needs_attention: settings.settings.notify_on_needs_attention,
      notify_on_mention: settings.settings.notify_on_mention,
      auto_update_enabled: settings.settings.auto_update_enabled,
      auto_update_interval_seconds:
        settings.settings.auto_update_interval_seconds,
      auto_archive_days: next,
    });
  } catch (caught) {
    archiveError.value = caught instanceof Error ? caught.message : String(caught);
  } finally {
    archivePersisting.value = false;
  }
}

function onArchiveKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") {
    event.preventDefault();
    void commitArchiveDays();
  }
}

onMounted(async () => {
  // StatusBar binds the sync store on mount, but a fresh launch may land on
  // this page before the bind completes. Refresh defensively so the radio
  // group reflects the persisted value rather than the 60s default.
  try {
    await sync.refreshSnapshot();
  } catch {
    // The status bar binding will retry; leaving the local default is fine.
  }
  // Load app_settings so the auto-archive input reflects the persisted
  // value rather than the store's DEFAULT_SETTINGS placeholder.
  try {
    await settings.load();
    autoArchiveBuffer.value = String(settings.autoArchiveDays);
  } catch {
    // The store populates `lastError`; falling back to the placeholder is
    // fine while the user is offline.
  }
});
</script>

<template>
  <div class="sync-panel">
    <header class="sync-panel__header">
      <h1 class="sync-panel__title">Sync</h1>
    </header>

    <p class="sync-panel__explainer">
      Choose how often PRism checks GitHub for new activity. Faster intervals
      surface changes sooner; slower intervals are kinder to your network.
      PRism is smart about it - if nothing has changed upstream, the check
      is nearly free.
    </p>

    <section class="sync-panel__section">
      <div class="sync-panel__section-head">
        <h3 class="sync-panel__section-title">How often to check</h3>
        <span class="sync-panel__section-desc">
          Applies to every connected account. New setting takes effect on the next check.
        </span>
      </div>

      <RadioGroupRoot
        v-model="selectedValue"
        :disabled="persisting"
        class="sync-panel__options"
        aria-label="Poll interval"
      >
        <RadioGroupItem
          v-for="option in INTERVAL_OPTIONS"
          :key="option.value"
          :value="option.value"
          class="sync-option"
          :class="{ 'sync-option--active': selectedValue === option.value }"
          :aria-label="option.label"
        >
          <span class="sync-option__radio">
            <RadioGroupIndicator class="sync-option__indicator" />
          </span>
          <span class="sync-option__body">
            <span class="sync-option__label">{{ option.label }}</span>
            <span class="sync-option__hint">{{ option.hint }}</span>
          </span>
        </RadioGroupItem>
      </RadioGroupRoot>

      <p v-if="error" class="sync-panel__error">{{ error }}</p>
    </section>

    <section class="sync-panel__section">
      <div class="sync-panel__section-head">
        <h3 class="sync-panel__section-title">Auto-archive after</h3>
        <span class="sync-panel__section-desc">
          How long a closed or merged PR lingers before flipping to the Archive view.
        </span>
      </div>

      <div class="sync-panel__archive">
        <label class="sync-panel__archive-field">
          <span class="sync-panel__archive-label">Days of inactivity</span>
          <span class="sync-panel__archive-input">
            <input
              v-model="autoArchiveBuffer"
              class="input"
              type="number"
              inputmode="numeric"
              :min="AUTO_ARCHIVE_MIN"
              :max="AUTO_ARCHIVE_MAX"
              step="1"
              :disabled="archivePersisting"
              aria-label="Auto-archive after days of inactivity"
              @blur="commitArchiveDays"
              @keydown="onArchiveKeydown"
            />
            <span class="sync-panel__archive-unit">days</span>
          </span>
        </label>
        <p class="sync-panel__archive-hint">{{ autoArchiveHint }}</p>
        <p class="sync-panel__archive-help">
          Set to 0 to disable auto-archive entirely. Maximum 365 days.
        </p>
        <p v-if="archiveError" class="sync-panel__error">{{ archiveError }}</p>
      </div>
    </section>
  </div>
</template>

<style scoped>
.sync-panel__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  margin-bottom: var(--s-6);
}

.sync-panel__title {
  margin: 0;
  font-size: var(--fs-24);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}

.sync-panel__sub {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  letter-spacing: 0.5px;
}

.sync-panel__explainer {
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: 1.55;
  margin: 0 0 var(--s-6);
  max-width: 600px;
}

.sync-panel__section {
  margin-bottom: var(--s-7);
}

.sync-panel__section-head {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: var(--s-4);
}

.sync-panel__section-title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
}

.sync-panel__section-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.sync-panel__options {
  display: flex;
  flex-direction: column;
  gap: 1px;
  background: var(--border-1);
  border-radius: var(--r-3);
  overflow: hidden;
}

.sync-option {
  background: var(--bg-2);
  padding: 14px 18px;
  display: grid;
  grid-template-columns: 18px 1fr;
  gap: var(--s-3);
  align-items: center;
  cursor: pointer;
  border: 0;
  text-align: left;
  width: 100%;
  font: inherit;
  color: inherit;
}

.sync-option--active {
  background: var(--accent-bg);
}

.sync-option:focus-visible {
  outline: none;
  box-shadow: inset 0 0 0 2px var(--focus-ring);
}

.sync-option:disabled {
  cursor: not-allowed;
  opacity: 0.6;
}

.sync-option__radio {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  border: 1px solid var(--border-3);
  background: var(--bg-1);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.sync-option[data-state="checked"] .sync-option__radio {
  border-color: var(--accent);
}

.sync-option__indicator {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--accent);
  display: block;
}

.sync-option__body {
  display: flex;
  flex-direction: column;
}

.sync-option__label {
  font-size: var(--fs-13);
  color: var(--text);
  font-weight: 500;
}

.sync-option__hint {
  font-size: var(--fs-12);
  color: var(--text-mute);
  margin-top: 2px;
  line-height: 1.45;
}

.sync-panel__error {
  margin-top: var(--s-4);
  padding: 10px 14px;
  border-radius: var(--r-2);
  background: var(--danger-bg);
  color: var(--danger);
  font-size: var(--fs-12);
}

.sync-panel__archive {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.sync-panel__archive-field {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.sync-panel__archive-label {
  font-size: var(--fs-12);
  color: var(--text-mute);
  font-weight: 500;
}

.sync-panel__archive-input {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
}

.sync-panel__archive-input .input {
  width: 96px;
  text-align: right;
}

.sync-panel__archive-unit {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.sync-panel__archive-hint {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text);
  line-height: 1.45;
}

.sync-panel__archive-help {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: 1.45;
}
</style>
