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
  readonly label: string;
  readonly isResolved: boolean;
}

interface BreakdownRow {
  readonly bucket: Bucket;
  readonly label: string;
  readonly count: number;
  readonly isResolved: boolean;
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
  const raw: {
    bucket: Bucket;
    count: number;
    tooltip: string;
    label: string;
    isResolved: boolean;
  }[] = (
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
      label,
      isResolved: bucket.startsWith("resolved"),
    }))
    .filter((s) => s.count > 0);

  return distributeWidths(raw, t.total);
});

const totalCount = computed<number>(() => props.threads?.total ?? 0);
const resolvedCount = computed<number>(() => {
  if (props.threads === null) return 0;
  return props.threads.resolved_involved + props.threads.resolved_uninvolved;
});

const resolvedPct = computed<number>(() => {
  if (totalCount.value === 0) return 0;
  return Math.round((resolvedCount.value / totalCount.value) * 100);
});

const breakdownRows = computed<readonly BreakdownRow[]>(() => {
  const t = props.threads;
  if (t === null) return [];
  return (
    [
      ["unresolved-uninvolved", "unresolved", t.unresolved_uninvolved],
      ["unresolved-involved", "unresolved (yours)", t.unresolved_involved],
      ["resolved-uninvolved", "resolved", t.resolved_uninvolved],
      ["resolved-involved", "resolved (yours)", t.resolved_involved],
    ] as const
  )
    .filter(([, , count]) => count > 0)
    .map(([bucket, label, count]) => ({
      bucket,
      label,
      count,
      isResolved: bucket.startsWith("resolved"),
    }));
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
  buckets: readonly {
    bucket: Bucket;
    count: number;
    tooltip: string;
    label: string;
    isResolved: boolean;
  }[],
  total: number,
): readonly Segment[] {
  const SLIVER_PCT = 5;
  const sliverTotal = SLIVER_PCT * buckets.length;
  const remaining = Math.max(0, 100 - sliverTotal);
  return buckets.map((b) => ({
    bucket: b.bucket,
    count: b.count,
    tooltip: b.tooltip,
    label: b.label,
    isResolved: b.isResolved,
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
        :as-child="true"
      >
        <div
          :class="['threads-bar__seg-band', `threads-bar__seg-band--${segment.bucket}`]"
          :style="{ width: `${segment.width}%` }"
        ></div>
        <template #content>
          <div class="threads-bar__seg-tip">
            <span
              :class="[
                'threads-bar__badge',
                `threads-bar__badge--${segment.bucket}`,
              ]"
              aria-hidden="true"
            >
              <svg
                v-if="segment.isResolved"
                width="14"
                height="14"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <circle cx="8" cy="8" r="6.25" />
                <path d="M5.25 8.25l2 2 3.5-4" />
              </svg>
              <svg
                v-else
                width="14"
                height="14"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <path
                  d="M2.5 4.5a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2H7l-3 2.5v-2.5H4.5a2 2 0 0 1-2-2V4.5Z"
                />
              </svg>
            </span>
            <div class="threads-bar__seg-tip-text">
              <div class="threads-bar__seg-tip-label">{{ segment.label }}</div>
              <div class="threads-bar__seg-tip-count">
                {{ segment.count }} {{ segment.count === 1 ? "thread" : "threads" }}
              </div>
            </div>
          </div>
        </template>
      </PRismTooltip>
    </div>
    <div class="threads-bar__nums">
      <template v-if="isEmpty">
        <span class="threads-bar__nums-empty">&mdash;</span>
      </template>
      <template v-else>
        <PRismTooltip :as-child="true">
          <span class="threads-bar__nums-count">
            <span>{{ resolvedCount }}</span><span class="threads-bar__nums-denom">/{{ totalCount }}</span>
          </span>
          <template #content>
            <div class="threads-bar__breakdown">
              <div class="threads-bar__breakdown-head">
                {{ resolvedCount }}/{{ totalCount }} resolved ({{ resolvedPct }}%)
              </div>
              <ul
                v-if="breakdownRows.length > 0"
                class="threads-bar__breakdown-list"
              >
                <li
                  v-for="row in breakdownRows"
                  :key="row.bucket"
                  class="threads-bar__breakdown-row"
                >
                  <span
                    :class="[
                      'threads-bar__badge',
                      `threads-bar__badge--${row.bucket}`,
                    ]"
                    aria-hidden="true"
                  >
                    <svg
                      v-if="row.isResolved"
                      width="12"
                      height="12"
                      viewBox="0 0 16 16"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <circle cx="8" cy="8" r="6.25" />
                      <path d="M5.25 8.25l2 2 3.5-4" />
                    </svg>
                    <svg
                      v-else
                      width="12"
                      height="12"
                      viewBox="0 0 16 16"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    >
                      <path
                        d="M2.5 4.5a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2H7l-3 2.5v-2.5H4.5a2 2 0 0 1-2-2V4.5Z"
                      />
                    </svg>
                  </span>
                  <span class="threads-bar__breakdown-count">{{ row.count }}</span>
                  <span class="threads-bar__breakdown-label">{{ row.label }}</span>
                </li>
              </ul>
            </div>
          </template>
        </PRismTooltip>
      </template>
    </div>
  </div>
</template>

<style scoped>
.threads-bar {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  color: var(--text-mute);
  font-size: var(--fs-11);
}

.threads-bar__seg {
  position: relative;
  display: flex;
  align-items: stretch;
  gap: 1px;
  height: 8px;
  flex: 1 1 auto;
  min-width: 0;
  border-radius: var(--r-1);
  background: var(--bg-4);
}

.threads-bar__seg-band {
  height: 100%;
  border-radius: var(--r-1);
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
  flex: 0 0 auto;
}

.threads-bar__nums-count {
  display: inline-flex;
  gap: 1px;
  cursor: default;
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

<!--
  Breakdown styles live in an unscoped block because `PRismTooltip` portals its
  content to `document.body`, so the scoped `data-v-*` attribute selector
  doesn't follow. Matches the pattern used by `ReviewerStack.vue` and
  `PRismTooltip` itself.
-->
<style>
.threads-bar__breakdown {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 180px;
}

.threads-bar__breakdown-head {
  font-size: var(--fs-11);
  color: var(--text);
  font-family: var(--font-mono);
  font-variant-numeric: tabular-nums;
}

.threads-bar__breakdown-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.threads-bar__breakdown-row {
  display: grid;
  grid-template-columns: auto auto 1fr;
  align-items: center;
  gap: 8px;
  font-size: var(--fs-11);
  color: var(--text);
}

/* `.threads-bar__badge` / `--<bucket>` live in
 * `assets/styles/pr-status.css` so the per-segment / breakdown tooltips and
 * the dashboard-row legend share the same swatch identity. */

.threads-bar__seg-tip {
  display: flex;
  align-items: center;
  gap: 10px;
}

.threads-bar__seg-tip-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.threads-bar__seg-tip-label {
  font-size: var(--fs-12);
  color: var(--text-strong);
  font-weight: 600;
}

.threads-bar__seg-tip-count {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
}

.threads-bar__breakdown-count {
  font-family: var(--font-mono);
  font-variant-numeric: tabular-nums;
  color: var(--text);
}

.threads-bar__breakdown-label {
  color: var(--text-mute);
}
</style>
