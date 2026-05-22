import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Router } from "vue-router";

import { useAppearanceStore, type Density } from "@/stores/appearance";
import type { ChipKey, FilterChipCounts } from "@/types/dashboard";

/**
 * Mirrors `DashboardView` in `src-tauri/src/dashboard/types.rs`. The serde
 * `kebab-case` rename means the Rust command accepts the lowercase variants
 * directly over the Tauri bridge. `archive` (ADR 0018) inverts the default
 * views' archive predicate and orders by `archived_at DESC` server-side
 * when the caller passes `DashboardSort::Updated` (the contract default).
 */
export type DashboardView =
  | "authored"
  | "assigned"
  | "watching"
  | "team"
  | "archive";

/**
 * Mirrors `DashboardSort` in `src-tauri/src/dashboard/types.rs`. M4 widens
 * the union with `"stale"` and `"needs-me"` per
 * `docs/contracts/triage-ux.md` + ADR 0015. The matching backend ORDER BYs
 * land with Wave 3-D; the contract PR pins the wire shape so the sort
 * selector + store can land independently.
 */
export type DashboardSort = "updated" | "stale" | "needs-me";

export type DashboardGroup = "repo" | "org" | "none";

export type ReviewerState =
  | "approved"
  | "changes-requested"
  | "commented"
  | "pending";

export interface CiSummary {
  readonly state: string;
  readonly total: number;
  readonly passing: number;
}

export interface ThreadsSummary {
  readonly total: number;
  readonly unresolved_involved: number;
  readonly unresolved_uninvolved: number;
  readonly resolved_involved: number;
  readonly resolved_uninvolved: number;
}

export interface ReviewerEntry {
  readonly login: string;
  readonly state: ReviewerState;
  readonly is_you: boolean;
  /** GitHub avatar URL for `login`; see ADR 0013. */
  readonly avatar_url: string | null;
}

export interface RepoRef {
  readonly id: number;
  readonly owner: string;
  readonly name: string;
}

export interface DashboardPullRequest {
  readonly id: number;
  readonly number: number;
  readonly title: string;
  readonly url: string;
  readonly state: string;
  readonly is_draft: boolean;
  readonly mergeable: string | null;
  readonly review_decision: string | null;
  readonly author_login: string;
  /** GitHub avatar URL for `author_login`; see ADR 0013. */
  readonly author_avatar_url: string | null;
  readonly base_ref: string;
  readonly head_ref: string;
  readonly created_at: number;
  readonly updated_at: number;
  readonly latest_status_change_at: number | null;
  readonly additions: number | null;
  readonly deletions: number | null;
  readonly changed_files: number | null;
  readonly ci: CiSummary | null;
  readonly threads: ThreadsSummary | null;
  readonly reviewers: readonly ReviewerEntry[];
  readonly repo: RepoRef;
  /**
   * Tracked accounts with a relation to this PR. Sorted ascending. Length 1
   * in the single-account-filter path; 1..N in the unified path; empty for
   * Team-view PRs in the unified path that have no relation rows (the view
   * filter is `repos.is_team_tracked`, not the relations table). The first
   * id is the representative account when consumers need one. See
   * ADR 0016 ("Dashboard row shape - option 1").
   */
  readonly account_ids: readonly number[];
  /** Triage signals - see ADR 0015 and `docs/contracts/triage-ux.md`.
   * In the unified path `unread` and `needs_attention` are merged via MAX
   * across relation owners; `mentioned_count_unread` is summed. */
  readonly unread: boolean;
  readonly needs_attention: boolean;
  readonly mentioned_count_unread: number;
}

export interface DashboardGroupBucket {
  readonly key: string;
  readonly label: string;
  readonly org: string | null;
  readonly items: readonly DashboardPullRequest[];
  readonly latestUpdatedAt: number;
  readonly failingCount: number;
}

interface SyncStatusEvent {
  readonly account_id: number;
  readonly phase: string;
}

/**
 * Per-account failure record from an archive / unarchive fan-out. The store
 * keeps the most recent batch's failures so the view can name the failed
 * accounts in an inline callout while the optimistic flip stays for the
 * successful relations.
 */
export interface ArchiveFailure {
  readonly accountId: number;
  readonly message: string;
}

const SYNC_STATUS_EVENT = "sync://status";
const DASHBOARD_REFRESH_EVENT = "dashboard://refresh";

