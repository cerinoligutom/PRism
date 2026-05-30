<script setup lang="ts">
import { computed, nextTick, onMounted, watch } from "vue";
import { useRoute, useRouter } from "vue-router";

import { openUrl } from "@tauri-apps/plugin-opener";

import PRismButton from "@/components/ui/PRismButton.vue";
import PullRequestRow from "@/components/dashboard/PullRequestRow.vue";
import GroupHeader, {
  type GroupSelectionState,
} from "@/components/dashboard/GroupHeader.vue";
import DensityToggle from "@/components/dashboard/DensityToggle.vue";
import GroupSelector from "@/components/dashboard/GroupSelector.vue";
import SortSelector from "@/components/dashboard/SortSelector.vue";
import FilterChipsBar from "@/components/dashboard/FilterChipsBar.vue";
import DashboardSearch from "@/components/dashboard/DashboardSearch.vue";
import FilteredEmptyState from "@/components/dashboard/FilteredEmptyState.vue";
import AccountPicker from "@/components/dashboard/AccountPicker.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import PRismPopover from "@/components/ui/PRismPopover.vue";
import PRismIconLegend from "@/components/ui/PRismIconLegend.vue";
import PRismCallout from "@/components/ui/PRismCallout.vue";
import ThreadStateIcon from "@/components/conversation/icons/ThreadStateIcon.vue";
import MyReviewStateIcon from "@/components/dashboard/icons/MyReviewStateIcon.vue";
import { SIGNAL_COPY } from "@/components/signals/signalCopy";
import PullRequestDrawer from "@/components/conversation/PullRequestDrawer.vue";
import { useAccountsStore } from "@/stores/accounts";
import { useAppearanceStore } from "@/stores/appearance";
import { useReposStore } from "@/stores/repos";
import { useSyncStore } from "@/stores/sync";
import {
  useDashboardStore,
  type DashboardGroup,
  type DashboardGroupBucket,
  type DashboardPullRequest,
  type DashboardSort,
  type DashboardSortDirection,
  type DashboardView as DashboardViewName,
} from "@/stores/dashboard";
import type { AccountMarker, ChipKey } from "@/types/dashboard";
import type { Density } from "@/stores/appearance";

const route = useRoute();
const router = useRouter();
const dashboard = useDashboardStore();
const accounts = useAccountsStore();
const appearance = useAppearanceStore();
const repos = useReposStore();
const sync = useSyncStore();

const hasAccounts = computed(() => !accounts.isEmpty);

/**
 * Single "is something fetching right now" flag for the dashboard's
 * interactive controls. Includes both the per-route dashboard list
 * hydration (`dashboard.loading`) and the global sync worker cycle
 * (`sync.aggregate === 'syncing'`) so manual Refresh, filter chips, and
 * the status-bar keyboard hint all reflect the same state.
 */
const isFetching = computed(
  () => dashboard.loading || sync.aggregate === "syncing",
);

/**
 * The sync worker is mid-cycle. The "no PRs yet" empty states swap to
 * "syncing now" copy while this is true so a fresh first-run (or any
 * later cycle that lands on an empty view) reads as "we're fetching"
 * instead of "the next cycle will populate this list".
 */
const isSyncing = computed<boolean>(() => sync.aggregate === "syncing");

/**
 * Shared lookup from account id to a render-ready marker. Computed once at
 * the view level so all rows can resolve `pullRequest.account_ids` without
 * each one wiring up the accounts store. See ADR 0016 ("Dashboard row shape
 * - option 1") for the merged-row contract that surfaces these markers.
 */
const accountMarkersById = computed<ReadonlyMap<number, AccountMarker>>(() => {
  const map = new Map<number, AccountMarker>();
  for (const a of accounts.accounts) {
    map.set(a.id, {
      id: a.id,
      label: a.label,
      login: a.login,
      avatar_url: a.avatar_url,
    });
  }
  return map;
});

