<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  useSyncActivityStore,
  type ActivityEvent,
  type ActivityLevel,
} from "@/stores/syncActivity";
import { useAccountsStore } from "@/stores/accounts";
import { useSyncStore, type AccountSyncState, type SyncPhase } from "@/stores/sync";
import { formatDuration } from "@/lib/format";

/**
 * Diagnostic activity panel (issue #122). Anchored above the sync chip in
 * the status bar; non-modal; dismisses on outside click, Esc, or another
 * click on the chip. Filters by account + level (info / warn / error).
 *
 * The auto-open-on-hover-after-failure behaviour is driven by the parent
 * (StatusBar.vue) — this component is purely about rendering the filtered
 * list and providing filter controls.
 */

interface Props {
  open: boolean;
  /** Snapshot of the chip's bounding rect at the moment the panel opened.
   * Not live - the parent refreshes it explicitly. */
  anchorRectSnapshot: DOMRect | null;
}

const props = defineProps<Props>();
const emit = defineEmits<{
  (e: "close"): void;
}>();

const activity = useSyncActivityStore();
const accounts = useAccountsStore();
const sync = useSyncStore();

const containerRef = ref<HTMLDivElement | null>(null);

const accountFilter = ref<number | "all">("all");
const levelFilter = ref<Record<ActivityLevel, boolean>>({
  info: true,
  warn: true,
  error: true,
});

const visibleAccounts = computed(() => accounts.accounts);

interface AccountStatusRow {
  readonly id: number;
  readonly label: string;
  readonly phase: SyncPhase;
  readonly phaseClass: string;
  readonly phaseLabel: string;
  readonly detail: string | null;
}

/**
 * Per-account current state, surfaced at the top of the panel so a user
 * with a failing account can identify which one without filtering. Sorted
 * by severity so failing accounts appear first.
 */
const accountStatusRows = computed<AccountStatusRow[]>(() => {
  if (visibleAccounts.value.length <= 1) return [];
  const stateById = new Map<number, AccountSyncState>();
  for (const state of sync.accounts) stateById.set(state.account_id, state);
  const rows = visibleAccounts.value.map((account): AccountStatusRow => {
    const state = stateById.get(account.id) ?? null;
    const phase: SyncPhase = state?.phase ?? "idle";
    return {
      id: account.id,
      label: account.label || account.login,
      phase,
      phaseClass: phaseDotClass(phase),
      phaseLabel: phaseDisplayLabel(phase),
      detail: detailFor(state),
    };
  });
  rows.sort((a, b) => phaseSeverity(b.phase) - phaseSeverity(a.phase));
  return rows;
});

function phaseSeverity(phase: SyncPhase): number {
  switch (phase) {
    case "error":
      return 5;
    case "unauthorized":
      return 4;
    case "rate_limited":
      return 3;
    case "syncing":
      return 2;
    case "synced":
      return 1;
    case "idle":
    default:
      return 0;
  }
}

function phaseDotClass(phase: SyncPhase): string {
  switch (phase) {
    case "error":
      return "account-status__dot account-status__dot--danger";
    case "unauthorized":
    case "rate_limited":
      return "account-status__dot account-status__dot--warning";
    case "syncing":
      return "account-status__dot account-status__dot--info account-status__dot--pulse";
    case "synced":
      return "account-status__dot account-status__dot--success";
    case "idle":
    default:
      return "account-status__dot";
  }
}

function phaseDisplayLabel(phase: SyncPhase): string {
  switch (phase) {
    case "error":
      return "Failed";
    case "unauthorized":
      return "Sign in";
    case "rate_limited":
      return "Throttled";
    case "syncing":
      return "Syncing";
    case "synced":
      return "Synced";
    case "idle":
    default:
      return "Idle";
  }
}

function detailFor(state: AccountSyncState | null): string | null {
  if (state === null) return null;
  if (state.message !== null && state.message !== "") return state.message;
  if (state.last_synced_at === null) return null;
  const synced = Date.parse(state.last_synced_at);
  if (Number.isNaN(synced)) return null;
  const secs = Math.max(0, Math.floor((Date.now() - synced) / 1000));
  return `${formatDuration(secs)} ago`;
}

const filtered = computed<ActivityEvent[]>(() => {
  return activity.events.filter((evt) => {
    if (!levelFilter.value[evt.level]) return false;
    if (accountFilter.value === "all") return true;
    return evt.account_id === accountFilter.value;
  });
});

const positionStyle = computed<Record<string, string>>(() => {
  const rect = props.anchorRectSnapshot;
  if (rect === null) {
    return { bottom: "32px", left: "8px" };
  }
  // Anchor the panel's bottom-left to the chip's top-left, with a 4px gap.
  const left = Math.max(8, Math.round(rect.left));
  const bottom = Math.max(32, Math.round(window.innerHeight - rect.top + 4));
  return {
    left: `${left}px`,
    bottom: `${bottom}px`,
  };
});

function formatRelative(timestampMs: number): string {
  const seconds = Math.max(0, Math.floor((Date.now() - timestampMs) / 1000));
  if (seconds < 1) return "just now";
  return `${formatDuration(seconds)} ago`;
}

