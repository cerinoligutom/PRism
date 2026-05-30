import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Router } from "vue-router";

import { useAppearanceStore, type Density } from "@/stores/appearance";
import { useTauriListener } from "@/composables/useTauriListener";
import { dashboardRouteName } from "@/router";
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
  | "tracked"
  | "archive";

/**
 * Mirrors `DashboardSort` in `src-tauri/src/dashboard/types.rs`. M4 widens
 * the union with `"stale"` and `"needs-me"` per
 * `docs/contracts/triage-ux.md` + ADR 0015. The matching backend ORDER BYs
 * land with Wave 3-D; the contract PR pins the wire shape so the sort
 * selector + store can land independently.
 */
export type DashboardSort = "updated" | "stale" | "needs-me";

/**
 * Client-side direction modifier for the active sort. The Tauri command still
 * owns the underlying ORDER BY (per `DashboardSort`); this just toggles a
 * post-fetch reversal so the user can flip newest/oldest within each group
 * without round-tripping the backend. `"desc"` is the natural direction every
 * sort emits from SQL ("updated" = newest first, etc.); `"asc"` reverses
 * inside `filteredPullRequests` so the grouped buckets inherit it for free.
 */
export type DashboardSortDirection = "asc" | "desc";

export type DashboardGroup = "repo" | "org" | "none";

export type ReviewerState =
  | "approved"
  | "changes-requested"
  | "commented"
  | "pending";

/**
 * Mirrors `MyReviewState` in `src-tauri/src/dashboard/types.rs`. The viewer's
 * own relationship to the PR, derived server-side (ADR 0031). Precedence
 * (highest wins): author > requested > changes-requested > approved >
 * commented > none.
 */
export type MyReviewState =
  | "author"
  | "requested"
  | "changes-requested"
  | "approved"
  | "commented"
  | "none";

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
  /** The viewer's own relationship to the PR for the my-review-state row slot.
   * Derived server-side with a fixed precedence; host-gated. See ADR 0031. */
  readonly my_review_state: MyReviewState;
  readonly repo: RepoRef;
  /**
   * Tracked accounts with a relation to this PR. Sorted ascending. Length 1
   * in the single-account-filter path; 1..N in the unified path; empty for
   * Tracked-view PRs in the unified path that have no relation rows (the
   * view filter is `repos.is_tracked`, not the relations table). The first
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

/**
 * Mirrors `DashboardViewCounts` in `src-tauri/src/dashboard/types.rs`. Powers
 * the sidebar count chips via one Tauri invoke per load instead of a per-view
 * list fan-out (M7 perf, issue #230).
 */