/**
 * True when there's no meaningful multi-account context to communicate -
 * either the dashboard is scoped to one specific account (the picker
 * already names the scope) or fewer than two accounts are connected
 * (the marker can't tell the user anything they don't already know).
 * In both cases the row marker is suppressed as redundant noise. Unified
 * scope with 2+ accounts renders the marker per ADR 0016.
 */
const isSingleAccountScope = computed<boolean>(
  () => dashboard.accountScope !== null || accounts.accounts.length < 2,
);

// Counter semantics per the contract: the first segment reflects the
// post-search PR count so it tracks the visible list as the user types.
const countLabel = computed(() => {
  const total = dashboard.filteredPullRequests.length;
  return total === 1 ? "1 open" : `${total} open`;
});

const activeChipsList = computed<readonly ChipKey[]>(() =>
  Array.from(dashboard.activeChips as Set<ChipKey>),
);

// Issue #336: gate the "Mark all read" action on the view + chip filter
// having at least one unread / mention. Reads `pullRequests` (the backend's
// view + chip filtered list) instead of `filteredPullRequests` because the
// in-memory search is client-side; the Rust command doesn't know about
// search, so the button state should match what the backend would mark.
const hasUnreadInView = computed<boolean>(() =>
  dashboard.pullRequests.some(
    (pr: DashboardPullRequest) =>
      pr.unread || pr.mentioned_count_unread > 0,
  ),
);

// View labels are nicer than the kebab keys when surfaced in copy ("12 of
// your authored PRs are hidden"). Falls back to the kebab key if a new view
// lands without a label entry.
const VIEW_INLINE_LABEL: Record<DashboardViewName, string> = {
  authored: "authored",
  assigned: "assigned",
  watching: "watching",
  tracked: "tracked",
  archive: "archived",
};

// ADR 0018: the Archive view's chip rail is hidden because the backend's
// chip-count fan-out returns zeros for it (the chip predicates don't apply
// to an archive bucket; closed/merged + 30 days inactive isn't a triage
// queue). Hiding the rail also keeps the empty-state filtering logic from
// claiming the view is "filtered" when it's not.
const isArchive = computed<boolean>(() => dashboard.view === "archive");
const isTracked = computed<boolean>(() => dashboard.view === "tracked");

/**
 * Distinguishes "user has opted into >=1 repo but no involved PRs landed yet"
 * from "user hasn't opted into anything". Drives the Tracked empty state
 * copy. Reads the in-memory repos store, which the dashboard hydrates from
 * `onMounted` so the count is meaningful by the time the empty state shows.
 */
const hasTrackedRepos = computed<boolean>(() => repos.totalTrackedCount > 0);

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

/**
 * Static row scaffold for the dashboard row-icon legend. Label text mirrors the
 * prior inline markup (`&middot;` resolves to the same middle dot); each row's
 * bespoke strip glyph / unread swatch / thread badge / colour swatch comes
 * through `PRismIconLegend`'s `#icon` slot, keyed by `id`. The PR-state strip
 * glyphs stay inline here pending the strip-dissolve slice (F1).
 */
const rowLegendSections = [
  {
    title: "Attention",
    rows: [{ id: "needs", label: SIGNAL_COPY.attention.label }],
  },
  {
    title: "Your review",
    rows: [
      { id: "review-author", label: SIGNAL_COPY.myReview.author.label },
      { id: "review-requested", label: SIGNAL_COPY.myReview.requested.label },
      {
        id: "review-changes-requested",
        label: SIGNAL_COPY.myReview["changes-requested"].label,
      },
      { id: "review-approved", label: SIGNAL_COPY.myReview.approved.label },
      { id: "review-commented", label: SIGNAL_COPY.myReview.commented.label },
    ],
  },
  {
    title: "Unread",
    rows: [{ id: "unread", label: "New activity since you last looked" }],
  },
  {
    title: "Threads",
    rows: [
      { id: "unresolved-uninvolved", label: "Unresolved" },
      { id: "unresolved-involved", label: "Unresolved · you're in it" },
      { id: "resolved-uninvolved", label: "Resolved" },
      { id: "resolved-involved", label: "Resolved · was yours" },
    ],
  },
  {
    title: "Colour key",
    rows: [
      { id: "warm", label: "Warm · involves you" },
      { id: "cool", label: "Cool · others only" },
    ],
  },
] as const;