function levelClass(level: ActivityLevel): string {
  switch (level) {
    case "warn":
      return "activity-row__icon--warn";
    case "error":
      return "activity-row__icon--error";
    case "info":
    default:
      return "activity-row__icon--info";
  }
}

function levelLabel(level: ActivityLevel): string {
  switch (level) {
    case "warn":
      return "Warning";
    case "error":
      return "Error";
    case "info":
    default:
      return "Info";
  }
}

function prUrlFor(event: ActivityEvent): string | null {
  if (event.kind === "pr_fetched") return event.url;
  if (event.kind === "pr_skipped_no_change") return event.url;
  return null;
}

async function openExternal(url: string): Promise<void> {
  try {
    await openUrl(url);
  } catch {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

function toggleLevel(level: ActivityLevel): void {
  levelFilter.value = {
    ...levelFilter.value,
    [level]: !levelFilter.value[level],
  };
}

function onAccountChange(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  accountFilter.value = value === "all" ? "all" : Number(value);
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    emit("close");
  }
}

function onOutsideClick(event: MouseEvent): void {
  if (!containerRef.value) return;
  const target = event.target as Node | null;
  if (target && !containerRef.value.contains(target)) {
    // Defer one tick so a click on the chip itself toggles cleanly instead of
    // racing the chip's own click handler.
    emit("close");
  }
}

// Bind / unbind dismissal listeners. Mouse / touch + Esc handle the
// non-modal dismiss contract.
watch(
  () => props.open,
  async (next, _prev) => {
    if (next) {
      await nextTick();
      // Mark the current failure as dismissed as soon as the user opens the
      // panel. The auto-open behaviour in StatusBar.vue keys off
      // `hasUnseenFailure`, so this clears the next-hover loop.
      activity.dismissFailure();
      document.addEventListener("mousedown", onOutsideClick, true);
      document.addEventListener("keydown", onKeydown);
    } else {
      document.removeEventListener("mousedown", onOutsideClick, true);
      document.removeEventListener("keydown", onKeydown);
    }
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  document.removeEventListener("mousedown", onOutsideClick, true);
  document.removeEventListener("keydown", onKeydown);
});

function rowClass(event: ActivityEvent): string[] {
  const classes = ["activity-row"];
  if (
    event.kind === "cycle_failed" &&
    activity.latestFailure !== null &&
    activity.latestFailure.id === event.id
  ) {
    classes.push("activity-row--highlight");
  }
  return classes;
}
</script>

<template>
  <div
    v-if="open"
    ref="containerRef"
    class="activity-panel"
    role="dialog"
    aria-modal="false"
    aria-label="Sync activity"
    :style="positionStyle"
  >
    <header class="activity-panel__header">
      <h2 class="activity-panel__title">Sync activity</h2>
      <button
        class="activity-panel__close"
        type="button"
        aria-label="Close"
        @click="emit('close')"
      >
        ×
      </button>
    </header>
    <ul
      v-if="accountStatusRows.length > 0"
      class="account-status"
      role="list"
      aria-label="Per-account sync state"
    >
      <li
        v-for="row in accountStatusRows"
        :key="row.id"
        class="account-status__row"
      >
        <span :class="row.phaseClass" :aria-label="row.phaseLabel" />
        <span class="account-status__label">{{ row.label }}</span>
        <span class="account-status__phase">{{ row.phaseLabel }}</span>
        <span v-if="row.detail !== null" class="account-status__detail">{{ row.detail }}</span>
      </li>
    </ul>
    <div class="activity-panel__filters">
      <label v-if="visibleAccounts.length > 1" class="activity-panel__field">
        <span class="activity-panel__field-label">Account</span>
        <select
          class="activity-panel__select"
          :value="accountFilter === 'all' ? 'all' : String(accountFilter)"
          @change="onAccountChange"
        >
          <option value="all">All accounts</option>
          <option
            v-for="account in visibleAccounts"
            :key="account.id"
            :value="String(account.id)"
          >
            {{ account.login }}
          </option>
        </select>
      </label>
      <div class="activity-panel__levels" role="group" aria-label="Level filter">
        <button
          v-for="level in (['info', 'warn', 'error'] as const)"
          :key="level"
          type="button"
          class="activity-panel__level-chip"
          :class="{ 'activity-panel__level-chip--active': levelFilter[level] }"
          :aria-pressed="levelFilter[level]"
          @click="toggleLevel(level)"
        >
          {{ levelLabel(level) }}
        </button>
      </div>
    </div>
    <ul v-if="filtered.length > 0" class="activity-panel__list" role="list">
      <li
        v-for="event in filtered"
        :key="event.id"
        :class="rowClass(event)"
      >
        <span
          class="activity-row__icon"
          :class="levelClass(event.level)"
          :aria-label="levelLabel(event.level)"
        />
        <div class="activity-row__body">
          <p class="activity-row__message">{{ event.message }}</p>
          <p class="activity-row__meta">
            <span>{{ formatRelative(event.timestamp_ms) }}</span>
          </p>
        </div>
        <button
          v-if="prUrlFor(event) !== null"
          type="button"
          class="activity-row__deeplink"
          aria-label="Open on GitHub"
          @click="openExternal(prUrlFor(event) as string)"
        >
          ↗
        </button>
      </li>
    </ul>
    <p v-else class="activity-panel__empty">No matching events yet.</p>
  </div>
</template>

<style scoped>
.activity-panel {
  position: fixed;
  z-index: 50;
  width: min(420px, calc(100vw - 32px));
  max-height: 60vh;
  background: var(--bg-1);
  border: 1px solid var(--border-1);
  border-radius: var(--r-3);
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.32);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  font-size: var(--fs-12);
}

