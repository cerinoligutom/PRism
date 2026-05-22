<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { RadioGroupIndicator, RadioGroupItem, RadioGroupRoot } from "reka-ui";

import { useSyncStore } from "@/stores/sync";

interface IntervalOption {
  readonly value: number;
  readonly label: string;
  readonly hint: string;
}

// The set spans the backend's clamp range (30s-600s, ADR 0004). The smallest
// value pushes the rate-budget guard the hardest; the largest is the
// background-friendly ceiling the worker will honour.
const INTERVAL_OPTIONS: readonly IntervalOption[] = [
  { value: 30, label: "30 seconds", hint: "High frequency. Watch your rate budget." },
  { value: 60, label: "1 minute", hint: "Default. Balances freshness and API cost." },
  { value: 300, label: "5 minutes", hint: "Quieter polling for steady-state dashboards." },
  { value: 600, label: "10 minutes", hint: "Background pace. Maximum allowed interval." },
];

const sync = useSyncStore();

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

onMounted(async () => {
  // StatusBar binds the sync store on mount, but a fresh launch may land on
  // this page before the bind completes. Refresh defensively so the radio
  // group reflects the persisted value rather than the 60s default.
  try {
    await sync.refreshSnapshot();
  } catch {
    // The status bar binding will retry; leaving the local default is fine.
  }
});
</script>

<template>
  <div class="sync-panel">
    <header class="sync-panel__header">
      <h1 class="sync-panel__title">Sync</h1>
      <span class="sync-panel__sub">POLL INTERVAL</span>
    </header>

    <p class="sync-panel__explainer">
      PRism polls GitHub on this interval to refresh PR state. Conditional
      requests (ETag / If-None-Match, per <strong>ADR 0004</strong>) skip the
      bulk of the payload when nothing has changed, so a tighter interval
      doesn't burn rate budget unless real updates are flowing.
    </p>

    <section class="sync-panel__section">
      <div class="sync-panel__section-head">
        <h3 class="sync-panel__section-title">Poll interval</h3>
        <span class="sync-panel__section-desc">
          Applies to all connected accounts. Changes take effect on the next sync cycle.
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
  font-size: var(--fs-14);
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
</style>