interface DashboardViewCountsPayload {
  readonly authored: number;
  readonly assigned: number;
  readonly watching: number;
  readonly tracked: number;
  readonly archive: number;
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

const DASHBOARD_REFRESH_EVENT = "dashboard://refresh";

const VIEW_LABELS: Record<DashboardView, string> = {
  authored: "Authored by me",
  assigned: "Review requested",
  watching: "Watching",
  tracked: "Tracked",
  archive: "Archive",
};

// One-line explainer per view, surfaced as a subtitle under the main title.
// "Assigned to me" was renamed to "Review requested" because the GitHub
// vocabulary for `is_review_requested` is "review request", not "assignee"
// (which is a different field PRism doesn't surface in v1).
const VIEW_SUBTITLES: Record<DashboardView, string> = {
  authored: "Pull requests you opened.",
  assigned: "Pull requests awaiting your review.",
  watching: "Pull requests you've commented on, reviewed, or been mentioned in.",
  tracked: "All open pull requests in repositories you follow.",
  archive: "Pull requests you've archived. Activity moves them back to the active list.",
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

// Group-bucket aggregation reads `updated_at` directly so the "active X ago"
// chip in `GroupHeader` matches the freshest row's time cell. Comments + pushes
// count as activity, which is the intuitive read of the label. The dashboard
// list sort is a separate concern and uses its own predicate elsewhere.
function sortTimestamp(pr: DashboardPullRequest): number {
  return pr.updated_at;
}

export const useDashboardStore = defineStore("dashboard", () => {
  const appearance = useAppearanceStore();

  const view = ref<DashboardView>("authored");
  const group = ref<DashboardGroup>("repo");
  const sort = ref<DashboardSort>("updated");
  const sortDirection = ref<DashboardSortDirection>("desc");
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
  const accountScope = ref<number | null>(null);

  const pullRequests = ref<DashboardPullRequest[]>([]);
  // IDs that have been optimistically flipped out of the current view by an
  // archive / unarchive action. The list filter drops these so the row fades
  // before the reload arrives; the next `load()` reconciles by reading the
  // canonical state. Cleared on every `load()` so a stale optimistic flip
  // can't survive a sync re-render.
  const optimisticallyArchivedIds = ref<Set<number>>(new Set());
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
    tracked: 0,
    archive: 0,
  });
  const loading = ref(false);
  const lastError = ref<string | null>(null);

  // PR currently expanded in the drawer host. `null` keeps the drawer closed.
  // The `'route'` surface navigates instead of mutating this; the drawer host
  // reads this ref directly to decide its open state.
  const expandedPullRequestId = ref<number | null>(null);

  // Row receiving keyboard-shortcut targeting (e.g. `E` to archive). `null`
  // means no row is focused, in which case row-targeted shortcuts no-op. The
  // ref tracks the PR id so it survives sort and chip churn that reorders
  // the underlying list; the `groups` computed restores the highlight as
  // long as the PR is still visible. Reset on view change so the next view
  // starts unfocused rather than inheriting a stale id from the previous one.
  const focusedPullRequestId = ref<number | null>(null);

  // Bulk multi-select state (#331). `selectedRows` keys PR ids to the row
  // payload so cross-view bulk ops (mark as read, archive selected) can run
  // against rows the active view has unloaded. The dashboard renders a
  // sticky toolbar above the table while non-empty. `lastSelectedId`
  // anchors Shift+click range extension so a contiguous slice of
  // `visibleRowIds` flips together. Selection persists across view-switches
  // so the user can stage a batch from multiple sidebar tabs; it resets
  // after a successful bulk write.
  const selectedRows = ref<Map<number, DashboardPullRequest>>(new Map());
  const lastSelectedId = ref<number | null>(null);

  const selectedRowIds = computed<ReadonlySet<number>>(() => {
    const ids = new Set<number>();
    for (const id of selectedRows.value.keys()) ids.add(id);
    return ids;
  });

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

  function setFocusedPullRequest(id: number | null): void {
    focusedPullRequestId.value = id;
  }

  /**
   * Advance the focus through the visible rows in the order they appear in
   * `groups` (the post-search, post-chip projection - what the user sees).
   * `delta = 1` moves down, `-1` moves up. With no focus yet, the first call
   * lands on the first row going forward or the last going back. Clamps at
   * each end rather than wrapping so the highlight doesn't "teleport" past
   * the visible list edges.
   */
  function moveFocus(delta: 1 | -1): void {
    const order = visibleRowIds.value;
    if (order.length === 0) {
      focusedPullRequestId.value = null;
      return;
    }
    const current = focusedPullRequestId.value;
    if (current === null) {
      focusedPullRequestId.value = delta === 1 ? order[0]! : order[order.length - 1]!;
      return;
    }
    const idx = order.indexOf(current);
    if (idx === -1) {
      focusedPullRequestId.value = delta === 1 ? order[0]! : order[order.length - 1]!;
      return;
    }
    const next = Math.max(0, Math.min(order.length - 1, idx + delta));
    focusedPullRequestId.value = order[next]!;
  }

  const listener = useTauriListener();
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
  const viewSubtitle = computed<string>(() => VIEW_SUBTITLES[view.value]);

  // Client-side search filter applied AFTER the backend's view + chip + sort
  // pass. The dataset is bounded (a few hundred PRs typical) so the contract
  // keeps search in-memory and avoids per-keystroke round-trips. Fields
  // searched: title, `owner/name`, author_login. Case-insensitive substring
  // match per the contract's "Search semantics".
  const filteredPullRequests = computed<DashboardPullRequest[]>(() => {
    const q = searchQuery.value.toLowerCase().trim();
    const pending = optimisticallyArchivedIds.value;
    const dropArchive = (pr: DashboardPullRequest): boolean => !pending.has(pr.id);
    const visible = pending.size === 0
      ? pullRequests.value
      : pullRequests.value.filter(dropArchive);
    const searched = q === ""
      ? visible
      : visible.filter((pr) => {
          const repoSlug = `${pr.repo.owner}/${pr.repo.name}`.toLowerCase();
          return (
            pr.title.toLowerCase().includes(q) ||
            repoSlug.includes(q) ||
            pr.author_login.toLowerCase().includes(q)
          );
        });
    // Ascending = reverse the natural backend order. The reversal happens
    // here so the grouped buckets (built off this list) inherit the
    // direction for free; bucket ordering itself still tracks the freshest
    // activity descending.
    return sortDirection.value === "asc" ? [...searched].reverse() : searched;
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
   * Flat list of PR ids in the order they render across `groups`. Drives
   * arrow-key focus traversal so the highlight follows the same buckets the
   * user reads, including collapsed groups (whose ids stay in order but
   * aren't visually present - moving onto them feels right because the
   * collapse is a session-level affordance, not a content filter).
   */
  const visibleRowIds = computed<readonly number[]>(() => {
    const ids: number[] = [];
    for (const bucket of groups.value) {
      for (const item of bucket.items) {
        ids.push(item.id);
      }
    }
    return ids;
  });

  /**
   * Pull the list for the active view _without_ chip filtering. The sidebar
   * count chips come from [`fetchViewCounts`] instead of five list calls, so
   * the only caller now is the no-chips path of [`load`].
   */
  async function fetchView(target: DashboardView): Promise<DashboardPullRequest[]> {
    return await invoke<DashboardPullRequest[]>("list_dashboard_pull_requests", {
      view: target,
      sort: sort.value,
      accountId: accountScope.value,
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
      accountId: accountScope.value,
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
   * `accountScope = null` (the ADR 0016 unified default) fans the count
   * across every tracked account and dedupes by PR id so a PR matched via
   * two accounts contributes one to each chip it triggers.
   */
  async function fetchChipCounts(): Promise<void> {
    try {
      chipCounts.value = await invoke<FilterChipCounts>(
        "list_filter_chip_counts",
        { view: view.value, accountId: accountScope.value },
      );
    } catch {
      chipCounts.value = null;
    }
  }

  /**
   * Pull the five view counts in one Tauri invoke. The backend computes
   * `SELECT COUNT(*)` per view sub-query against predicates that mirror
   * `list_dashboard_pull_requests` so each field equals the length of the
   * matching per-view list (M7 perf, issue #230).
   */
  async function fetchViewCounts(): Promise<DashboardViewCountsPayload> {
    return await invoke<DashboardViewCountsPayload>(
      "list_dashboard_view_counts",
      { accountId: accountScope.value },
    );
  }

  /**
   * Fetches the sidebar view counts and the active view's row list. M7 perf
   * (issue #230) collapses the previous five-way `list_dashboard_pull_requests`
   * fan-out into one `list_dashboard_view_counts` call so each cycle now
   * blocks on two invokes (counts + active view) instead of five or six. The
   * chip count fetch still fires as a fire-and-forget background invoke.
   *
   * ADR 0018: chips don't apply to the Archive view - the backend
   * panics if a chip predicate reaches it via the chip-count path, and
   * the W2 UI hides the chip rail there. Skipping the chip-filtered
   * re-fetch keeps the Archive view at one list call per load even if a
   * stale chip set leaked through.
   */
  async function load(): Promise<void> {
    loading.value = true;
    lastError.value = null;
    // Clear pending optimistic flips before the fetch lands. The canonical
    // state from SQL drives the next paint; any row that should still hide
    // does so via its `archived_at` predicate in the dashboard query.
    if (optimisticallyArchivedIds.value.size > 0) {
      optimisticallyArchivedIds.value = new Set();
    }
    const active = view.value;
    const skipChipsForArchive = active === "archive";
    const fetchActive = activeChips.value.size > 0 && !skipChipsForArchive
      ? fetchActiveViewWithChips(active)
      : fetchView(active);
    try {
      const [counts, activeRows] = await Promise.all([
        fetchViewCounts(),
        fetchActive,
      ]);
      viewCounts.value = {
        authored: counts.authored,
        assigned: counts.assigned,
        watching: counts.watching,
        tracked: counts.tracked,
        archive: counts.archive,
      };
      pullRequests.value = activeRows;
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
    // Drop any focused row so the new view doesn't inherit a highlight for
    // a PR that almost certainly isn't in the next list.
    focusedPullRequestId.value = null;
    // Bulk selection survives view switches so a user can stage a batch
    // across multiple sidebar tabs (selecting in Authored, swinging through
    // Watching, then marking-read the lot from one toolbar). The selected
    // payloads ride along in `selectedRows` so the bulk action paths don't
    // need the previous view's rows to still be in `pullRequests`.
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

  function setSortDirection(next: DashboardSortDirection): void {
    if (sortDirection.value === next) return;
    sortDirection.value = next;
  }

  function toggleSortDirection(): void {
    sortDirection.value = sortDirection.value === "desc" ? "asc" : "desc";
  }

  function setDensity(next: Density): void {
    density.value = next;
    appearance.setDensity(next);
  }

  function setAccountScope(next: number | null): void {
    if (accountScope.value === next) return;
    accountScope.value = next;
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
   * Mark every PR in the active view + chip filter as read (issue #336). The
   * backend takes the same `(view, chips, account_id)` tuple the dashboard
   * list query uses, so the user marks what they see.
   *
   * Optimistic flip: every visible row's `unread` + `mentioned_count_unread`
   * are cleared in-memory before the round-trip lands. The post-write reload
   * reconciles `needs_attention` and the canonical counters; the next sync
   * cycle can re-raise an unread state if a comment lands between the click
   * and the reload, which is the same race the per-row flip already accepts.
   *
   * Returns the number of distinct PRs the backend touched - the caller can
   * fold the value into a toast or status copy without reading it out of the
   * store.
   */
  async function markViewRead(): Promise<number> {
    const targetView = view.value;
    const chips = Array.from(activeChips.value);
    markVisibleRowsReadOptimistically();
    try {
      const marked = await invoke<number>("mark_view_read", {
        view: targetView,
        accountId: accountScope.value,
        chips,
      });
      return marked;
    } finally {
      await load();
    }
  }

  /**
   * Optimistically flip the in-memory `unread` / `mentioned_count_unread`
   * fields on every visible row. The reload that follows reads the canonical
   * state; this just keeps the dashboard from showing stale dots in the
   * window between the invoke and the refresh.
   */
  function markVisibleRowsReadOptimistically(): void {
    let touched = false;
    const next = pullRequests.value.map((row) => {
      if (!row.unread && row.mentioned_count_unread === 0) return row;
      touched = true;
      return { ...row, unread: false, mentioned_count_unread: 0 };
    });
    if (touched) pullRequests.value = next;
  }

  /**
   * Archive a PR across the relations the viewer holds for it. The Tauri
   * command takes a single (account, PR) pair so the fan-out happens here:
   * one parallel invoke per relation owner mirrors the Rust mark-read multi
   * shape (ADR 0016) and keeps partial-failure isolation client-side.
   *
   * Optimistic UI: the row is added to `optimisticallyArchivedIds` so it disappears
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

  /**
   * Toggle the bulk-selection state for one PR row (#331). Updates
   * `lastSelectedId` so a subsequent Shift+click can extend a range from
   * here. The full payload rides into `selectedRows` so cross-view bulk
   * actions still know each row's `account_ids` and URLs after the user
   * has switched sidebar tabs.
   */
  function toggleSelection(pr: DashboardPullRequest): void {
    const next = new Map(selectedRows.value);
    if (next.has(pr.id)) {
      next.delete(pr.id);
    } else {
      next.set(pr.id, pr);
    }
    selectedRows.value = next;
    lastSelectedId.value = pr.id;
  }

  /**
   * Extend the selection between `lastSelectedId` and `pr` along the
   * visible row order (#331). Every id in the slice flips to checked; ids
   * outside the slice keep their existing state so the user can compose
   * multiple ranges. With no anchor yet (`lastSelectedId === null`) this
   * falls back to a plain toggle so the first Shift+click still feels
   * responsive instead of being a silent no-op.
   */
  function extendSelection(
    pr: DashboardPullRequest,
    orderedIds: readonly number[],
  ): void {
    const anchor = lastSelectedId.value;
    if (anchor === null) {
      toggleSelection(pr);
      return;
    }
    const fromIdx = orderedIds.indexOf(anchor);
    const toIdx = orderedIds.indexOf(pr.id);
    if (fromIdx === -1 || toIdx === -1) {
      toggleSelection(pr);
      return;
    }
    const [lo, hi] = fromIdx <= toIdx ? [fromIdx, toIdx] : [toIdx, fromIdx];
    const idToRow = new Map<number, DashboardPullRequest>();
    for (const row of pullRequests.value) idToRow.set(row.id, row);
    const next = new Map(selectedRows.value);
    for (let i = lo; i <= hi; i++) {
      const id = orderedIds[i];
      if (id === undefined) continue;
      const row = idToRow.get(id);
      if (row !== undefined) next.set(id, row);
    }
    selectedRows.value = next;
    lastSelectedId.value = pr.id;
  }

  /**
   * Add every PR in `rows` to the selection. Used by the group-header
   * "select all in this group" affordance so a single click stages every
   * visible row in a bucket. Idempotent for rows already selected.
   */
  function selectMany(rows: readonly DashboardPullRequest[]): void {
    if (rows.length === 0) return;
    const next = new Map(selectedRows.value);
    for (const row of rows) next.set(row.id, row);
    selectedRows.value = next;
    const last = rows[rows.length - 1];
    if (last !== undefined) lastSelectedId.value = last.id;
  }

  /**
   * Drop every id in `ids` from the selection. Mirrors [`selectMany`] for
   * the inverse path (clearing a group's selection from the header).
   */
  function deselectMany(ids: readonly number[]): void {
    if (ids.length === 0) return;
    const next = new Map(selectedRows.value);
    let touched = false;
    for (const id of ids) {
      if (next.delete(id)) touched = true;
    }
    if (!touched) return;
    selectedRows.value = next;
    if (lastSelectedId.value !== null && !next.has(lastSelectedId.value)) {
      lastSelectedId.value = null;
    }
  }

  function clearSelection(): void {
    if (selectedRows.value.size === 0 && lastSelectedId.value === null) {
      return;
    }
    selectedRows.value = new Map();
    lastSelectedId.value = null;
  }

  /**
   * Archive every PR in the current `selectedRowIds`. Fans the write out by
   * account: for each tracked account holding a relation row for any
   * selected PR, invokes `mark_prs_archived(pull_request_ids, account_id)`
   * with the subset that account holds. The single SQL UPDATE per account
   * keeps a 30-row selection at one round-trip per account, not 30.
   *
   * Optimistic flip: every selected id lands in `optimisticallyArchivedIds`
   * up-front so the rows fade before the writes resolve. Per-account
   * failures collect into `archiveError` so the surface can name them; the
   * post-write `load()` reconciles canonical state. Selection clears once
   * the dispatch completes (matching the acceptance criterion).
   */
  async function archiveSelected(): Promise<void> {
    if (selectedRows.value.size === 0) return;
    const selectedIds = Array.from(selectedRows.value.keys());
    // Group the selected PR ids by relation owner so each account gets one
    // batched invoke. Rows with no `account_ids` (Tracked-view PRs with no
    // relation in unified scope) can't archive - they're skipped silently
    // for the same reason the per-row `canArchive` guard hides the menu
    // entry on them. Reads from `selectedRows` instead of `pullRequests` so
    // rows the user staged from a different view (now unloaded) still
    // contribute their `account_ids` to the fan-out.
    const byAccount = new Map<number, number[]>();
    for (const row of selectedRows.value.values()) {
      for (const accountId of row.account_ids) {
        const list = byAccount.get(accountId);
        if (list === undefined) byAccount.set(accountId, [row.id]);
        else list.push(row.id);
      }
    }
    if (byAccount.size === 0) {
      clearSelection();
      return;
    }
    archiveError.value = null;
    inFlightArchiveBatches += 1;
    for (const id of selectedIds) markRowArchivedOptimistically(id);
    try {
      const entries = Array.from(byAccount.entries());
      const results = await Promise.allSettled(
        entries.map(([accountId, ids]) =>
          invoke("mark_prs_archived", {
            pullRequestIds: ids,
            accountId,
          }).then(() => accountId),
        ),
      );
      const failures: ArchiveFailure[] = [];
      results.forEach((r, i) => {
        if (r.status === "rejected") {
          const accountId = entries[i]?.[0] ?? -1;
          failures.push({
            accountId,
            message: formatError(r.reason),
          });
        }
      });
      if (failures.length > 0) {
        archiveError.value = failures;
        if (failures.length === entries.length) {
          // Full failure across every account - revert the optimistic
          // flips so the rows stay visible. The post-write reload would
          // re-add them anyway, but the explicit revert keeps the table
          // stable in the brief window before `load()` settles.
          for (const id of selectedIds) revertOptimisticArchive(id);
        }
      }
      clearSelection();
    } finally {
      await load();
      inFlightArchiveBatches -= 1;
    }
  }

  /**
   * Mark every currently-selected PR as read. Reads from `selectedRows` so
   * the action works across view switches - a PR staged in Authored still
   * marks-read after the user has flipped to Watching. Fans the write out
   * per PR with `account_id: null` so the Rust command settles every
   * relation owner in one transaction, matching the per-row "Mark unread"
   * path in reverse.
   *
   * Optimistic flip: every selected row's `unread` /
   * `mentioned_count_unread` clears locally before the round-trip lands so
   * the dots disappear in the same paint as the click. The post-write
   * reload reconciles `needs_attention` and any race-window state.
   */
  async function markSelectedRead(): Promise<void> {
    if (selectedRows.value.size === 0) return;
    const selectedIds = Array.from(selectedRows.value.keys());
    for (const id of selectedIds) markRowReadOptimistically(id);
    try {
      await Promise.allSettled(
        selectedIds.map((pullRequestId) =>
          invoke("mark_pr_read", { pullRequestId, accountId: null }),
        ),
      );
      clearSelection();
    } finally {
      await load();
    }
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
    if (optimisticallyArchivedIds.value.has(pullRequestId)) return;
    const next = new Set(optimisticallyArchivedIds.value);
    next.add(pullRequestId);
    optimisticallyArchivedIds.value = next;
  }

  function revertOptimisticArchive(pullRequestId: number): void {
    if (!optimisticallyArchivedIds.value.has(pullRequestId)) return;
    const next = new Set(optimisticallyArchivedIds.value);
    next.delete(pullRequestId);
    optimisticallyArchivedIds.value = next;
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

  /**
   * Open the drawer surface for `pullRequestId` directly. The deep-link
   * router (#339) reaches for this when the appearance setting routes the
   * detail surface to the drawer; the in-app row click goes through
   * `openPullRequest`, which needs a full `DashboardPullRequest` payload
   * the deep-link path doesn't have.
   */
  function setExpandedPullRequest(pullRequestId: number | null): void {
    expandedPullRequestId.value = pullRequestId;
  }

  /**
   * Open a PR from an external entry point (desktop notification click,
   * `prism://` deep link, persistent inbox row click) via the active detail
   * surface from the appearance store (issue #410).
   *
   * Behaviour matches the in-app `openPullRequest` row click:
   * - `'route'` pushes the named `pr-detail` route with the resolver's view.
   * - `'drawer'` pushes the matching dashboard route first (the drawer host
   *   mounts on `DashboardView`), then sets `expandedPullRequestId` so the
   *   drawer opens on the next paint.
   *
   * Sets account scope before routing so the back-navigation lands on a list
   * that contains the deep-linked row. `setAccountScope` is a no-op when the
   * scope already matches and triggers a single `load()` otherwise.
   *
   * The caller resolves coordinates and applies any side effects it owns
   * (mark-read on the inbox path, `pr_lookup_by_coordinates` fallback on the
   * deep-link miss path) before invoking this helper. Returns a Promise so
   * callers can await the view change when they need to sequence work after
   * the navigation lands.
   */
  async function openPrFromExternal(
    target: {
      readonly pullRequestId: number;
      readonly accountId: number;
      readonly view: DashboardView;
    },
    router: Router,
  ): Promise<void> {
    setAccountScope(target.accountId);
    if (appearance.prDetailSurface === "route") {
      await router.push({
        name: "pr-detail",
        params: { view: target.view, id: target.pullRequestId },
      });
      return;
    }
    await router.push({ name: dashboardRouteName(target.view) });
    expandedPullRequestId.value = target.pullRequestId;
  }

  async function bind(): Promise<void> {
    await listener.bind(() =>
      Promise.all([
        // ADR 0029: one refresh signal for every cache-mutating write. The
        // sync worker emits this at the end of each successful cycle, and
        // triage commands (archive / mark-read) emit it on their commits.
        // In-flight fan-out batches suppress the reload so the store's own
        // coalesced reload runs once at the end.
        listen(DASHBOARD_REFRESH_EVENT, () => {
          if (inFlightArchiveBatches > 0) return;
          void load();
        }),
      ]),
    );
  }

  function unbind(): void {
    listener.unbind();
  }

  function clearError(): void {
    lastError.value = null;
  }

  return {
    view,
    group,
    sort,
    sortDirection,
    activeChips,
    searchQuery,
    chipCounts,
    density,
    accountScope,
    pullRequests,
    filteredPullRequests,
    loading,
    lastError,
    expandedPullRequestId,
    focusedPullRequestId,
    collapsedGroups,
    setGroupCollapsed,
    setFocusedPullRequest,
    moveFocus,
    viewLabel,
    viewSubtitle,
    groups,
    visibleRowIds,
    counts,
    load,
    setView,
    setGroup,
    setSort,
    setSortDirection,
    toggleSortDirection,
    setDensity,
    setAccountScope,
    toggleChip,
    clearChips,
    setSearchQuery,
    clearFilters,
    markPullRequestUnread,
    markViewRead,
    archive,
    unarchive,
    archiveError,
    dismissArchiveError,
    selectedRows,
    selectedRowIds,
    lastSelectedId,
    toggleSelection,
    extendSelection,
    selectMany,
    deselectMany,
    clearSelection,
    archiveSelected,
    markSelectedRead,
    openPullRequest,
    openPrFromExternal,
    closeExpanded,
    setExpandedPullRequest,
    bind,
    unbind,
    clearError,
  };
});

/**
 * Discriminated union mirroring the Rust error enums that back the dashboard
 * and triage Tauri commands. The shape comes from
 * `#[serde(tag = "kind", rename_all = "snake_case")]` on
 * `DashboardCommandError` (`src-tauri/src/dashboard/commands.rs`) and
 * `TriageCommandError` (`src-tauri/src/triage/commands.rs`). Both error
 * surfaces share `Internal`; the dashboard surface adds `NotFound` for the
 * route-metadata lookup.
 */
type DashboardLikeCommandError =
  | { kind: "internal" }
  | { kind: "not_found" };

/**
 * Translates the structured Rust error into a single user-facing message.
 * Mirrors `formatAuthError` in `src/stores/accounts.ts`. Falls back to the
 * generic dashboard message when the payload isn't one of the kinds we know
 * about, so a future variant doesn't render the raw object.
 */
function formatError(err: unknown): string {
  if (typeof err === "object" && err !== null && "kind" in err) {
    const tagged = err as DashboardLikeCommandError;
    switch (tagged.kind) {
      case "not_found":
        return "We couldn't find that pull request.";
      case "internal":
        return "Couldn't load pull requests. Check the application logs.";
    }
  }
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "Couldn't load pull requests.";
}
