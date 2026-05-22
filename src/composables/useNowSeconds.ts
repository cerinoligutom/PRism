import { computed, type ComputedRef } from "vue";
import { useTimestamp } from "@vueuse/core";

/**
 * Shared reactive "now in unix seconds" ref, ticking every 60s. Lets dashboard
 * rows recompute relative time labels and stale strips off a real Vue
 * dependency instead of a `Date.now()` call inside a `computed` (which is not
 * tracked and won't invalidate without an unrelated re-render).
 *
 * Module-level singleton so a list of N rows shares one interval rather than
 * spinning up one per row. `useTimestamp` from VueUse handles the underlying
 * `setInterval` lifecycle (and scope cleanup), and `ShallowRef<number>` is
 * already a reactive source.
 *
 * Interval is 60s because the visible labels round to minute/hour/day - faster
 * ticks would burn cycles without changing what the user sees.
 */
const TICK_INTERVAL_MS = 60_000;

const nowMs = useTimestamp({ interval: TICK_INTERVAL_MS });

const nowSecondsRef: ComputedRef<number> = computed(() =>
  Math.floor(nowMs.value / 1000),
);

/** Returns the shared reactive unix-seconds ref. Safe to call from any setup. */
export function useNowSeconds(): ComputedRef<number> {
  return nowSecondsRef;
}
