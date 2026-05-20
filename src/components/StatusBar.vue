<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted } from "vue";
import { useSyncStore, type SyncPhase } from "@/stores/sync";
import { useAccountsStore } from "@/stores/accounts";

const sync = useSyncStore();
const accounts = useAccountsStore();

onMounted(async () => {
  await sync.bind();
});

onBeforeUnmount(() => {
  sync.unbind();
});

interface SummaryLine {
  readonly phase: SyncPhase;
  readonly dotClass: string;
  readonly label: string;
}

const summary = computed<SummaryLine>(() => {
  const phase = sync.aggregate;
  switch (phase) {
    case "error":
      return { phase, dotClass: "dot dot-danger", label: "Sync failed" };
    case "unauthorized":
      return { phase, dotClass: "dot dot-warning", label: "Reauth required" };
    case "rate_limited":
      return { phase, dotClass: "dot dot-warning", label: "Rate limited" };
    case "syncing":
      return { phase, dotClass: "dot dot-info dot-pulse", label: "Syncing" };
    case "synced":
      return { phase, dotClass: "dot dot-success", label: "Live" };
    case "idle":
    default:
      return {
        phase,
        dotClass: "dot",
        label: accounts.isEmpty ? "Idle · no accounts" : "Idle",
      };
  }
});

const accountsLabel = computed<string | null>(() => {
  if (accounts.isEmpty) return null;
  return accounts.count === 1 ? "1 account" : `${accounts.count} accounts`;
});

const lastSyncedLabel = computed<string | null>(() => {
  const secs = sync.secondsSinceLastSync;
  if (secs === null) return null;
  return `Synced ${formatRelative(secs)} ago`;
});

const nextSyncLabel = computed<string | null>(() => {
  const secs = sync.nextSyncInSeconds;
  if (secs === null) return null;
  return `next in ${formatRelative(secs)}`;
});

const budgetLabel = computed<string | null>(() => {
  const pct = sync.rateRemainingPct;
  if (pct === null) return null;
  const used = Math.max(0, 100 - pct);
  const limit = sync.rateLimit;
  return limit === null ? `API budget · ${used}%` : `API budget · ${used}% / ${limit}/hr`;
});

function formatRelative(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h`;
}
</script>

<template>
  <footer class="status-bar">
    <span class="status-bar__item">
      <span :class="summary.dotClass" />
      <span>{{ summary.label }}</span>
      <template v-if="accountsLabel !== null">
        <span aria-hidden="true">·</span>
        <span>{{ accountsLabel }}</span>
      </template>
    </span>
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
    <span class="status-bar__item"><kbd>⌘</kbd><kbd>R</kbd> Refresh</span>
    <span class="status-bar__item"><kbd>⌘</kbd><kbd>,</kbd> Settings</span>
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

.status-bar__spacer {
  flex: 1;
}

.status-bar kbd {
  font-size: var(--fs-9);
  padding: 0 4px;
}
</style>
