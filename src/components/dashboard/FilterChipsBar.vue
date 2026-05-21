<script setup lang="ts">
import type { ChipKey, FilterChipCounts } from "@/types/dashboard";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

interface Props {
  /** Per-chip live counts; `null` while loading. The bar still renders the
   * chip labels so the user has something to click before the first count
   * arrives. */
  counts: FilterChipCounts | null;
  /** Set of currently active chip keys. Multi-select; composes as AND. */
  active: ReadonlySet<ChipKey>;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  /** Toggle a single chip; the parent owns the Set. */
  toggle: [key: ChipKey];
  /** Clear-all affordance for the filtered-empty state to call into. */
  clear: [];
}>();

interface ChipDef {
  readonly key: ChipKey;
  readonly label: string;
  /** Maps the chip key to the matching field on `FilterChipCounts`. */
  readonly countField: keyof FilterChipCounts;
}

const chips: readonly ChipDef[] = [
  { key: "needs-attention", label: "Needs my attention", countField: "needs_attention" },
  { key: "unresolved-threads", label: "Unresolved threads", countField: "unresolved_threads" },
  { key: "ci-failing", label: "CI failing", countField: "ci_failing" },
  { key: "stale", label: "Stale", countField: "stale" },
  { key: "drafts", label: "Drafts", countField: "drafts" },
];

/** The four single-line tooltip strings. "Needs my attention" uses the
 * `#content` slot below so it's not in this map. */
const tooltipText: Partial<Record<ChipKey, string>> = {
  "unresolved-threads": "PRs with at least one unresolved review thread.",
  "ci-failing": "PRs whose latest commit's CI rollup is FAILURE or ERROR.",
  stale: "PRs with no activity in the last 7 days.",
  drafts: "Draft PRs.",
};

interface AttentionRow {
  readonly key: string;
  readonly text: string;
}

const attentionRows: readonly AttentionRow[] = [
  { key: "authored-thread", text: "You authored, unresolved thread involves you" },
  { key: "reviewer", text: "You're a requested reviewer (pending)" },
  { key: "mentions", text: "You have unread @mentions" },
  { key: "changes", text: "Changes requested on your PR" },
];

function isActive(key: ChipKey): boolean {
  return props.active.has(key);
}

function countFor(field: keyof FilterChipCounts): number | null {
  return props.counts === null ? null : props.counts[field];
}

function onToggle(key: ChipKey): void {
  emit("toggle", key);
}

defineExpose({
  /** Lets the empty-state component clear all chips without coupling to the
   * store directly. The parent can also call this; matches the contract's
   * `clear` emit shape (the parent decides what "clear" means). */
  clear: () => emit("clear"),
});
</script>

<template>
  <div class="filter-chips" role="group" aria-label="Filter pull requests">
    <span class="filter-chips__label">FILTER</span>
    <PRismTooltip
      v-for="chip in chips"
      :key="chip.key"
      :text="tooltipText[chip.key]"
      :as-child="true"
    >
      <button
        type="button"
        :class="['chip', { active: isActive(chip.key) }]"
        :aria-pressed="isActive(chip.key)"
        @click="onToggle(chip.key)"
      >
        <span>{{ chip.label }}</span>
        <span v-if="countFor(chip.countField) !== null" class="count">
          {{ countFor(chip.countField) }}
        </span>
      </button>
      <template v-if="chip.key === 'needs-attention'" #content>
        <div class="filter-chips__tooltip">
          <div class="filter-chips__tooltip-head">PRs match if any of:</div>
          <ul class="filter-chips__tooltip-list">
            <li
              v-for="row in attentionRows"
              :key="row.key"
              class="filter-chips__tooltip-row"
            >
              {{ row.text }}
            </li>
          </ul>
        </div>
      </template>
    </PRismTooltip>
  </div>
</template>

<style scoped>
.filter-chips {
  display: inline-flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
}

.filter-chips__label {
  font-family: var(--font-mono);
  font-size: 9px;
  color: var(--text-faint);
  letter-spacing: 1px;
  text-transform: uppercase;
  margin-right: 4px;
}
</style>

<!--
  Tooltip body styles live in an unscoped block because Reka's `TooltipPortal`
  teleports the content node to `document.body`, and Vue's scoped `data-v-*`
  attribute selectors don't follow it across the portal. Matches the same
  pattern used by `ReviewerStack.vue` and `ThreadsBar.vue`.
-->
<style>
.filter-chips__tooltip {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 200px;
}

.filter-chips__tooltip-head {
  font-size: var(--fs-11);
  color: var(--text);
  font-weight: 500;
}

.filter-chips__tooltip-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.filter-chips__tooltip-row {
  font-size: var(--fs-11);
  color: var(--text-mute);
  padding-left: 10px;
  position: relative;
}

.filter-chips__tooltip-row::before {
  content: "";
  position: absolute;
  left: 0;
  top: 7px;
  width: 4px;
  height: 4px;
  border-radius: 50%;
  background: var(--text-faint);
}
</style>
