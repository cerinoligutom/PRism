<script setup lang="ts">
import { computed } from "vue";
import type { ThreadsSummary } from "@/types/dashboard";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

interface Props {
  threads: ThreadsSummary | null;
}

const props = defineProps<Props>();

type Bucket =
  | "unresolved-uninvolved"
  | "unresolved-involved"
  | "resolved-uninvolved"
  | "resolved-involved";

interface Segment {
  readonly bucket: Bucket;
  readonly count: number;
  readonly width: number;
  readonly tooltip: string;
}

/**
 * Renders the em-dash count + segment-less bar. Backend contract: this fires
 * when the PR has never had a review thread (rollup is `null`) or has had its
 * threads pruned (`total === 0`).
 */
const isEmpty = computed<boolean>(
  () => props.threads === null || props.threads.total === 0,
);

/**
 * Visual muting once nothing on the bar is urgent — applies whenever zero
 * threads are unresolved (the empty case included). The artboard mutes any
 * row whose count reads `0/N`.
 */
const isMuted = computed<boolean>(() => {
  if (isEmpty.value || props.threads === null) return true;
  return (
    props.threads.unresolved_involved + props.threads.unresolved_uninvolved === 0
  );
});

/**
 * The four-bucket bar segments, in display order. Single-thread categories
 * still need to be visible, so any non-zero bucket renders at minimum the
 * sliver width (~5%) - smaller buckets get the floor; larger ones share what's
 * left in proportion to their raw share. See ADR 0012.
 */
const segments = computed<readonly Segment[]>(() => {
  const t = props.threads;
  if (t === null || t.total === 0) return [];
  const raw: { bucket: Bucket; count: number; tooltip: string }[] = (
    [
      ["unresolved-uninvolved", "Unresolved", t.unresolved_uninvolved],
      ["unresolved-involved", "Unresolved (involved)", t.unresolved_involved],
      ["resolved-uninvolved", "Resolved", t.resolved_uninvolved],
      ["resolved-involved", "Resolved (involved)", t.resolved_involved],
    ] as const
  )
    .map(([bucket, label, count]) => ({
      bucket,
      count,
      tooltip: tooltipFor(label, count),
    }))
    .filter((s) => s.count > 0);

  return distributeWidths(raw, t.total);
});

const totalCount = computed<number>(() => props.threads?.total ?? 0);
const unresolvedCount = computed<number>(() => {
  if (props.threads === null) return 0;
  return (
    props.threads.unresolved_involved + props.threads.unresolved_uninvolved
  );
});

function tooltipFor(label: string, count: number): string {
  const noun = count === 1 ? "thread" : "threads";
  return `${label} · ${count} ${noun}`;
}

/**
 * Allocate a 5% sliver floor to any non-zero bucket then proportionally share
 * the remaining width across all buckets based on their raw count. This keeps
 * single-thread buckets visible without crushing the larger ones to zero.
 */
function distributeWidths(
  buckets: readonly { bucket: Bucket; count: number; tooltip: string }[],
  total: number,
): readonly Segment[] {
  const SLIVER_PCT = 5;
  const sliverTotal = SLIVER_PCT * buckets.length;
  const remaining = Math.max(0, 100 - sliverTotal);
  return buckets.map((b) => ({
    bucket: b.bucket,
    count: b.count,
    tooltip: b.tooltip,
    width: SLIVER_PCT + (b.count / total) * remaining,
  }));
}
</script>

<template>
  <div :class="['threads-bar', isMuted && 'threads-bar--muted']">
    <div class="threads-bar__seg" aria-hidden="true">
      <PRismTooltip
        v-for="segment in segments"
        :key="segment.bucket"
        :text="segment.tooltip"
        :as-child="true"
      >
        <div
          :class="['threads-bar__seg-band', `threads-bar__seg-band--${segment.bucket}`]"
          :style="{ width: `${segment.width}%` }"
        ></div>
      </PRismTooltip>
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

.threads-bar__seg-band {
  height: 100%;
}

.threads-bar__seg-band--unresolved-uninvolved {
  background: var(--danger);
}

.threads-bar__seg-band--unresolved-involved {
  background: var(--warning);
}

.threads-bar__seg-band--resolved-uninvolved {
  background: var(--info);
}

.threads-bar__seg-band--resolved-involved {
  background: var(--success);
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