function isGroupCollapsed(bucketKey: string): boolean {
  return dashboard.collapsedGroups.has(bucketKey);
}

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
  // Also sync the keyboard-targeted focus so `E` immediately after a click
  // or Enter activates the same PR the user opened.
  dashboard.setFocusedPullRequest(pr.id);
  dashboard.openPullRequest(pr, router);
}

function closeDrawer(): void {
  dashboard.closeExpanded();
}

async function refresh(): Promise<void> {
  if (sync.aggregate === "syncing") return;
  await sync.refreshNow(null);
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

function onSortDirectionUpdate(value: DashboardSortDirection): void {
  dashboard.setSortDirection(value);
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
  // ADR 0016: pass `null` so the Rust command fans the flip across every
  // relation owner. In single-account-filter mode the relation set is the
  // active account; in unified mode it's every in-scope account that has a
  // relation row for the PR. Both surfaces converge on a single round-trip
  // that matches the merged row's read semantics.
  void dashboard.markPullRequestUnread(pr.id, null);
}

function onMarkAllRead(): void {
  // Issue #336: the store's `markViewRead` carries the active view + chips +
  // account scope to the Tauri command and reconciles via the post-write
  // reload. No confirmation dialog - the per-row unread toggle is the
  // reversal path.
  void dashboard.markViewRead();
}

function onArchive(pr: DashboardPullRequest): void {
  // ADR 0018 + ADR 0016: the Tauri command takes one (account, PR) per call,
  // so the store fans out across the row's `account_ids`. In default views
  // those are the unarchived relation owners; archiving them is the action.
  void dashboard.archive(pr.id, pr.account_ids);
}

function onUnarchive(pr: DashboardPullRequest): void {
  // Archive-view row: `account_ids` holds the archived relation owners; the
  // unarchive write clears `archived_at` for each one.
  void dashboard.unarchive(pr.id, pr.account_ids);
}

/**
 * Bulk-archive multi-select (#331). Shift+click extends the selection from
 * the last anchor along the visible row order; plain click toggles a single
 * row. The store owns the anchor + range logic so this handler only routes
 * the modifier through.
 */
function onToggleSelect(pr: DashboardPullRequest, shiftKey: boolean): void {
  if (shiftKey) {
    dashboard.extendSelection(pr, dashboard.visibleRowIds);
    return;
  }
  dashboard.toggleSelection(pr);
}

function onCancelSelection(): void {
  dashboard.clearSelection();
}

function onArchiveSelected(): void {
  void dashboard.archiveSelected();
}

function onMarkSelectedRead(): void {
  void dashboard.markSelectedRead();
}

/**
 * Resolve the tri-state for a bucket's select-all checkbox by comparing the
 * bucket's row ids against the live selection set. `"some"` covers any
 * partial overlap so the indeterminate dash renders correctly when the
 * user has flipped individual rows ahead of the group action.
 */
function bucketSelectionState(
  bucket: DashboardGroupBucket,
): GroupSelectionState {
  if (bucket.items.length === 0) return "none";
  const selected = selectedRowIdsSet.value;
  let hit = 0;
  for (const item of bucket.items) {
    if (selected.has(item.id)) hit += 1;
  }
  if (hit === 0) return "none";
  if (hit === bucket.items.length) return "all";
  return "some";
}

function onBucketToggleSelectAll(
  bucket: DashboardGroupBucket,
  next: boolean,
): void {
  if (next) {
    dashboard.selectMany(bucket.items);
    return;
  }
  dashboard.deselectMany(bucket.items.map((item) => item.id));
}

/**
 * Open every PR in the bucket on Unravel. The URL mirrors GitHub's shape
 * on the unravel.sh host - same convention as the per-row Unravel button
 * in `PullRequestRow.vue`. We don't probe the index ahead of time; a 404
 * stays the caller's problem rather than a per-row round-trip.
 *
 * Sequential awaits so the OS receives URLs in the bucket's rendered
 * order; the items list is already sorted via `filteredPullRequests` and
 * its direction modifier, so the resulting tab order mirrors what the
 * user sees on screen.
 */
async function onBucketOpenAllUnravel(
  bucket: DashboardGroupBucket,
): Promise<void> {
  for (const item of bucket.items) {
    const url = `https://www.unravel.sh/${item.repo.owner}/${item.repo.name}/pull/${item.number}`;
    await openUrl(url);
  }
}

async function onBucketOpenAllGitHub(
  bucket: DashboardGroupBucket,
): Promise<void> {
  for (const item of bucket.items) {
    await openUrl(item.url);
  }
}

const selectedCount = computed<number>(() => dashboard.selectedRowIds.size);
const hasSelection = computed<boolean>(() => selectedCount.value > 0);
const selectedRowIdsSet = computed<ReadonlySet<number>>(
  () => dashboard.selectedRowIds,
);

/**
 * True when the current route surfaces the Archive view. Issue #197 lands
 * the route + sidebar entry; this PR keeps the row wiring backward-safe by
 * reading from the route meta directly so the union doesn't need to change
 * ahead of #197. The Tauri-side `DashboardView::Archive` variant already
 * accepts the wire value.
 */
const isArchiveView = computed<boolean>(
  () => route.meta?.dashboardView === "archive",
);

/**
 * Resolve a failed-fan-out account id to a display label. Falls back to the
 * id when the account is no longer in the lookup (e.g. removed between the
 * write and the failure handler).
 */
function archiveErrorAccountLabel(id: number): string {
  const marker = accountMarkersById.value.get(id);
  if (marker === undefined) return `account #${id}`;
  return marker.label || marker.login;
}

function onAccountScopeUpdate(value: number | null): void {
  // Mirror the choice into the persisted appearance store, then drive the
  // dashboard's reactive scope. `setAccountScope` is a no-op when unchanged
  // and triggers `load()` otherwise, so chip counts + rows reload off the
  // existing watch chain.
  appearance.setAccountScope(value);
  dashboard.setAccountScope(value);
}

/**
 * Resolve the persisted scope against the live account set. A stale id whose
 * account was removed since the last session falls back to unified so the
 * first load doesn't query a non-existent account. Called once, before the
 * first `dashboard.load()` fires, to keep the initial fetch aligned with the
 * UI's reported scope.
 */
function restorePersistedScope(): void {
  const persisted = appearance.accountScope;
  if (persisted === null) {
    dashboard.accountScope = null;
    return;
  }
  const stillExists = accounts.accounts.some((a) => a.id === persisted);
  if (stillExists) {
    dashboard.accountScope = persisted;
    return;
  }
  // Reconcile a stale persisted id - clear it so the next persist cycle
  // doesn't carry the dangling reference forward.
  appearance.setAccountScope(null);
  dashboard.accountScope = null;
}

/**
 * Hydrate the repos store for every connected account that hasn't already
 * been loaded this session. The Tracked empty state branches on the tracked
 * count, so it needs the store warm before the branch evaluates; settings
 * already loads these when the user visits Repositories, but the dashboard
 * can't assume the user has been there first.
 */
async function ensureReposHydrated(): Promise<void> {
  const work = accounts.accounts
    .filter((a) => repos.getRepos(a.id).length === 0)
    .map((a) => repos.load(a.id));
  if (work.length > 0) await Promise.all(work);
}

onMounted(async () => {
  await accounts.refresh();
  const next = routeView();
  if (next !== null) {
    // Use the bare ref so the initial load runs even when the route's view
    // matches the store default.
    dashboard.view = next;
  }
  restorePersistedScope();
  // Run the repos hydration alongside the dashboard load so the Tracked
  // empty-state branching (`hasTrackedRepos`) has data the moment the
  // dashboard list resolves to empty - no flicker between the "0 tracked"
  // and "tracked but empty" copies.
  await Promise.all([dashboard.load(), ensureReposHydrated()]);
});

watch(() => route.meta?.dashboardView, () => {
  void syncFromRoute();
});

/**
 * Scroll the keyboard-focused row into view when the focus id changes via
 * arrow keys. `block: 'nearest'` keeps in-viewport rows put; only off-screen
 * targets trigger a scroll. `behavior: 'instant'` avoids queuing animations
 * when the user holds ArrowDown - rapid presses snap instead of chase.
 * The `nextTick` defers until the row's `pr-row--focused` paint lands so
 * the selector resolves against the just-flipped DOM.
 */
watch(
  () => dashboard.focusedPullRequestId,
  async (id) => {
    if (id === null) return;
    await nextTick();
    const row = document.querySelector(`[data-row-pr-id="${id}"]`);
    if (row !== null) {
      row.scrollIntoView({ block: "nearest", behavior: "instant" });
    }
  },
);
</script>

<template>
  <section class="dashboard">
    <header class="dashboard__header">
      <div class="dashboard__top-row">
        <div class="dashboard__heading">
          <div class="dashboard__title-row">
            <h1 class="dashboard__title">{{ dashboard.viewLabel }}</h1>
            <span class="dashboard__count mono">{{ countLabel }}</span>
          </div>
          <p class="dashboard__subtitle">{{ dashboard.viewSubtitle }}</p>
        </div>
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
            :disabled="isFetching"
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
        <PRismPopover :as-child="true" side="bottom" align="end">
          <button
            type="button"
            class="btn btn-icon dashboard__legend-btn"
            aria-label="Row icon legend"
          >
            <svg
              width="13"
              height="13"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.5"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <circle cx="8" cy="8" r="6.5" />
              <line x1="8" y1="11.25" x2="8" y2="7.25" />
              <circle cx="8" cy="5" r="0.6" fill="currentColor" stroke="none" />
            </svg>
          </button>
          <template #content>
            <PRismIconLegend
              region-label="Row icon legend"
              :sections="rowLegendSections"
              :min-width="240"
            >
              <template #icon="{ id }">
                <span v-if="id === 'needs'" class="dashboard-legend__attention-swatch" aria-hidden="true">
                  <span class="dashboard-legend__attention-dot"></span>
                </span>
                <span v-else-if="id === 'review-author'" class="pr-row__state my-review--author" aria-hidden="true">
                  <MyReviewStateIcon state="author" />
                </span>
                <span v-else-if="id === 'review-requested'" class="pr-row__state my-review--requested" aria-hidden="true">
                  <MyReviewStateIcon state="requested" />
                </span>
                <span v-else-if="id === 'review-changes-requested'" class="pr-row__state my-review--changes-requested" aria-hidden="true">
                  <MyReviewStateIcon state="changes-requested" />
                </span>
                <span v-else-if="id === 'review-approved'" class="pr-row__state my-review--approved" aria-hidden="true">
                  <MyReviewStateIcon state="approved" />
                </span>
                <span v-else-if="id === 'review-commented'" class="pr-row__state my-review--commented" aria-hidden="true">
                  <MyReviewStateIcon state="commented" />
                </span>
                <span v-else-if="id === 'unread'" class="dashboard-legend__unread-swatch" aria-hidden="true">
                  <span class="dashboard-legend__unread-title">Aa</span>
                </span>
                <span v-else-if="id === 'unresolved-uninvolved'" class="threads-bar__badge threads-bar__badge--unresolved-uninvolved" aria-hidden="true">
                  <ThreadStateIcon state="unresolved" :size="12" />
                </span>
                <span v-else-if="id === 'unresolved-involved'" class="threads-bar__badge threads-bar__badge--unresolved-involved" aria-hidden="true">
                  <ThreadStateIcon state="unresolved" :size="12" />
                </span>
                <span v-else-if="id === 'resolved-uninvolved'" class="threads-bar__badge threads-bar__badge--resolved-uninvolved" aria-hidden="true">
                  <ThreadStateIcon state="resolved" :size="12" />
                </span>
                <span v-else-if="id === 'resolved-involved'" class="threads-bar__badge threads-bar__badge--resolved-involved" aria-hidden="true">
                  <ThreadStateIcon state="resolved" :size="12" />
                </span>
                <span v-else-if="id === 'warm'" class="legend-swatch-pair" aria-hidden="true">
                  <span class="legend-swatch legend-swatch--warning"></span>
                  <span class="legend-swatch legend-swatch--success"></span>
                </span>
                <span v-else-if="id === 'cool'" class="legend-swatch-pair" aria-hidden="true">
                  <span class="legend-swatch legend-swatch--danger"></span>
                  <span class="legend-swatch legend-swatch--info"></span>
                </span>
              </template>
            </PRismIconLegend>
          </template>
        </PRismPopover>
      </div>

      <div class="dashboard__chips-row">
        <template v-if="hasAccounts">
          <span class="dashboard__chips-label">SCOPE</span>
          <AccountPicker
            :accounts="accounts.accounts"
            :model-value="dashboard.accountScope"
            @update:model-value="onAccountScopeUpdate"
          />
          <span class="dashboard__chips-sep" aria-hidden="true" />
        </template>
        <FilterChipsBar
          v-if="!isArchive"
          :counts="dashboard.chipCounts"
          :active="(dashboard.activeChips as ReadonlySet<ChipKey>)"
          :disabled="isFetching"
          @toggle="onToggleChip"
          @clear="onClearChips"
        />
        <span v-if="!isArchive" class="dashboard__chips-sep" aria-hidden="true" />
        <span class="dashboard__chips-label">GROUP</span>
        <GroupSelector
          :model-value="dashboard.group"
          @update:model-value="onGroupUpdate"
        />
        <span class="dashboard__chips-label dashboard__chips-label--gap">SORT</span>
        <SortSelector
          :model-value="(dashboard.sort as DashboardSort)"
          :direction="(dashboard.sortDirection as DashboardSortDirection)"
          @update:model-value="onSortUpdate"
          @update:direction="onSortDirectionUpdate"
        />
        <div class="dashboard__chips-spacer" />
        <PRismTooltip
          :text="hasUnreadInView
            ? 'Clear unread and mention dots on every PR in this view.'
            : 'No unread PRs to mark in this view.'"
          :as-child="true"
        >
          <button
            type="button"
            class="btn btn-sm"
            :disabled="!hasUnreadInView || isFetching"
            @click="onMarkAllRead"
          >
            Mark all read
          </button>
        </PRismTooltip>
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
      v-else-if="dashboard.loading && dashboard.pullRequests.length === 0 && !isSyncing"
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
      <div v-if="isArchive" class="dashboard-empty">
        <h2 class="dashboard-empty__title">No archived pull requests</h2>
        <p class="dashboard-empty__copy">
          PRs land here when they close or merge, or when you archive them
          from the row overflow menu.
        </p>
      </div>
      <div v-else-if="isTracked && !hasTrackedRepos" class="dashboard-empty">
        <h2 class="dashboard-empty__title">No tracked repositories yet</h2>
        <p class="dashboard-empty__copy">
          The Tracked view shows PRs from repositories you've opted in.
          You pick the repos yourself so API budget stays under control.
          A separate Teams-driven view lands in M8.
        </p>
        <PRismButton to="/settings/repositories" variant="primary">
          Open Repositories settings
        </PRismButton>
      </div>
      <div v-else-if="isTracked && isSyncing" class="dashboard-empty">
        <h2 class="dashboard-empty__title">Syncing your tracked repositories...</h2>
        <p class="dashboard-empty__copy">
          PRism is fetching pull requests from the repos you've opted in. Rows
          will appear here as soon as the first cycle finishes.
        </p>
      </div>
      <div v-else-if="isTracked" class="dashboard-empty">
        <h2 class="dashboard-empty__title">No pull requests in your tracked repositories yet</h2>
        <p class="dashboard-empty__copy">
          None of your tracked repos currently have PRs you're involved with.
          If you expected to see PRs here, double-check you're an author,
          reviewer, or mentioned on the PR.
        </p>
        <PRismButton to="/settings/repositories" variant="primary">
          Manage tracked repositories
        </PRismButton>
      </div>
      <div v-else-if="isSyncing" class="dashboard-empty">
        <h2 class="dashboard-empty__title">Syncing your pull requests...</h2>
        <p class="dashboard-empty__copy">
          PRism is fetching the latest from GitHub. Rows will appear here as
          soon as the first cycle finishes.
        </p>
      </div>
      <div v-else class="dashboard-empty">
        <h2 class="dashboard-empty__title">No pull requests in this view yet</h2>
        <p class="dashboard-empty__copy">
          The next sync cycle will populate this list. You can also refresh
          manually.
        </p>
        <button type="button" class="btn" @click="refresh">Refresh now</button>
      </div>
    </div>

    <div v-else class="dashboard__list scroll">
      <div
        v-if="hasSelection && !isArchive"
        class="dashboard__bulk-toolbar"
        role="region"
        aria-label="Bulk selection actions"
      >
        <span class="dashboard__bulk-count">
          {{ selectedCount === 1 ? "1 selected" : `${selectedCount} selected` }}
        </span>
        <div class="dashboard__bulk-actions">
          <PRismButton
            variant="ghost"
            size="sm"
            :disabled="isFetching"
            @click="onMarkSelectedRead"
          >
            Mark as read
          </PRismButton>
          <PRismButton
            variant="primary"
            size="sm"
            :disabled="isFetching"
            @click="onArchiveSelected"
          >
            Archive selected
          </PRismButton>
          <PRismButton variant="ghost" size="sm" @click="onCancelSelection">
            Cancel
          </PRismButton>
        </div>
      </div>

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
          :collapsed="isGroupCollapsed(bucket.key)"
          :selectable="!isArchive"
          :selection-state="bucketSelectionState(bucket)"
          @update:collapsed="(value: boolean) => dashboard.setGroupCollapsed(bucket.key, value)"
          @toggle-select-all="(value: boolean) => onBucketToggleSelectAll(bucket, value)"
          @open-all-unravel="() => { void onBucketOpenAllUnravel(bucket); }"
          @open-all-github="() => { void onBucketOpenAllGitHub(bucket); }"
        />
        <TransitionGroup
          v-show="!isGroupCollapsed(bucket.key)"
          name="dashboard-row"
          tag="div"
          class="dashboard__rows"
        >
          <PullRequestRow
            v-for="pr in bucket.items"
            :key="`${pr.account_ids.join('-')}:${pr.id}`"
            :pull-request="pr"
            :density="dashboard.density"
            :unread="pr.unread"
            :needs-attention="pr.needs_attention"
            :accounts-by-id="accountMarkersById"
            :single-account-scope="isSingleAccountScope"
            :is-archive-view="isArchiveView"
            :focused="pr.id === dashboard.focusedPullRequestId"
            :selected="selectedRowIdsSet.has(pr.id)"
            :selection-active="hasSelection"
            @open="openPullRequest"
            @mark-unread="onMarkUnread"
            @archive="onArchive"
            @unarchive="onUnarchive"
            @toggle-select="onToggleSelect"
          />
        </TransitionGroup>
      </section>
    </div>

    <div
      v-if="dashboard.archiveError !== null && dashboard.archiveError.length > 0"
      class="dashboard__archive-error"
    >
      <PRismCallout variant="danger">
        <strong>Archive failed for {{ dashboard.archiveError.length === 1 ? "one account" : `${dashboard.archiveError.length} accounts` }}.</strong>
        <span class="dashboard__archive-error-accounts">
          {{ dashboard.archiveError.map((f: { accountId: number }) => archiveErrorAccountLabel(f.accountId)).join(", ") }}
        </span>
        <button
          type="button"
          class="dashboard__archive-error-dismiss"
          @click="dashboard.dismissArchiveError()"
        >
          Dismiss
        </button>
      </PRismCallout>
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

/* Heading wraps the title row + the subtitle so the right-side controls
 * (search, density, etc.) keep aligning to the centre of the title row,
 * not to the column's midpoint once the subtitle is in play. */
.dashboard__heading {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.dashboard__title-row {
  display: flex;
  align-items: center;
  gap: var(--s-3);
}

.dashboard__subtitle {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-tight);
}

.dashboard__chips-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
}

