import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Diagnostic activity feed (issue #122).
 *
 * Mirrors the Rust `sync::activity` module: subscribes to the
 * `sync://activity` event for live updates and hydrates the rolling window
 * from `list_recent_activity` on init. Exposes:
 *
 * - `events` — most-recent-first list, capped to match the backend buffer.
 * - `activeCycle` — true when the latest event for any account is mid-cycle
 *   (cycle_started or any phase event without a matching cycle_completed /
 *   cycle_failed since).
 * - `currentPhaseLabel` — the human-readable label for the live ticker. Held
 *   for at least 500ms and updated no more than once per 250ms.
 * - `latestFailure` — the most recent `cycle_failed` event, used by the panel
 *   to auto-open on hover and to highlight the failing row.
 *
 * The throttling logic is extracted to `syncActivity/throttle.ts` so the
 * timing rules can be unit-tested without spinning up a Tauri app.
 */

export type ActivityLevel = "info" | "warn" | "error";

export type SyncPhaseLabel = "discovery" | "enrichment" | "pruning";

export type ActivityKind =
  | { readonly kind: "cycle_started" }
  | { readonly kind: "phase_started"; readonly phase: SyncPhaseLabel }
  | {
      readonly kind: "phase_progress";
      readonly phase: SyncPhaseLabel;
      readonly current: number;
      readonly total: number;
    }
  | {
      readonly kind: "pr_fetched";
      readonly number: number;
      readonly owner: string;
      readonly name: string;
      readonly url: string;
    }
  | {
      readonly kind: "pr_skipped_no_change";
      readonly number: number;
      readonly owner: string;
      readonly name: string;
      readonly url: string;
    }
  | {
      readonly kind: "phase_completed";
      readonly phase: SyncPhaseLabel;
      readonly summary: string;
      /**
       * Number of GraphQL responses skipped via the body-hash cache during
       * the phase (ADR 0004, issue #234). Omitted by the backend when zero.
       */
      readonly cache_skips?: number;
    }
  | {
      readonly kind: "cycle_completed";
      readonly prs_visited: number;
      readonly summary: string;
    }
  | {
      readonly kind: "cycle_failed";
      readonly error_message: string;
      readonly error_kind: string;
    }
  | {
      readonly kind: "rate_limit_pause";
      readonly reset_in_seconds: number;
    };

export interface ActivityEventBase {
  readonly id: number;
  readonly timestamp_ms: number;
  readonly level: ActivityLevel;
  readonly account_id: number | null;
  readonly message: string;
}

export type ActivityEvent = ActivityEventBase & ActivityKind;

export const SYNC_ACTIVITY_EVENT = "sync://activity";

/** Mirrors `activity::BUFFER_CAP` so the frontend doesn't out-grow the backend. */
const BUFFER_CAP = 200;

/** Live-ticker timing constants (issue #122 brief). */
const TICKER_MIN_FRAME_MS = 500;
const TICKER_MIN_INTERVAL_MS = 250;

/**
 * Derive the live-ticker label for one account's latest event.
 *
 * Returns `null` when the event isn't a "cycle in flight" frame
 * (cycle_completed / cycle_failed / rate_limit_pause). Exported for tests
 * and so the throttling layer can ignore frames it would otherwise debounce.
 */
export function phaseLabelFor(event: ActivityEvent, login: string | null): string | null {
  switch (event.kind) {
    case "cycle_started":
      return login ? `Discovering for ${login}...` : "Discovering...";
    case "phase_started":
      switch (event.phase) {
        case "discovery":
          return login ? `Discovering for ${login}...` : "Discovering...";
        case "enrichment":
          return "Fetching detail...";
        case "pruning":
          return "Pruning...";
      }
      return null;
    case "phase_progress":
      if (event.phase === "enrichment") {
        return event.total > 0
          ? `Fetching detail (${event.current}/${event.total})...`
          : `Fetching detail (${event.current})...`;
      }
      return null;
    case "pr_fetched":
      return `Fetching detail (#${event.number})...`;
    case "pr_skipped_no_change":
      return `Skipping #${event.number} (no change)...`;
    case "phase_completed":
    case "cycle_completed":
    case "cycle_failed":
    case "rate_limit_pause":
      return null;
  }
}

/**
 * Whether a cycle is in flight for the activity feed.
 *
 * `true` until a terminal event (cycle_completed / cycle_failed) lands for
 * the same account. Failed cycles flip back to "not in flight" so the live
 * ticker doesn't keep spinning - the panel surfaces the failure instead.
 */
export function isActiveCycleEvent(event: ActivityEvent): boolean {
  switch (event.kind) {
    case "cycle_started":
    case "phase_started":
    case "phase_progress":
    case "pr_fetched":
    case "pr_skipped_no_change":
    case "phase_completed":
      return true;
    case "cycle_completed":
    case "cycle_failed":
    case "rate_limit_pause":
      return false;
  }
}

interface Throttler {
  (next: string | null): void;
  flush(): void;
  dispose(): void;
}

/**
 * Build a throttler that holds each frame for `minFrameMs` and never fires
 * more than once per `minIntervalMs`. The callback is invoked synchronously
 * the first time and then on a trailing timer for subsequent values.
 *
 * Exported for use in the store; the pure rules live in `./syncActivity/throttle.ts`
 * for unit testing.
 */
function makeThrottler(
  apply: (next: string | null) => void,
  minFrameMs = TICKER_MIN_FRAME_MS,
  minIntervalMs = TICKER_MIN_INTERVAL_MS,
): Throttler {
  let lastAppliedAt = 0;
  let lastValue: string | null = null;
  let pendingValue: string | null = null;
  let pendingTimer: ReturnType<typeof setTimeout> | null = null;

  function commit(next: string | null): void {
    lastAppliedAt = Date.now();
    lastValue = next;
    apply(next);
  }

  function schedule(): void {
    if (pendingTimer !== null) return;
    const now = Date.now();
    const sinceLast = now - lastAppliedAt;
    const wait = Math.max(minIntervalMs, minFrameMs - sinceLast);
    pendingTimer = setTimeout(() => {
      pendingTimer = null;
      if (pendingValue === lastValue) return;
      commit(pendingValue);
    }, Math.max(0, wait));
  }

  function fn(next: string | null): void {
    pendingValue = next;
    const sinceLast = Date.now() - lastAppliedAt;
    if (lastAppliedAt === 0 || sinceLast >= minFrameMs) {
      if (pendingTimer !== null) {
        clearTimeout(pendingTimer);
        pendingTimer = null;
      }
      commit(next);
      return;
    }
    schedule();
  }

  fn.flush = (): void => {
    if (pendingTimer !== null) {
      clearTimeout(pendingTimer);
      pendingTimer = null;
    }
    if (pendingValue !== lastValue) {
      commit(pendingValue);
    }
  };

  fn.dispose = (): void => {
    if (pendingTimer !== null) {
      clearTimeout(pendingTimer);
      pendingTimer = null;
    }
  };

  return fn as Throttler;
}

export const useSyncActivityStore = defineStore("syncActivity", () => {
  const events = ref<ActivityEvent[]>([]);
  const tickerLabel = ref<string | null>(null);
  /**
   * The id of the most recent failure event the user has dismissed via
   * a panel open. Used to suppress the auto-open-on-hover behaviour for
   * failures the user has already seen.
   */
  const acknowledgedFailureId = ref<number | null>(null);

  let listeners: UnlistenFn[] = [];
  const throttler = makeThrottler((next) => {
    tickerLabel.value = next;
  });

  /**
   * Most recent event overall (used by the ticker derivation). The backing
   * `events` array is kept in arrival order (most recent first) so this is
   * just the first entry.
   */
  const latest = computed<ActivityEvent | null>(() =>
    events.value.length > 0 ? (events.value[0] as ActivityEvent) : null,
  );

  /**
   * True iff the latest event for ANY account is mid-cycle. Multi-account
   * users see the chip ticker as long as at least one account is syncing.
   */
  const activeCycle = computed<boolean>(() => {
    if (events.value.length === 0) return false;
    // Track per-account: the most recent event per account decides whether
    // it's still mid-cycle. As soon as any account is mid-cycle, we report
    // active.
    const seen = new Set<number | null>();
    for (const evt of events.value) {
      const key = evt.account_id;
      if (seen.has(key)) continue;
      seen.add(key);
      if (isActiveCycleEvent(evt)) return true;
    }
    return false;
  });

  /**
   * The most recent `cycle_failed` event, if any. The panel uses this to
   * highlight the failing row and to drive auto-open-on-hover behaviour.
   */
  const latestFailure = computed<ActivityEvent | null>(() => {
    for (const evt of events.value) {
      if (evt.kind === "cycle_failed") return evt;
      // Stop walking once we hit a fresh terminal success event for the
      // same account - older failures aren't "current".
      if (evt.kind === "cycle_completed") {
        return null;
      }
    }
    return null;
  });

  const currentPhaseLabel = computed<string | null>(() => tickerLabel.value);

  function recomputeTickerFromLatest(loginLookup: (accountId: number | null) => string | null): void {
    const evt = latest.value;
    if (evt === null || !isActiveCycleEvent(evt)) {
      throttler(null);
      return;
    }
    const login = loginLookup(evt.account_id);
    throttler(phaseLabelFor(evt, login));
  }

  function pushEvent(evt: ActivityEvent): void {
    events.value = [evt, ...events.value].slice(0, BUFFER_CAP);
  }

  function applyEvent(evt: ActivityEvent, loginLookup: (accountId: number | null) => string | null): void {
    pushEvent(evt);
    recomputeTickerFromLatest(loginLookup);
  }

  async function hydrate(): Promise<void> {
    try {
      const recent = await invoke<ActivityEvent[]>("list_recent_activity", {
        input: { limit: BUFFER_CAP },
      });
      // `list_recent_activity` returns most-recent-first, matching the local
      // ordering — no reverse needed.
      events.value = [...recent];
    } catch {
      // Hydration failure isn't fatal; the live event channel will refill
      // the buffer as the worker emits.
      events.value = [];
    }
  }

  async function bind(loginLookup: (accountId: number | null) => string | null): Promise<void> {
    if (listeners.length > 0) return;
    listeners = await Promise.all([
      listen<ActivityEvent>(SYNC_ACTIVITY_EVENT, (e) => applyEvent(e.payload, loginLookup)),
    ]);
    await hydrate();
    recomputeTickerFromLatest(loginLookup);
  }

  function unbind(): void {
    for (const off of listeners) off();
    listeners = [];
    throttler.dispose();
  }

  function acknowledgeFailure(): void {
    const failure = latestFailure.value;
    if (failure === null) return;
    acknowledgedFailureId.value = failure.id;
  }

  /**
   * True when there's a current failure the user hasn't dismissed by opening
   * the panel yet. Drives the auto-open-on-hover behaviour.
   */
  const hasUnseenFailure = computed<boolean>(() => {
    const failure = latestFailure.value;
    if (failure === null) return false;
    return acknowledgedFailureId.value !== failure.id;
  });

  return {
    events,
    activeCycle,
    currentPhaseLabel,
    latestFailure,
    hasUnseenFailure,
    bind,
    unbind,
    acknowledgeFailure,
    hydrate,
  };
});
