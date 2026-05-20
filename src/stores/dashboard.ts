import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { useAppearanceStore, type Density } from "@/stores/appearance";

/**
 * Mirrors `DashboardView` in `src-tauri/src/dashboard/types.rs`. The serde
 * `kebab-case` rename means the Rust command accepts the lowercase variants
 * directly over the Tauri bridge.
 */
export type DashboardView = "authored" | "assigned" | "watching" | "team";

/**
 * Mirrors `DashboardSort` in `src-tauri/src/dashboard/types.rs`. M2 ships
 * `Updated` only; the union widens in M4 when the sort selector lands.
 */
export type DashboardSort = "updated";

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

export interface ReviewerEntry {
  readonly login: string;
  readonly state: ReviewerState;
  readonly is_you: boolean;
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
  readonly base_ref: string;
  readonly head_ref: string;
  readonly created_at: number;
  readonly updated_at: number;
  readonly latest_status_change_at: number | null;
  readonly additions: number | null;
  readonly deletions: number | null;
  readonly changed_files: number | null;
  readonly ci: CiSummary | null;
  readonly reviewers: readonly ReviewerEntry[];
  readonly repo: RepoRef;
  readonly account_id: number;
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

const SYNC_STATUS_EVENT = "sync://status";

const VIEW_LABELS: Record<DashboardView, string> = {
  authored: "Authored by me",
  assigned: "Assigned to me",
  watching: "Watching",
  team: "Team",
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
  const density = ref<Density>(appearance.density);
  const accountFilter = ref<number | null>(null);

  const pullRequests = ref<DashboardPullRequest[]>([]);
  // Per-view counts; refreshed alongside `pullRequests` so the sidebar stays
  // accurate even while a non-current view's list isn't loaded into memory.
  const viewCounts = ref<Record<DashboardView, number>>({
    authored: 0,
    assigned: 0,
    watching: 0,
    team: 0,
  });
  const loading = ref(false);
  const lastError = ref<string | null>(null);

  let statusUnlisten: UnlistenFn | null = null;

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

  const groups = computed<DashboardGroupBucket[]>(() => {
    const buckets = new Map<string, {
      key: string;
      label: string;
      org: string | null;
      items: DashboardPullRequest[];
      latestUpdatedAt: number;
      failingCount: number;
    }>();

    for (const pr of pullRequests.value) {
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
   * Pull the list for one view. Used for both the active view (results land
   * in `pullRequests`) and for the inactive views' counts (only the length
   * is kept).
   */
  async function fetchView(target: DashboardView): Promise<DashboardPullRequest[]> {
    return await invoke<DashboardPullRequest[]>("list_dashboard_pull_requests", {
      view: target,
      sort: sort.value,
      accountId: accountFilter.value,
    });
  }

  /**
   * Fetches every view in parallel. The active view's rows feed
   * `pullRequests`; the lengths feed the sidebar counts. Four SQL reads per
   * load is the price of accurate sidebar counts without back-channelling
   * the counts separately from the Rust side.
   */
  async function load(): Promise<void> {
    loading.value = true;
    lastError.value = null;
    try {
      const [authored, assigned, watching, team] = await Promise.all([
        fetchView("authored"),
        fetchView("assigned"),
        fetchView("watching"),
        fetchView("team"),
      ]);
      viewCounts.value = {
        authored: authored.length,
        assigned: assigned.length,
        watching: watching.length,
        team: team.length,
      };
      pullRequests.value = (() => {
        switch (view.value) {
          case "authored":
            return authored;
          case "assigned":
            return assigned;
          case "watching":
            return watching;
          case "team":
            return team;
        }
      })();
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
    await load();
  }

  function setGroup(next: DashboardGroup): void {
    group.value = next;
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

  async function bind(): Promise<void> {
    if (statusUnlisten !== null) return;
    // Refresh on each completed cycle so the dashboard reflects the latest
    // sync without the user clicking through. The worker emits `synced` once
    // it has finished writing rows for the cycle.
    statusUnlisten = await listen<SyncStatusEvent>(SYNC_STATUS_EVENT, (event) => {
      if (event.payload.phase === "synced") {
        void load();
      }
    });
  }

  function unbind(): void {
    if (statusUnlisten !== null) {
      statusUnlisten();
      statusUnlisten = null;
    }
  }

  function clearError(): void {
    lastError.value = null;
  }

  return {
    view,
    group,
    sort,
    density,
    accountFilter,
    pullRequests,
    loading,
    lastError,
    viewLabel,
    groups,
    counts,
    load,
    setView,
    setGroup,
    setDensity,
    setAccountFilter,
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