.activity-panel__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-1);
  background: var(--bg-2);
}

.activity-panel__title {
  font-size: var(--fs-12);
  font-weight: 600;
  letter-spacing: 0.3px;
  text-transform: uppercase;
  color: var(--text-strong);
  margin: 0;
}

.activity-panel__close {
  background: transparent;
  border: none;
  color: var(--text-faint);
  font-size: 18px;
  line-height: 1;
  padding: 0 6px;
  cursor: pointer;
}

.activity-panel__close:hover {
  color: var(--text-strong);
}

.activity-panel__filters {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 14px;
  border-bottom: 1px solid var(--border-1);
  flex-wrap: wrap;
}

.activity-panel__field {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: var(--fs-10);
  color: var(--text-faint);
}

.activity-panel__field-label {
  text-transform: uppercase;
  letter-spacing: 0.3px;
}

.activity-panel__select {
  background: var(--bg-2);
  color: var(--text);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  padding: 4px 6px;
  font-size: var(--fs-12);
}

.activity-panel__levels {
  display: inline-flex;
  gap: 4px;
  margin-left: auto;
}

.activity-panel__level-chip {
  font-size: var(--fs-10);
  text-transform: uppercase;
  letter-spacing: 0.3px;
  padding: 3px 8px;
  border-radius: 999px;
  border: 1px solid var(--border-1);
  background: transparent;
  color: var(--text-faint);
  cursor: pointer;
}

.activity-panel__level-chip--active {
  background: var(--bg-3, var(--bg-2));
  color: var(--text);
  border-color: var(--border-2, var(--border-1));
}

.activity-panel__list {
  list-style: none;
  margin: 0;
  padding: 4px 0;
  overflow-y: auto;
  flex: 1;
  min-height: 0;
}

.activity-panel__empty {
  padding: 18px 14px;
  text-align: center;
  color: var(--text-faint);
  margin: 0;
}

.activity-row {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 8px 14px;
  border-bottom: 1px solid var(--border-1);
}

.activity-row:last-child {
  border-bottom: none;
}

.activity-row--highlight {
  background: color-mix(in oklch, var(--danger) 12%, transparent);
}

.activity-row__icon {
  flex: 0 0 8px;
  width: 8px;
  height: 8px;
  border-radius: 999px;
  margin-top: 5px;
}

.activity-row__icon--info {
  background: var(--info);
}

.activity-row__icon--warn {
  background: var(--warning);
}

.activity-row__icon--error {
  background: var(--danger);
}

.activity-row__body {
  flex: 1;
  min-width: 0;
}

.activity-row__message {
  margin: 0;
  color: var(--text);
  word-wrap: break-word;
}

.activity-row__meta {
  margin: 2px 0 0 0;
  color: var(--text-faint);
  font-size: var(--fs-10);
}

.activity-row__deeplink {
  background: transparent;
  border: none;
  color: var(--text-faint);
  cursor: pointer;
  padding: 2px 6px;
  border-radius: var(--r-2);
}

.activity-row__deeplink:hover {
  color: var(--text-strong);
  background: var(--bg-2);
}

.account-status {
  list-style: none;
  margin: 0;
  padding: 6px 14px;
  border-bottom: 1px solid var(--border-1);
  background: var(--bg-2);
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.account-status__row {
  display: grid;
  grid-template-columns: 10px minmax(0, 1fr) auto auto;
  align-items: center;
  gap: 8px;
  font-size: var(--fs-11);
  color: var(--text);
}

.account-status__label {
  font-weight: 500;
  color: var(--text-strong);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.account-status__phase {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  letter-spacing: 0.3px;
  text-transform: uppercase;
  color: var(--text-faint);
}

.account-status__detail {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  max-width: 180px;
}

.account-status__dot {
  width: 8px;
  height: 8px;
  border-radius: 999px;
  background: var(--text-disabled);
  flex: 0 0 8px;
}

.account-status__dot--success {
  background: var(--success);
}

.account-status__dot--info {
  background: var(--info);
}

.account-status__dot--warning {
  background: var(--warning);
}

.account-status__dot--danger {
  background: var(--danger);
}

.account-status__dot--pulse {
  animation: account-status-pulse 1.4s ease-in-out infinite;
}

@keyframes account-status-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.45; }
}
</style>
