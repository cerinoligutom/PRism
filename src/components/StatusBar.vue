<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useEventListener, useResizeObserver } from "@vueuse/core";
import { useSyncStore, type SyncPhase } from "@/stores/sync";
import { useAccountsStore } from "@/stores/accounts";
import { useSyncActivityStore } from "@/stores/syncActivity";
import { usePlatformModifier } from "@/composables/usePlatformModifier";
import SyncActivityPanel from "./StatusBar/SyncActivityPanel.vue";
import { formatDuration } from "@/lib/format";

const sync = useSyncStore();
const accounts = useAccountsStore();
const syncActivity = useSyncActivityStore();

function loginForAccount(accountId: number | null): string | null {
  if (accountId === null) return null;
  return accounts.accounts.find((a) => a.id === accountId)?.login ?? null;
}

const chipRef = ref<HTMLButtonElement | null>(null);
const anchorRectSnapshot = ref<DOMRect | null>(null);
const panelOpen = ref(false);

function refreshAnchor(): void {
  if (chipRef.value === null) return;
  anchorRectSnapshot.value = chipRef.value.getBoundingClientRect();
}

function openPanel(): void {
  refreshAnchor();
  panelOpen.value = true;
}

function closePanel(): void {
  panelOpen.value = false;
}

function togglePanel(): void {
  if (panelOpen.value) {
    closePanel();
  } else {
    openPanel();
  }
}

function onChipHover(): void {
  // Auto-open behaviour: when the chip is in an error state and the user
  // hasn't acknowledged the failure yet, hovering reveals the panel so the
  // failure surface isn't missed. Once the panel opens the failure is marked
  // acknowledged, so subsequent hovers don't keep triggering the popup.
  if (panelOpen.value) return;
  if (summary.value.phase !== "error") return;
  if (!syncActivity.hasUnseenFailure) return;
  openPanel();
}

useResizeObserver(chipRef, refreshAnchor);
useEventListener(window, "resize", refreshAnchor);

onMounted(async () => {
  await sync.bind();
  await syncActivity.bind(loginForAccount);
  refreshAnchor();
});

onBeforeUnmount(() => {
  sync.unbind();
  syncActivity.unbind();
});

interface SummaryLine {
  readonly phase: SyncPhase;
  readonly dotClass: string;
  readonly labelClass: string;
  readonly label: string;
}

const summary = computed<SummaryLine>(() => {
  const phase = sync.aggregate;
  switch (phase) {
    case "error":
      return { phase, dotClass: "dot dot-danger", labelClass: "text-danger", label: "Sync failed" };
    case "unauthorized":
      return { phase, dotClass: "dot dot-warning", labelClass: "text-warning", label: "Reauth required" };
    case "rate_limited":
      return {
        phase,
        dotClass: "dot dot-warning",
        labelClass: "text-warning",
        label: rateLimitedLabel.value,
      };
    case "syncing":
      return { phase, dotClass: "dot dot-info dot-pulse", labelClass: "text-info", label: "Syncing" };
    case "synced":
      return { phase, dotClass: "dot dot-success", labelClass: "text-success", label: "Live" };
    case "idle":
    default:
      return {
        phase,
        dotClass: "dot",
        labelClass: "",
        label: accounts.isEmpty ? "Idle · no accounts" : "Idle",
      };
  }
});

/**
 * Phase chip label when an account is rate-limited. Surfaces the GitHub
 * sub-bucket that bottomed out ("Search budget low" / "Graphql budget low" /
 * "Core budget low") so a multi-account viewer can act on the specific
 * resource instead of staring at a generic "rate limited" message.
 */
const rateLimitedLabel = computed<string>(() => {
  const resource = sync.lastRateLimitResource;
  if (resource === null) return "Rate limited";
  const pretty = resource === "graphql" ? "GraphQL" : resource.charAt(0).toUpperCase() + resource.slice(1);
  return `${pretty} budget low`;
});

/**
 * Live ticker label. When a cycle is in flight, the chip's text reads from
 * the activity store's `currentPhaseLabel` (throttled in the store). When
 * idle / completed / errored, fall back to the phase-derived summary label.
 */
const chipLabel = computed<string>(() => {
  if (syncActivity.activeCycle && syncActivity.currentPhaseLabel !== null) {
    return syncActivity.currentPhaseLabel;
  }
  return summary.value.label;
});

