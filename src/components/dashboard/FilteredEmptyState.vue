<script setup lang="ts">
import type { ChipKey } from "@/types/dashboard";

interface Props {
  /** Number of PRs in the active view before search + chip filtering. The
   * empty state surfaces this as "<N> of your <view> PRs are hidden" so the
   * user knows how many rows would return if they cleared the filters. */
  hiddenCount: number;
  /** Human-readable view label, lowercased for inline copy ("authored",
   * "assigned", etc). */
  viewLabel: string;
  /** Currently active chip keys, ordered for stable rendering. The parent
   * passes the chip set; this component only renders + emits drop intents. */
  activeChips: readonly ChipKey[];
  /** The current search query, surfaced as a removable pill alongside the
   * chips so the user can drop it independently. */
  searchQuery: string;
}

defineProps<Props>();

const emit = defineEmits<{
  /** Drop a single chip; the parent reuses the store's `toggleChip` so the
   * intersection is recomputed via the same code path as the chip bar. */
  "drop-chip": [key: ChipKey];
  /** Clear the search query without touching the chip set. */
  "clear-search": [];
  /** Drop every active chip + the search query in one go. Wired to the
   * existing `clearFilters` action so the row list refetches once. */
  "clear-all": [];
}>();

const CHIP_LABELS: Record<ChipKey, string> = {
  "needs-attention": "Needs my attention",
  "unresolved-threads": "Unresolved threads",
  "ci-failing": "CI failing",
  stale: "Stale",
  drafts: "Drafts",
};
</script>

<template>
  <div class="filtered-empty">
    <p class="filtered-empty__head">No PRs match these filters</p>
    <p class="filtered-empty__sub">
      {{ hiddenCount }} of your {{ viewLabel }} PRs are hidden. Try removing a
      filter to see them.
    </p>

    <div
      v-if="activeChips.length > 0 || searchQuery.length > 0"
      class="filtered-empty__pills"
    >
      <button
        v-for="key in activeChips"
        :key="key"
        type="button"
        class="chip active filtered-empty__pill"
        @click="emit('drop-chip', key)"
      >
        <span>{{ CHIP_LABELS[key] }}</span>
        <span class="filtered-empty__pill-x" aria-hidden="true">x</span>
        <span class="sr-only">Remove filter</span>
      </button>
      <button
        v-if="searchQuery.length > 0"
        type="button"
        class="chip active filtered-empty__pill"
        @click="emit('clear-search')"
      >
        <span>Search: {{ searchQuery }}</span>
        <span class="filtered-empty__pill-x" aria-hidden="true">x</span>
        <span class="sr-only">Clear search</span>
      </button>
    </div>

    <div class="filtered-empty__actions">
      <button type="button" class="btn" @click="emit('clear-all')">
        Clear filters
      </button>
    </div>
  </div>
</template>

<style scoped>
.filtered-empty {
  max-width: 420px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--s-3);
}

.filtered-empty__head {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  letter-spacing: -0.3px;
  color: var(--text-strong);
}

.filtered-empty__sub {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
  line-height: var(--lh-body);
}

.filtered-empty__pills {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 6px;
  margin-top: var(--s-1);
}

.filtered-empty__pill {
  height: 22px;
  font-size: var(--fs-10);
}

.filtered-empty__pill-x {
  color: var(--accent-strong);
  opacity: 0.7;
  margin-left: 4px;
}

.filtered-empty__pill:hover .filtered-empty__pill-x {
  opacity: 1;
}

.filtered-empty__actions {
  display: flex;
  gap: var(--s-2);
  margin-top: var(--s-1);
}

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
</style>