.dashboard__chips-spacer {
  flex: 1;
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

.dashboard__archive-error {
  padding: var(--s-3) var(--s-6) var(--s-3);
  border-top: 1px solid var(--border-1);
}

.dashboard__archive-error-accounts {
  margin-left: 6px;
  color: var(--text-mute);
}

.dashboard__archive-error-dismiss {
  background: transparent;
  border: 0;
  padding: 0 0 0 var(--s-3);
  color: inherit;
  font-weight: 500;
  cursor: pointer;
  text-decoration: underline;
}

.dashboard__archive-error-dismiss:hover {
  color: var(--text-strong);
}

.dashboard__archive-error-dismiss:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: 2px;
  border-radius: 2px;
}

.dashboard__list {
  flex: 1;
  overflow-y: auto;
  padding: 0 0 var(--s-6);
}

/* Sticky bulk-archive toolbar. Sits at the top of the scrolling list so it
 * stays visible while the user scrolls through a long selection. Matches the
 * existing `.dashboard__header` density - same horizontal padding, accent
 * border to read as an active mode banner rather than a notification. */
.dashboard__bulk-toolbar {
  position: sticky;
  top: 0;
  z-index: 5;
  display: flex;
  align-items: center;
  gap: var(--s-3);
  padding: var(--s-2) var(--s-6);
  background: var(--accent-bg);
  border-bottom: 1px solid var(--border-2);
}

