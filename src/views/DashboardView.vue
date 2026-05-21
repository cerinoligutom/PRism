<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, watch } from "vue";
import { useRoute, useRouter } from "vue-router";

import PRismButton from "@/components/ui/PRismButton.vue";
import PullRequestRow from "@/components/dashboard/PullRequestRow.vue";
import GroupHeader from "@/components/dashboard/GroupHeader.vue";
import DensityToggle from "@/components/dashboard/DensityToggle.vue";
import GroupSelector from "@/components/dashboard/GroupSelector.vue";
import SortSelector from "@/components/dashboard/SortSelector.vue";
import FilterChipsBar from "@/components/dashboard/FilterChipsBar.vue";
import DashboardSearch from "@/components/dashboard/DashboardSearch.vue";
import FilteredEmptyState from "@/components/dashboard/FilteredEmptyState.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import PullRequestDrawer from "@/components/conversation/PullRequestDrawer.vue";
import { useAccountsStore } from "@/stores/accounts";
import {
  useDashboardStore,
  type DashboardGroup,
  type DashboardPullRequest,
  type DashboardSort,
  type DashboardView as DashboardViewName,
} from "@/stores/dashboard";
import type { ChipKey } from "@/types/dashboard";
import type { Density } from "@/stores/appearance";

const route = useRoute();
const router = useRouter();
const dashboard = useDashboardStore();
const accounts = useAccountsStore();

const hasAccounts = computed(() => !accounts.isEmpty);

// Counter semantics per the contract: the first segment reflects the
// post-search PR count so it tracks the visible list as the user types.
const countLabel = computed(() => {
  const total = dashboard.filteredPullRequests.length;
  return total === 1 ? "1 open" : `${total} open`;
});

const activeChipsList = computed<readonly ChipKey[]>(() =>
  Array.from(dashboard.activeChips as Set<ChipKey>),
);

// View labels are nicer than the kebab keys when surfaced in copy ("12 of
// your authored PRs are hidden"). Falls back to the kebab key if a new view
// lands without a label entry.
const VIEW_INLINE_LABEL: Record<DashboardViewName, string> = {
  authored: "authored",
  assigned: "assigned",
  watching: "watching",
  team: "team",
};

const viewInlineLabel = computed<string>(
  () => VIEW_INLINE_LABEL[dashboard.view as DashboardViewName] ?? dashboard.view,
);

// "Hidden" = raw view count minus the visible row count after chips +
// search. The raw count comes from `viewCounts[active]` (the unfiltered
// fetch in `load()`); the visible count is `filteredPullRequests.length`
// after the in-memory search drops rows.
const hiddenCount = computed<number>(() => {
  const raw = dashboard.counts[dashboard.view as DashboardViewName] ?? 0;
  const visible = dashboard.filteredPullRequests.length;
  return Math.max(0, raw - visible);
});

const isFilteredEmpty = computed<boolean>(() => {
  if (dashboard.filteredPullRequests.length > 0) return false;
  return (
    dashboard.activeChips.size > 0 || dashboard.searchQuery.length > 0
  );
});

function routeView(): DashboardViewName | null {
  const meta = route.meta?.dashboardView;
  return typeof meta === "string" ? (meta as DashboardViewName) : null;
}

async function syncFromRoute(): Promise<void> {
  const next = routeView();
  if (next === null) return;
  await dashboard.setView(next);
}

function openPullRequest(pr: DashboardPullRequest): void {
  // Reka's DialogContent captures `document.activeElement` on mount as the
  // restore target, then refocuses it on unmount — so the row that emitted
  // `open` keeps focus when the drawer dismisses without us tracking it.
  dashboard.openPullRequest(pr, router);
}

function closeDrawer(): void {
  dashboard.closeExpanded();
}

async function refresh(): Promise<void> {
  await dashboard.load();
}

function onDensityUpdate(value: Density): void {
  dashboard.setDensity(value);
}

function onGroupUpdate(value: DashboardGroup): void {
  dashboard.setGroup(value);
}

