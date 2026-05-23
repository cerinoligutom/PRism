import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { useTimestamp } from "@vueuse/core";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { useTauriListener } from "@/composables/useTauriListener";

export type SyncPhase =
  | "idle"
  | "syncing"
  | "synced"
  | "error"
  | "unauthorized"
  | "rate_limited";

export interface AccountSyncState {
  readonly account_id: number;
  readonly phase: SyncPhase;
  readonly last_synced_at: string | null;
  readonly next_sync_in_seconds: number | null;
  readonly message: string | null;
  readonly rate_remaining_pct: number | null;
  readonly rate_limit: number | null;
}

export interface SyncStatusSnapshot {
  readonly accounts: readonly AccountSyncState[];
  readonly interval_seconds: number;
  readonly min_interval_seconds: number;
  readonly max_interval_seconds: number;
}

interface SyncStatusEvent {
  readonly account_id: number;
  readonly phase: SyncPhase;
  readonly last_synced_at: string | null;
  readonly next_sync_in_seconds: number | null;
  readonly message: string | null;
  readonly rate_remaining_pct: number | null;
  readonly rate_limit: number | null;
}

/**
 * GitHub sub-bucket name. The worker tags rate-limit events with which
 * resource bottomed out so the status bar can read "search budget low"
 * instead of the generic "rate limited". Absent for legacy events emitted
 * before the bucket hint was added.
 */
export type RateLimitResource = "core" | "search" | "graphql";

interface SyncRateLimitEvent {
  readonly account_id: number;
  readonly rate_remaining_pct: number;
  readonly limit: number | null;
  readonly reset_in_seconds: number | null;
  readonly resource?: RateLimitResource;
}

const SYNC_STATUS_EVENT = "sync://status";
const SYNC_ERROR_EVENT = "sync://error";
const SYNC_RATE_LIMIT_EVENT = "sync://rate-limit-warning";

/**
 * Aggregate sync phase across all accounts. Used by the status-bar dot.
 *
 * Worst-state-wins so a single failing account turns the dot red even when
 * other accounts are happy.
 */
function aggregatePhase(accounts: readonly AccountSyncState[]): SyncPhase {
  if (accounts.length === 0) return "idle";
  const phases = new Set(accounts.map((a) => a.phase));
  if (phases.has("error")) return "error";
  if (phases.has("unauthorized")) return "unauthorized";
  if (phases.has("rate_limited")) return "rate_limited";
  if (phases.has("syncing")) return "syncing";
  if (phases.has("synced")) return "synced";
  return "idle";
}