.dashboard__bulk-count {
  font-size: var(--fs-13);
  font-weight: 500;
  color: var(--text-strong);
  flex: 1;
}

.dashboard__bulk-actions {
  display: flex;
  align-items: center;
  gap: 6px;
}

.dashboard__group {
  margin-top: var(--s-2);
}

.dashboard__rows {
  display: contents;
}

/* Optimistic archive flip: fade + collapse the row out of the list while the
 * fan-out completes. The leave-active class drives the animation; the leave-to
 * class is the final state. `dashboard__rows` uses `display: contents` so the
 * wrapping div doesn't break the row's grid alignment with the bucket header. */
.dashboard-row-leave-active {
  transition:
    opacity 140ms ease,
    transform 140ms ease;
}

.dashboard-row-leave-to {
  opacity: 0;
  transform: translateX(8px);
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

/* Mirrors the conversation header legend trigger - accent tint so it reads
 * as "tap me for help" rather than chrome. Other chrome comes from
 * `.btn.btn-icon` in `primitives.css`. */
.dashboard__legend-btn {
  color: var(--accent);
}

.dashboard__legend-btn:hover:not(:disabled) {
  color: var(--accent-strong);
}
</style>

<!--
  Unscoped because Reka's `PopoverPortal` teleports legend content out of the
  scoped-CSS attribute boundary. The container / section / row scaffold now
  comes from `PRismIconLegend`; only the dashboard-specific unread swatch
  survives here. `.pr-row__state` shape + variant palettes come from
  `assets/styles/pr-status.css`; ThreadsBar bucket badges come from
  `ThreadsBar.vue`'s own unscoped block (loaded on every PR row).
-->
<style>
/* Attention cue mirrors the dashboard row's 10px column + 7px centred accent
 * dot so the swatch looks like the real thing. */
.dashboard-legend__attention-swatch,
.dashboard-legend__unread-swatch {
  width: 22px;
  height: 22px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.dashboard-legend__attention-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent-strong);
}

/* Unread now reads as a bolder title (the dot is the attention affordance,
 * ADR 0031), so the swatch shows weighted glyphs rather than a dot. */
.dashboard-legend__unread-title {
  font-size: var(--fs-13);
  font-weight: 600;
  color: var(--text-strong);
  line-height: 1;
}
</style>