function onSortUpdate(value: DashboardSort): void {
  dashboard.setSort(value);
}

function onSearchUpdate(value: string): void {
  dashboard.setSearchQuery(value);
}

function onToggleChip(key: ChipKey): void {
  dashboard.toggleChip(key);
}

function onClearChips(): void {
  dashboard.clearChips();
}

function onClearSearch(): void {
  dashboard.setSearchQuery("");
}

function onClearAll(): void {
  dashboard.clearFilters();
}

function onMarkUnread(pr: DashboardPullRequest): void {
  void dashboard.markPullRequestUnread(pr.id, pr.account_id);
}

onMounted(async () => {
  await accounts.refresh();
  await dashboard.bind();
  const next = routeView();
  if (next !== null) {
    // Use the bare ref so the initial load runs even when the route's view
    // matches the store default.
    dashboard.view = next;
  }
  await dashboard.load();
});

onBeforeUnmount(() => {
  dashboard.unbind();
});

watch(() => route.meta?.dashboardView, () => {
  void syncFromRoute();
});
</script>

<template>
  <section class="dashboard">
    <header class="dashboard__header">
      <div class="dashboard__top-row">
        <h1 class="dashboard__title">{{ dashboard.viewLabel }}</h1>
        <span class="dashboard__count mono">{{ countLabel }}</span>
        <div class="dashboard__spacer" />
        <DashboardSearch
          :model-value="dashboard.searchQuery"
          @update:model-value="onSearchUpdate"
        />
        <DensityToggle
          :model-value="dashboard.density"
          @update:model-value="onDensityUpdate"
        />
        <PRismTooltip text="Refresh" :as-child="true">
          <button
            type="button"
            class="btn btn-icon"
            :disabled="dashboard.loading"
            @click="refresh"
          >
            <svg
              width="13"
              height="13"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
            >
              <path d="M2 6l2-2a5 5 0 018.5 1M14 10l-2 2a5 5 0 01-8.5-1M2 2v4h4M14 14v-4h-4" />
            </svg>
          </button>
        </PRismTooltip>
      </div>

      <div class="dashboard__chips-row">
        <FilterChipsBar
          :counts="dashboard.chipCounts"
          :active="(dashboard.activeChips as ReadonlySet<ChipKey>)"
          @toggle="onToggleChip"
          @clear="onClearChips"
        />
        <span class="dashboard__chips-sep" aria-hidden="true" />
        <span class="dashboard__chips-label">GROUP</span>
        <GroupSelector
          :model-value="dashboard.group"
          @update:model-value="onGroupUpdate"
        />
        <span class="dashboard__chips-label dashboard__chips-label--gap">SORT</span>
        <SortSelector
          :model-value="(dashboard.sort as DashboardSort)"
          @update:model-value="onSortUpdate"
        />
      </div>
    </header>

    <div v-if="!hasAccounts" class="dashboard__empty">
      <div class="dashboard-empty">
        <span class="dashboard-empty__mark" aria-hidden="true">
          <svg
            width="48"
            height="48"
            viewBox="0 0 32 32"
            fill="none"
            stroke="currentColor"
            stroke-width="1.4"
            stroke-linejoin="round"
            stroke-linecap="round"
          >
            <line x1="2" y1="16" x2="9.5" y2="16" opacity="0.55" />
            <path d="M16 4 L28 26 L4 26 Z" />
          </svg>
        </span>
        <h2 class="dashboard-empty__title">Connect a GitHub account to get started</h2>
        <p class="dashboard-empty__copy">
          PRism watches the pull requests you care about across every repo and
          org you choose. Your token never leaves your machine.
        </p>
        <PRismButton to="/onboarding" variant="primary" size="lg">
          Connect GitHub
        </PRismButton>
      </div>
    </div>

    <div
      v-else-if="dashboard.lastError !== null"
      class="dashboard__notice dashboard__notice--error"
      role="alert"
    >
      <span>{{ dashboard.lastError }}</span>
      <button type="button" class="btn btn-sm" @click="refresh">Try again</button>
    </div>

    <div
      v-else-if="dashboard.loading && dashboard.pullRequests.length === 0"
      class="dashboard__notice"
    >
      Loading pull requests...
    </div>

    <div v-else-if="isFilteredEmpty" class="dashboard__empty">
      <FilteredEmptyState
        :hidden-count="hiddenCount"
        :view-label="viewInlineLabel"
        :active-chips="activeChipsList"
        :search-query="dashboard.searchQuery"
        @drop-chip="onToggleChip"
        @clear-search="onClearSearch"
        @clear-all="onClearAll"
      />
    </div>

    <div
      v-else-if="dashboard.pullRequests.length === 0"
      class="dashboard__empty"
    >
      <div class="dashboard-empty">
        <h2 class="dashboard-empty__title">No pull requests in this view yet</h2>
        <p class="dashboard-empty__copy">
          The next sync cycle will populate this list. You can also refresh
          manually.
        </p>
        <button type="button" class="btn" @click="refresh">Refresh now</button>
      </div>
    </div>

    <div v-else class="dashboard__list scroll">
      <section
        v-for="bucket in dashboard.groups"
        :key="bucket.key"
        class="dashboard__group"
      >
        <GroupHeader
          :label="bucket.label"
          :org="bucket.org"
          :count="bucket.items.length"
          :failing="bucket.failingCount"
          :latest-updated-at="bucket.latestUpdatedAt"
        />
        <PullRequestRow
          v-for="pr in bucket.items"
          :key="`${pr.account_id}:${pr.id}`"
          :pull-request="pr"
          :density="dashboard.density"
          :unread="pr.unread"
          :needs-attention="pr.needs_attention"
          @open="openPullRequest"
          @mark-unread="onMarkUnread"
        />
      </section>
    </div>

    <PullRequestDrawer
      :pull-request-id="dashboard.expandedPullRequestId"
      @close="closeDrawer"
    />
  </section>