export const useSyncStore = defineStore("sync", () => {
  const accounts = ref<AccountSyncState[]>([]);
  const intervalSeconds = ref<number>(300);
  const minIntervalSeconds = ref<number>(30);
  const maxIntervalSeconds = ref<number>(600);

  /**
   * The GitHub sub-bucket whose budget tripped the most recent rate-limit
   * warning. Lets the status bar disambiguate "search budget low" from
   * "graphql budget low" - the issue's motivating case is a single
   * bucket bottoming out while the others still have headroom.
   */
  const lastRateLimitResource = ref<RateLimitResource | null>(null);

  /**
   * Shared 1s ticker. Drives `secondsSinceLastSync` and `secondsUntilNextSync`
   * so every consumer of the store updates from the same timer instead of
   * each subscribing its own interval.
   */
  const nowMs = useTimestamp({ interval: 1000 });

  /**
   * Wall-clock time (ms since epoch) when each account's `next_sync_in_seconds`
   * was last received. Lets the UI tick the countdown down live without
   * waiting for the next status event.
   */
  const appliedAtMs = ref<Map<number, number>>(new Map());

  const listener = useTauriListener();

  const aggregate = computed<SyncPhase>(() => aggregatePhase(accounts.value));

  const latestSyncedAt = computed<string | null>(() => {
    let best: string | null = null;
    for (const a of accounts.value) {
      if (a.last_synced_at && (best === null || a.last_synced_at > best)) {
        best = a.last_synced_at;
      }
    }
    return best;
  });

  const secondsSinceLastSync = computed<number | null>(() => {
    if (latestSyncedAt.value === null) return null;
    const synced = Date.parse(latestSyncedAt.value);
    if (Number.isNaN(synced)) return null;
    return Math.max(0, Math.floor((nowMs.value - synced) / 1000));
  });

  const nextSyncInSeconds = computed<number | null>(() => {
    let soonest: number | null = null;
    for (const a of accounts.value) {
      if (a.next_sync_in_seconds === null) continue;
      if (soonest === null || a.next_sync_in_seconds < soonest) {
        soonest = a.next_sync_in_seconds;
      }
    }
    return soonest;
  });

  /**
   * Live countdown to the next sync across all accounts. Anchored to the
   * wall-clock time each account's last status event arrived, so the value
   * ticks down between events instead of holding the stale snapshot number.
   */
  const secondsUntilNextSync = computed<number | null>(() => {
    const now = nowMs.value;
    let soonest: number | null = null;
    for (const a of accounts.value) {
      if (a.next_sync_in_seconds === null) continue;
      const anchor = appliedAtMs.value.get(a.account_id);
      const remaining =
        anchor === undefined
          ? a.next_sync_in_seconds
          : Math.max(0, Math.floor((anchor + a.next_sync_in_seconds * 1000 - now) / 1000));
      if (soonest === null || remaining < soonest) soonest = remaining;
    }
    return soonest;
  });

  /**
   * Lowest rate-budget percentage observed across accounts. The status bar
   * shows the worst-case so a single throttled account doesn't get masked.
   */
  const rateRemainingPct = computed<number | null>(() => {
    let lowest: number | null = null;
    for (const a of accounts.value) {
      if (a.rate_remaining_pct === null) continue;
      if (lowest === null || a.rate_remaining_pct < lowest) {
        lowest = a.rate_remaining_pct;
      }
    }
    return lowest;
  });

  const rateLimit = computed<number | null>(() => {
    for (const a of accounts.value) {
      if (a.rate_limit !== null) return a.rate_limit;
    }
    return null;
  });

  function upsertAccount(next: AccountSyncState): void {
    const idx = accounts.value.findIndex((a) => a.account_id === next.account_id);
    if (idx === -1) {
      accounts.value = [...accounts.value, next];
    } else {
      accounts.value = accounts.value.map((a, i) => (i === idx ? next : a));
    }
  }

  function recordAppliedAt(accountId: number): void {
    const next = new Map(appliedAtMs.value);
    next.set(accountId, Date.now());
    appliedAtMs.value = next;
  }

  function upsertSyncStatus(event: SyncStatusEvent): void {
    upsertAccount({
      account_id: event.account_id,
      phase: event.phase,
      last_synced_at: event.last_synced_at,
      next_sync_in_seconds: event.next_sync_in_seconds,
      message: event.message,
      rate_remaining_pct: event.rate_remaining_pct,
      rate_limit: event.rate_limit,
    });
    recordAppliedAt(event.account_id);
  }

  async function refreshSnapshot(): Promise<void> {
    const snap = await invoke<SyncStatusSnapshot>("get_sync_status");
    const now = Date.now();
    accounts.value = [...snap.accounts];
    const next = new Map<number, number>();
    for (const a of snap.accounts) next.set(a.account_id, now);
    appliedAtMs.value = next;
    intervalSeconds.value = snap.interval_seconds;
    minIntervalSeconds.value = snap.min_interval_seconds;
    maxIntervalSeconds.value = snap.max_interval_seconds;
  }

  async function bind(): Promise<void> {
    await listener.bind(() =>
      Promise.all([
        listen<SyncStatusEvent>(SYNC_STATUS_EVENT, (e) => upsertSyncStatus(e.payload)),
        listen<SyncStatusEvent>(SYNC_ERROR_EVENT, (e) => upsertSyncStatus(e.payload)),
        listen<SyncRateLimitEvent>(SYNC_RATE_LIMIT_EVENT, (e) => {
          // The follow-up status event will carry the full state; here we
          // just surface the warning message so the toast layer can react.
          lastRateLimitResource.value = e.payload.resource ?? null;
          const existing = accounts.value.find((a) => a.account_id === e.payload.account_id);
          if (existing === undefined) return;
          upsertAccount({
            ...existing,
            rate_remaining_pct: e.payload.rate_remaining_pct,
            rate_limit: e.payload.limit ?? existing.rate_limit,
          });
        }),
      ]),
    );
    await refreshSnapshot();
  }

  function unbind(): void {
    listener.unbind();
  }

  async function refreshNow(accountId: number | null = null): Promise<number> {
    const result = await invoke<{ triggered: number }>("refresh_now", {
      input: { account_id: accountId },
    });
    return result.triggered;
  }

  async function setIntervalSeconds(seconds: number): Promise<number> {
    const result = await invoke<{ applied_seconds: number }>("set_sync_interval", {
      input: { seconds },
    });
    intervalSeconds.value = result.applied_seconds;
    return result.applied_seconds;
  }

  return {
    accounts,
    intervalSeconds,
    minIntervalSeconds,
    maxIntervalSeconds,
    aggregate,
    latestSyncedAt,
    secondsSinceLastSync,
    nextSyncInSeconds,
    secondsUntilNextSync,
    rateRemainingPct,
    rateLimit,
    lastRateLimitResource,
    bind,
    unbind,
    refreshSnapshot,
    refreshNow,
    setIntervalSeconds,
  };
});