const accountsLabel = computed<string | null>(() => {
  if (accounts.isEmpty) return null;
  return accounts.count === 1 ? "1 account" : `${accounts.count} accounts`;
});

const lastSyncedLabel = computed<string | null>(() => {
  const secs = sync.secondsSinceLastSync;
  if (secs === null) return null;
  return `Synced ${formatDuration(secs)} ago`;
});

const nextSyncLabel = computed<string | null>(() => {
  const secs = sync.secondsUntilNextSync;
  if (secs === null) return null;
  return `next in ${formatDuration(secs)}`;
});

const budgetLabel = computed<string | null>(() => {
  const pct = sync.rateRemainingPct;
  if (pct === null) return null;
  const used = Math.max(0, 100 - pct);
  const limit = sync.rateLimit;
  return limit === null ? `API budget · ${used}%` : `API budget · ${used}% / ${limit}/hr`;
});

// Refresh-shortcut glyph matches the binding installed by `useKeyboardShortcuts`.
const refreshGlyph = usePlatformModifier();
</script>

<template>
  <footer class="status-bar">
    <button
      ref="chipRef"
      type="button"
      class="status-bar__item status-bar__chip"
      :aria-expanded="panelOpen"
      aria-label="Open sync activity"
      @click="togglePanel"
      @mouseenter="onChipHover"
    >
      <span :class="summary.dotClass" />
      <span :class="summary.labelClass">{{ chipLabel }}</span>
      <template v-if="accountsLabel !== null">
        <span aria-hidden="true">·</span>
        <span>{{ accountsLabel }}</span>
      </template>
    </button>
    <span v-if="lastSyncedLabel !== null" class="status-bar__item">
      <span>{{ lastSyncedLabel }}</span>
      <template v-if="nextSyncLabel !== null">
        <span aria-hidden="true">·</span>
        <span>{{ nextSyncLabel }}</span>
      </template>
    </span>
    <span v-if="budgetLabel !== null" class="status-bar__item">
      {{ budgetLabel }}
    </span>
    <span class="status-bar__spacer" />
    <!-- Cmd+K Search and Cmd+, Settings hints land with their M7 bindings. -->
    <!-- <span class="status-bar__item status-bar__item--hint"><kbd>⌘</kbd><kbd>K</kbd> Search</span> -->
    <span
      class="status-bar__item status-bar__item--hint"
      :class="{ 'status-bar__item--hint-disabled': summary.phase === 'syncing' }"
    ><kbd>{{ refreshGlyph }}</kbd><kbd>R</kbd> Refresh</span>
    <!-- <span class="status-bar__item status-bar__item--hint"><kbd>⌘</kbd><kbd>,</kbd> Settings</span> -->

    <Teleport to="body">
      <SyncActivityPanel :open="panelOpen" :anchor-rect-snapshot="anchorRectSnapshot" @close="closePanel" />
    </Teleport>
  </footer>
</template>

<style scoped>
.status-bar {
  height: 28px;
  border-top: 1px solid var(--border-1);
  background: var(--bg-2);
  display: flex;
  align-items: center;
  padding: 0 var(--s-4);
  gap: var(--s-4);
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  letter-spacing: 0.3px;
}

.status-bar__item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}

.status-bar__chip {
  background: transparent;
  border: none;
  color: inherit;
  font: inherit;
  letter-spacing: inherit;
  padding: 0 4px;
  margin: 0 -4px;
  cursor: pointer;
  border-radius: var(--r-2);
}

.status-bar__chip:hover {
  background: color-mix(in oklch, var(--text) 8%, transparent);
}

.status-bar__chip:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
}

.status-bar__spacer {
  flex: 1;
}

.status-bar kbd {
  font-size: var(--fs-9);
  padding: 0 4px;
}

/* Faded look while a sync cycle is in flight - the keyboard shortcut's own
 * handler no-ops to prevent stampedes (see `useKeyboardShortcuts`), so the
 * visual disabled state keeps the hint honest. */
.status-bar__item--hint-disabled {
  opacity: 0.4;
}

/* Keyboard hints drop off first when the window is narrower than Tailwind's
 * `sm` breakpoint so the live state, timing, and budget items still fit. */
@media (max-width: 640px) {
  .status-bar__item--hint {
    display: none;
  }
}
</style>