</template>

<style scoped>
.dashboard {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.dashboard__header {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  padding: var(--s-5) var(--s-6) var(--s-4);
  border-bottom: 1px solid var(--border-1);
}

.dashboard__top-row {
  display: flex;
  align-items: center;
  gap: var(--s-3);
}

.dashboard__chips-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}

.dashboard__chips-sep {
  width: 1px;
  height: 16px;
  background: var(--border-1);
  margin: 0 4px;
}

.dashboard__chips-label {
  font-family: var(--font-mono);
  font-size: 9px;
  color: var(--text-faint);
  letter-spacing: 1px;
  text-transform: uppercase;
  margin-right: 4px;
}

.dashboard__chips-label--gap {
  margin-left: 8px;
}

.dashboard__title {
  margin: 0;
  font-size: var(--fs-20);
  font-weight: 600;
  letter-spacing: -0.5px;
  color: var(--text-strong);
}

.dashboard__count {
  font-size: var(--fs-12);
  color: var(--text-faint);
}

.dashboard__spacer {
  flex: 1;
}

.dashboard__notice {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  padding: var(--s-4) var(--s-6);
  color: var(--text-mute);
  font-size: var(--fs-12);
}

.dashboard__notice--error {
  color: var(--danger);
}

.dashboard__list {
  flex: 1;
  overflow-y: auto;
  padding: 0 0 var(--s-6);
}

.dashboard__group {
  margin-top: var(--s-2);
}

.dashboard__empty {
  flex: 1;
  display: grid;
  place-items: center;
  padding: var(--s-8) var(--s-6);
}

.dashboard-empty {
  max-width: 420px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--s-3);
}

.dashboard-empty__mark {
  color: var(--text-strong);
  margin-bottom: var(--s-1);
}

.dashboard-empty__title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  letter-spacing: -0.3px;
  color: var(--text-strong);
}

.dashboard-empty__copy {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
  line-height: var(--lh-body);
}
</style>