const VIEW_LABELS: Record<DashboardView, string> = {
  authored: "Authored by me",
  assigned: "Assigned to me",
  watching: "Watching",
  team: "Team",
  archive: "Archive",
};

function bucketKey(pr: DashboardPullRequest, group: DashboardGroup): string {
  switch (group) {
    case "repo":
      return `${pr.repo.owner}/${pr.repo.name}`;
    case "org":
      return pr.repo.owner;
    case "none":
      return "all";
  }
}

function bucketLabel(pr: DashboardPullRequest, group: DashboardGroup): string {
  switch (group) {
    case "repo":
      return `${pr.repo.owner} / ${pr.repo.name}`;
    case "org":
      return pr.repo.owner;
    case "none":
      return "All pull requests";
  }
}

function bucketOrg(
  pr: DashboardPullRequest,
  group: DashboardGroup,
): string | null {
  return group === "repo" ? pr.repo.owner : null;
}

function sortTimestamp(pr: DashboardPullRequest): number {
  return pr.latest_status_change_at ?? pr.updated_at;
}

export const useDashboardStore = defineStore("dashboard", () => {
  const appearance = useAppearanceStore();

  const view = ref<DashboardView>("authored");
  const group = ref<DashboardGroup>("repo");
  const sort = ref<DashboardSort>("updated");
  // M4 triage state. `activeChips` uses a Set for cheap toggle semantics; the
  // Tauri command takes the `Array.from(...)` projection. `searchQuery` is a
  // client-side filter (per the contract's "Search semantics") so we don't
  // round-trip every keystroke. `chipCounts` is `null` while the per-(view,
  // account) fetch is in flight so `FilterChipsBar` can render labels without
  // numbers.
  const activeChips = ref<Set<ChipKey>>(new Set());
  const searchQuery = ref<string>("");
  const chipCounts = ref<FilterChipCounts | null>(null);
  const density = ref<Density>(appearance.density);
  const accountFilter = ref<number | null>(null);

  const pullRequests = ref<DashboardPullRequest[]>([]);
  // IDs that have been optimistically flipped out of the current view by an
  // archive / unarchive action. The list filter drops these so the row fades
  // before the reload arrives; the next `load()` reconciles by reading the
  // canonical state. Cleared on every `load()` so a stale optimistic flip
  // can't survive a sync re-render.
  const pendingArchiveIds = ref<Set<number>>(new Set());
  // Most-recent batch of per-account failures from an archive / unarchive
  // fan-out. `null` when there are no outstanding failures; the dashboard
  // view renders an inline callout off this value and clears it on user
  // dismissal or the next successful action.
  const archiveError = ref<readonly ArchiveFailure[] | null>(null);
  // Per-view counts; refreshed alongside `pullRequests` so the sidebar stays
  // accurate even while a non-current view's list isn't loaded into memory.
  const viewCounts = ref<Record<DashboardView, number>>({
    authored: 0,
    assigned: 0,
    watching: 0,
    team: 0,
    archive: 0,
  });
  const loading = ref(false);
  const lastError = ref<string | null>(null);

  // PR currently expanded in the drawer host. `null` keeps the drawer closed.
  // The `'route'` surface navigates instead of mutating this; the drawer host
  // reads this ref directly to decide its open state.
  const expandedPullRequestId = ref<number | null>(null);

  // Session-local collapsed groups, keyed by the bucket key produced in the
  // `groups` computed. Survives navigation between views (the store lives for
  // the app session) but not a full app restart. Durable persistence across
  // restarts is a follow-up; unknown keys are dropped on rehydrate when that
  // lands. Default is empty (all groups expanded).
  const collapsedGroups = ref<Set<string>>(new Set());

  function setGroupCollapsed(bucketKey: string, collapsed: boolean): void {
    const next = new Set(collapsedGroups.value);
    if (collapsed) next.add(bucketKey);
    else next.delete(bucketKey);
    collapsedGroups.value = next;
  }

  let statusUnlisten: UnlistenFn | null = null;
  let refreshUnlisten: UnlistenFn | null = null;
  // Bumped while the store owns an archive / unarchive fan-out so the
  // `dashboard://refresh` listener skips the backend's per-call emits during
  // a fan-out the store will already settle with a single coalesced reload.
  let inFlightArchiveBatches = 0;

  // Keep the in-store density mirror aligned with the persisted Appearance
  // setting. Mirroring (rather than reading through) keeps the store API
  // self-contained for callers that don't already have the appearance store.
  watch(
    () => appearance.density,
    (next) => {
      density.value = next;
    },
  );

  const viewLabel = computed<string>(() => VIEW_LABELS[view.value]);

  // Client-side search filter applied AFTER the backend's view + chip + sort
  // pass. The dataset is bounded (a few hundred PRs typical) so the contract
  // keeps search in-memory and avoids per-keystroke round-trips. Fields
  // searched: title, `owner/name`, author_login. Case-insensitive substring
  // match per the contract's "Search semantics".
  const filteredPullRequests = computed<DashboardPullRequest[]>(() => {
    const q = searchQuery.value.toLowerCase().trim();
    const pending = pendingArchiveIds.value;
    const dropArchive = (pr: DashboardPullRequest): boolean => !pending.has(pr.id);
    const visible = pending.size === 0
      ? pullRequests.value
      : pullRequests.value.filter(dropArchive);
    if (q === "") return visible;
    return visible.filter((pr) => {
      const repoSlug = `${pr.repo.owner}/${pr.repo.name}`.toLowerCase();
      return (
        pr.title.toLowerCase().includes(q) ||
        repoSlug.includes(q) ||
        pr.author_login.toLowerCase().includes(q)
      );
    });
  });

  const groups = computed<DashboardGroupBucket[]>(() => {
    const buckets = new Map<string, {
      key: string;
      label: string;
      org: string | null;
      items: DashboardPullRequest[];
      latestUpdatedAt: number;
      failingCount: number;
    }>();

    for (const pr of filteredPullRequests.value) {
      const key = bucketKey(pr, group.value);
      const existing = buckets.get(key);
      const ts = sortTimestamp(pr);
      const failing = pr.ci?.state === "FAILURE" ? 1 : 0;
      if (existing === undefined) {
        buckets.set(key, {
          key,
          label: bucketLabel(pr, group.value),
          org: bucketOrg(pr, group.value),
          items: [pr],
          latestUpdatedAt: ts,
          failingCount: failing,
        });
      } else {
        existing.items.push(pr);
        if (ts > existing.latestUpdatedAt) existing.latestUpdatedAt = ts;
        existing.failingCount += failing;
      }
    }

    return Array.from(buckets.values()).sort(
      (a, b) => b.latestUpdatedAt - a.latestUpdatedAt,
    );
  });

  const counts = computed(() => ({ ...viewCounts.value }));

  /**
   * Pull the list for one view _without_ chip filtering. Used to keep
   * `viewCounts` honest: the sidebar count chips reflect the raw view scope
   * so they don't shrink when the user narrows the active view via chips.
   */
  async function fetchView(target: DashboardView): Promise<DashboardPullRequest[]> {
    return await invoke<DashboardPullRequest[]>("list_dashboard_pull_requests", {
      view: target,
      sort: sort.value,
      accountId: accountFilter.value,
      activeChips: null,
    });
  }

  /**
   * Pull the active view _with_ chip filtering applied server-side. Only the
   * row list the user sees needs the chip predicates; the sidebar counts
   * stay raw via [`fetchView`] above.
   */
  async function fetchActiveViewWithChips(
    target: DashboardView,
  ): Promise<DashboardPullRequest[]> {
    return await invoke<DashboardPullRequest[]>("list_dashboard_pull_requests", {
      view: target,
      sort: sort.value,
      accountId: accountFilter.value,
      activeChips: Array.from(activeChips.value),
    });
  }

  /**
   * Refresh the per-chip counts for the active (view, account) pair. Counts
   * are independent of the active chip set per the contract's "Counts rule":
   * each count shows what would match if that chip alone were toggled.
   * Called on view / account / sync-status changes; the result is `null` on
   * failure so the bar gracefully degrades to label-only chips.
   *
   * `accountFilter = null` (the ADR 0016 unified default) fans the count
   * across every tracked account and dedupes by PR id so a PR matched via
   * two accounts contributes one to each chip it triggers.
   */
  async function fetchChipCounts(): Promise<void> {
    try {
      chipCounts.value = await invoke<FilterChipCounts>(
        "list_filter_chip_counts",
        { view: view.value, accountId: accountFilter.value },
      );
    } catch {
      chipCounts.value = null;
    }
  }

  /**
   * Fetches every view in parallel. The active view's rows feed
   * `pullRequests`; the lengths feed the sidebar counts. Five SQL reads per
   * load is the price of accurate sidebar counts (including the Archive
   * entry from ADR 0018) without back-channelling the counts separately
   * from the Rust side.
   */
  async function load(): Promise<void> {
    loading.value = true;
    lastError.value = null;
    // Clear pending optimistic flips before the fetch lands. The canonical
    // state from SQL drives the next paint; any row that should still hide
    // does so via its `archived_at` predicate in the dashboard query.
    if (pendingArchiveIds.value.size > 0) {
      pendingArchiveIds.value = new Set();
    }
    const active = view.value;
    try {
      const [authored, assigned, watching, team, archive] = await Promise.all([
        fetchView("authored"),
        fetchView("assigned"),
        fetchView("watching"),
        fetchView("team"),
        fetchView("archive"),
      ]);
      viewCounts.value = {
        authored: authored.length,
        assigned: assigned.length,
        watching: watching.length,
        team: team.length,
        archive: archive.length,
      };
      const rawActive = (() => {
        switch (active) {
          case "authored":
            return authored;
          case "assigned":
            return assigned;
          case "watching":
            return watching;
          case "team":
            return team;
          case "archive":
            return archive;
        }
      })();
      // The active view fans out to a second fetch only when chips are
      // active. Reusing the unfiltered result keeps the common no-chip path
      // at five calls per load, matching the existing budget.
      //
      // ADR 0018: chips don't apply to the Archive view - the backend
      // panics if a chip predicate reaches it via the chip-count path, and
      // the W2 UI hides the chip rail there. Skipping the chip-filtered
      // re-fetch keeps the Archive view at one call per load even if a
      // stale chip set leaked through.
      const skipChipsForArchive = active === "archive";
      pullRequests.value =
        activeChips.value.size > 0 && !skipChipsForArchive
          ? await fetchActiveViewWithChips(active)
          : rawActive;
      // The Archive view's chip rail is hidden; counts would be zeros
      // regardless. Skip the round-trip.
      if (!skipChipsForArchive) {
        void fetchChipCounts();
      } else {
        chipCounts.value = null;
      }
    } catch (err) {
      lastError.value = formatError(err);
      pullRequests.value = [];
    } finally {
      loading.value = false;
    }
  }

  async function setView(next: DashboardView): Promise<void> {
    if (view.value === next) return;
    view.value = next;
    // View change resets chips + search but preserves sort per the contract.
    // The reset clears the chip state before `load()` so the active view's
    // fetch lands without the previous view's chips bleeding through.
    activeChips.value = new Set();
    searchQuery.value = "";
    await load();
  }

  function setGroup(next: DashboardGroup): void {
    group.value = next;
  }

  function setSort(next: DashboardSort): void {
    if (sort.value === next) return;
    sort.value = next;
    // Sort change re-queries; the backend owns ordering so this is a
    // round-trip, not an in-memory re-sort.
    void load();
  }

  function setDensity(next: Density): void {
    density.value = next;
    appearance.setDensity(next);
  }

  function setAccountFilter(next: number | null): void {
    if (accountFilter.value === next) return;
    accountFilter.value = next;
    void load();
  }

  function toggleChip(key: ChipKey): void {
    const next = new Set(activeChips.value);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    activeChips.value = next;
    void load();
  }

  function clearChips(): void {
    if (activeChips.value.size === 0) return;
    activeChips.value = new Set();
    void load();
  }

  function setSearchQuery(next: string): void {
    searchQuery.value = next;
  }

  function clearFilters(): void {
    // Filtered-empty-state escape hatch: reset both the chip set and the
    // search query, then refetch once so the backend drops chip predicates.
    const hadChips = activeChips.value.size > 0;
    activeChips.value = new Set();
    searchQuery.value = "";
    if (hadChips) void load();
  }

  /**
   * Flip a PR back to unread. `accountId = null` (ADR 0016) tells the Rust
   * command to fan the unread flip out across every relation owner so a
   * merged dashboard row in unified mode flips uniformly. `accountId` set to
   * a specific id keeps the existing single-account semantic (used when the
   * caller explicitly wants to flip one account's read state without
   * touching the others - e.g. a future "mark unread on this account only"
   * affordance).
   *
   * The unified-row affordance defaults to `null`: a user reads "the PR",
   * not "the PR through account X", and the merged row's unread dot should
   * settle the same way the merge aggregated it.
   */
  async function markPullRequestUnread(
    pullRequestId: number,
    accountId: number | null,
  ): Promise<void> {
    // Optimistically flip the dot back on while the Rust write + recompute
    // round-trips. The follow-up reload reconciles `needs_attention` and the
    // canonical mention counter; the dot itself is settled by this flip.
    markRowUnreadOptimistically(pullRequestId);
    try {
      await invoke("mark_pr_unread", { pullRequestId, accountId });
    } finally {
      // The backend recomputes `needs_attention` inside the same transaction,
      // so a single reload puts both the dot and any tint back in step.
      await load();
    }
  }

  /**
   * Archive a PR across the relations the viewer holds for it. The Tauri
   * command takes a single (account, PR) pair so the fan-out happens here:
   * one parallel invoke per relation owner mirrors the Rust mark-read multi
   * shape (ADR 0016) and keeps partial-failure isolation client-side.
   *
   * Optimistic UI: the row is added to `pendingArchiveIds` so it disappears
   * from the active default view's list before the first write resolves. The
   * `dashboard://refresh` event fired by each successful command kicks a
   * reload; the set is cleared inside `load()` so the canonical state from
   * SQL settles the view.
   *
   * Per-account failures collect into `archiveError` so the surface can name
   * the failed accounts. The successful relations' archive state persists -
   * the next reload reflects the partial progress and the failed ids reappear
   * because their `archived_at IS NULL` predicate still holds.
   */
  async function archive(
    pullRequestId: number,
    accountIds: readonly number[],
  ): Promise<void> {
    await runArchiveFanOut("mark_pr_archived", pullRequestId, accountIds);
  }

  /**
   * Unarchive a PR across the archived relation owners. Mirrors [`archive`]
   * with the inverse command. From the Archive view the optimistic flip
   * removes the row from the visible list; the post-write reload then reads
   * the canonical state (the relations that didn't archive cleanly stay in
   * the Archive view).
   */
  async function unarchive(
    pullRequestId: number,
    accountIds: readonly number[],
  ): Promise<void> {
    await runArchiveFanOut("mark_pr_unarchived", pullRequestId, accountIds);
  }

  function dismissArchiveError(): void {
    archiveError.value = null;
  }

  async function runArchiveFanOut(
    command: "mark_pr_archived" | "mark_pr_unarchived",
    pullRequestId: number,
    accountIds: readonly number[],
  ): Promise<void> {
    if (accountIds.length === 0) return;
    archiveError.value = null;
    inFlightArchiveBatches += 1;
    markRowArchivedOptimistically(pullRequestId);
    try {
      const results = await Promise.allSettled(
        accountIds.map((accountId) =>
          invoke(command, { pullRequestId, accountId }).then(() => accountId),
        ),
      );
      const failures: ArchiveFailure[] = [];
      results.forEach((r, i) => {
        if (r.status === "rejected") {
          const accountId = accountIds[i] ?? -1;
          failures.push({
            accountId,
            message: formatError(r.reason),
          });
        }
      });
      if (failures.length > 0) {
        archiveError.value = failures;
        if (failures.length === accountIds.length) {
          // Full failure - revert the optimistic flip so the row stays
          // visible; the next reload reads canonical state anyway, but
          // the explicit revert keeps the row stable across the brief
          // window before `load()` finishes.
          revertOptimisticArchive(pullRequestId);
        }
      }
    } finally {
      // Backend emits `dashboard://refresh` per successful command; the
      // listener skips while `inFlightArchiveBatches > 0` so this single
      // reload coalesces the fan-out.
      await load();
      inFlightArchiveBatches -= 1;
    }
  }

  function markRowArchivedOptimistically(pullRequestId: number): void {
    if (pendingArchiveIds.value.has(pullRequestId)) return;
    const next = new Set(pendingArchiveIds.value);
    next.add(pullRequestId);
    pendingArchiveIds.value = next;
  }

  function revertOptimisticArchive(pullRequestId: number): void {
    if (!pendingArchiveIds.value.has(pullRequestId)) return;
    const next = new Set(pendingArchiveIds.value);
    next.delete(pullRequestId);
    pendingArchiveIds.value = next;
  }

  function markRowUnreadOptimistically(pullRequestId: number): void {
    let touched = false;
    const next = pullRequests.value.map((row) => {
      if (row.id !== pullRequestId) return row;
      if (row.unread) return row;
      touched = true;
      return { ...row, unread: true };
    });
    if (touched) pullRequests.value = next;
  }

  /**
   * Open a PR via the active detail surface from the appearance store.
   * - `'drawer'` sets `expandedPullRequestId` so the drawer host mounts it.
   * - `'route'` navigates to the named `pr-detail` route, preserving the
   *   current view in the URL.
   * - `'inline'` is reserved for a post-M3 follow-up host; we coerce it to
   *   the drawer for now so the runtime path stays valid even if a stale
   *   persisted value sneaks through.
   */
  function openPullRequest(pr: DashboardPullRequest, router: Router): void {
    // Both surfaces (drawer + route) drive `fetch_pr_conversation` on mount,
    // which runs `auto_mark_read` on the Rust side. Optimistically flip the
    // local row so the dot clears in the same paint as the surface opens;
    // the eventual reload reconciles if the write fails.
    markRowReadOptimistically(pr.id);
    if (appearance.prDetailSurface === "route") {
      void router.push({
        name: "pr-detail",
        params: { view: view.value, id: pr.id },
      });
      return;
    }
    expandedPullRequestId.value = pr.id;
  }

  function markRowReadOptimistically(pullRequestId: number): void {
    // Replace the array reference so Vue's shallow ref reactivity fires.
    // The next sync-cycle reload re-reads the canonical state from SQL; this
    // optimistic flip keeps the dot from lingering between open and reload.
    let touched = false;
    const next = pullRequests.value.map((row) => {
      if (row.id !== pullRequestId) return row;
      if (!row.unread && row.mentioned_count_unread === 0) return row;
      touched = true;
      return { ...row, unread: false, mentioned_count_unread: 0 };
    });
    if (touched) pullRequests.value = next;
  }

  function closeExpanded(): void {
    expandedPullRequestId.value = null;
  }

  async function bind(): Promise<void> {
    if (statusUnlisten === null) {
      // Refresh on each completed cycle so the dashboard reflects the latest
      // sync without the user clicking through. The worker emits `synced`
      // once it has finished writing rows for the cycle.
      statusUnlisten = await listen<SyncStatusEvent>(SYNC_STATUS_EVENT, (event) => {
        if (event.payload.phase === "synced") {
          void load();
        }
      });
    }
    if (refreshUnlisten === null) {
      // Triage writes (ADR 0018: archive / unarchive) emit this so the active
      // view reloads without waiting for the next sync tick. The store's own
      // `archive` / `unarchive` actions already trigger a single coalesced
      // reload after their fan-out completes; this listener catches writes
      // that originate outside the store (other windows, future surfaces).
      refreshUnlisten = await listen(DASHBOARD_REFRESH_EVENT, () => {
        if (inFlightArchiveBatches > 0) return;
        void load();
      });
    }
  }

  function unbind(): void {
    if (statusUnlisten !== null) {
      statusUnlisten();
      statusUnlisten = null;
    }
    if (refreshUnlisten !== null) {
      refreshUnlisten();
      refreshUnlisten = null;
    }
  }

  function clearError(): void {
    lastError.value = null;
  }

  return {
    view,
    group,
    sort,
    activeChips,
    searchQuery,
    chipCounts,
    density,
    accountFilter,
    pullRequests,
    filteredPullRequests,
    loading,
    lastError,
    expandedPullRequestId,
    collapsedGroups,
    setGroupCollapsed,
    viewLabel,
    groups,
    counts,
    load,
    setView,
    setGroup,
    setSort,
    setDensity,
    setAccountFilter,
    toggleChip,
    clearChips,
    setSearchQuery,
    clearFilters,
    markPullRequestUnread,
    archive,
    unarchive,
    archiveError,
    dismissArchiveError,
    openPullRequest,
    closeExpanded,
    bind,
    unbind,
    clearError,
  };
});

function formatError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "Couldn't load pull requests.";
}
