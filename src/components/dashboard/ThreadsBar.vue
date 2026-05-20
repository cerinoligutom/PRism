<script setup lang="ts">
import { computed } from "vue";
import type { ThreadsSummary } from "@/types/dashboard";

interface Props {
  threads: ThreadsSummary | null;
}

const props = defineProps<Props>();

interface SegmentWidths {
  readonly unresolved: number;
  readonly involved: number;
  readonly resolved: number;
}

const ZERO_WIDTHS: SegmentWidths = {
  unresolved: 0,
  involved: 0,
  resolved: 0,
} as const;

/**
 * Renders the em-dash count + segment-less bar. Backend contract: this fires
 * when the PR has never had a review thread (rollup is `null`) or has had its
 * threads pruned (`total === 0`).
 */
const isEmpty = computed<boolean>(
  () => props.threads === null || props.threads.total === 0,
);

/**
 * Visual muting once nothing on the bar is urgent — applies whenever there
 * are zero unresolved threads (the empty case included). The artboard mutes
 * any row whose count reads `0/N`.
 */
const isMuted = computed<boolean>(
  () => isEmpty.value || (props.threads?.unresolved ?? 0) === 0,
);

/**
 * Segment widths expressed as percentages of the bar. The three segments are
 * rendered as disjoint slices so the bar reads at a glance:
 *
 *   - `unresolved` claims the open-thread share of `total`.
 *   - `involved` shows only the *resolved* threads the active account has
 *     commented on (their "settled contributions" footprint). Capped at the
 *     resolved share so the segments stay disjoint.
 *   - `resolved` fills the remaining settled / outdated threads.
 *
 * Backend `involved` overlaps both states, so any thread that is both
 * unresolved and involved counts toward `unresolved` here.
 */
const segments = computed<SegmentWidths>(() => {
  if (props.threads === null || props.threads.total === 0) {
    return ZERO_WIDTHS;
  }
  const { total, unresolved, involved } = props.threads;
  const resolvedTotal = Math.max(0, total - unresolved);
  const involvedSegment = Math.min(involved, resolvedTotal);
  const resolvedSegment = resolvedTotal - involvedSegment;
  return {
    unresolved: (unresolved / total) * 100,
    involved: (involvedSegment / total) * 100,
    resolved: (resolvedSegment / total) * 100,
  };
});

const unresolvedCount = computed<number>(() => props.threads?.unresolved ?? 0);
const totalCount = computed<number>(() => props.threads?.total ?? 0);
</script>

<template>
  <div :class="['threads-bar', isMuted && 'threads-bar--muted']">
    <div class="threads-bar__seg" aria-hidden="true">
      <div
        v-if="segments.unresolved > 0"
        class="threads-bar__seg-unresolved"
        :style="{ width: `${segments.unresolved}%` }"
      ></div>
      <div
        v-if="segments.involved > 0"
        class="threads-bar__seg-involved"
        :style="{ width: `${segments.involved}%` }"
      ></div>
      <div
        v-if="segments.resolved > 0"
        class="threads-bar__seg-resolved"
        :style="{ width: `${segments.resolved}%` }"
      ></div>
    </div>
    <div class="threads-bar__nums">
      <template v-if="isEmpty">
        <span class="threads-bar__nums-empty">&mdash;</span>
      </template>
      <template v-else>
        <span>{{ unresolvedCount }}</span>
        <span class="threads-bar__nums-denom">/{{ totalCount }}</span>
      </template>
    </div>
  </div>
</template>

<style scoped>
.threads-bar {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--text-mute);
  font-size: var(--fs-11);
}

.threads-bar__seg {
  position: relative;
  display: flex;
  height: 6px;
  width: 90px;
  border-radius: var(--r-1);
  overflow: hidden;
  background: var(--bg-4);
  flex: 0 0 auto;
}

.threads-bar__seg-unresolved {
  background: var(--accent);
  height: 100%;
}

.threads-bar__seg-involved {
  background: var(--info);
  height: 100%;
}

.threads-bar__seg-resolved {
  background: var(--success);
  height: 100%;
}

.threads-bar__nums {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text);
  font-variant-numeric: tabular-nums;
  display: inline-flex;
  gap: 1px;
}

.threads-bar__nums-denom {
  color: var(--text-faint);
}

.threads-bar__nums-empty {
  color: var(--text-faint);
}

.threads-bar--muted .threads-bar__nums {
  color: var(--text-mute);
}
</style>
