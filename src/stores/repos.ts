import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

export interface RepoSummary {
  readonly id: number;
  readonly account_id: number;
  readonly owner: string;
  readonly name: string;
  readonly visibility: string;
  readonly is_tracked: boolean;
}

type ReposCommandError =
  | { kind: "account_not_found" }
  | { kind: "repo_not_found" }
  | { kind: "unauthorized" }
  | { kind: "rate_limited" }
  | { kind: "network"; host: string }
  | { kind: "internal" };

/**
 * Translates the structured Rust error into a single user-facing message.
 * The shape comes from `#[serde(tag = "kind", rename_all = "snake_case")]`
 * on `ReposCommandError` in `src-tauri/src/repos/commands.rs`.
 */
function formatReposError(raw: unknown): string {
  if (typeof raw === "object" && raw !== null && "kind" in raw) {
    const err = raw as ReposCommandError;
    switch (err.kind) {
      case "account_not_found":
        return "Account not found.";
      case "repo_not_found":
        return "Repository not found.";
      case "unauthorized":
        return "GitHub rejected the token. Re-authenticate the account.";
      case "rate_limited":
        return "GitHub rate-limited the request. Try again later.";
      case "network":
        return `Couldn't reach ${err.host}. Check your connection.`;
      case "internal":
        return "Something went wrong. Check the application logs.";
    }
  }
  return typeof raw === "string" ? raw : "Unexpected error.";
}

export const useReposStore = defineStore("repos", () => {
  // Per-account repo lists, keyed by account id. Keeping them keyed lets the
  // panel render multiple accounts side-by-side without re-fetching.
  const byAccount = ref<Record<number, RepoSummary[]>>({});
  const loadingAccountIds = ref<Set<number>>(new Set());
  const refreshingAccountIds = ref<Set<number>>(new Set());
  const lastError = ref<string | null>(null);

  const isLoadingAny = computed(() => loadingAccountIds.value.size > 0);
  const isRefreshingAny = computed(() => refreshingAccountIds.value.size > 0);

  /**
   * Total tracked repos across every account whose list has been loaded.
   * Reflects the in-memory store, so callers must ensure `load(accountId)`
   * has been invoked for each account they care about before reading it.
   */
  const totalTrackedCount = computed<number>(() => {
    let count = 0;
    for (const repos of Object.values(byAccount.value)) {
      for (const repo of repos) {
        if (repo.is_tracked) count += 1;
      }
    }
    return count;
  });

  function getRepos(accountId: number): readonly RepoSummary[] {
    return byAccount.value[accountId] ?? [];
  }

  function isLoading(accountId: number): boolean {
    return loadingAccountIds.value.has(accountId);
  }

  function isRefreshing(accountId: number): boolean {
    return refreshingAccountIds.value.has(accountId);
  }

  async function load(accountId: number): Promise<void> {
    loadingAccountIds.value = new Set(loadingAccountIds.value).add(accountId);
    lastError.value = null;
    try {
      const repos = await invoke<RepoSummary[]>("list_repos_for_account", {
        accountId,
      });
      byAccount.value = { ...byAccount.value, [accountId]: repos };
    } catch (err) {
      lastError.value = formatReposError(err);
    } finally {
      const next = new Set(loadingAccountIds.value);
      next.delete(accountId);
      loadingAccountIds.value = next;
    }
  }

  async function refresh(accountId: number): Promise<void> {
    refreshingAccountIds.value = new Set(refreshingAccountIds.value).add(accountId);
    lastError.value = null;
    try {
      const repos = await invoke<RepoSummary[]>("refresh_account_repos", {
        accountId,
      });
      byAccount.value = { ...byAccount.value, [accountId]: repos };
    } catch (err) {
      lastError.value = formatReposError(err);
    } finally {
      const next = new Set(refreshingAccountIds.value);
      next.delete(accountId);
      refreshingAccountIds.value = next;
    }
  }

  async function setTracked(repoId: number, tracked: boolean): Promise<void> {
    lastError.value = null;
    // Optimistic update so the toggle feels instant; revert on failure.
    const previous = byAccount.value;
    const next: Record<number, RepoSummary[]> = {};
    for (const [accountIdStr, repos] of Object.entries(previous)) {
      const accountId = Number(accountIdStr);
      next[accountId] = repos.map((repo) =>
        repo.id === repoId ? { ...repo, is_tracked: tracked } : repo,
      );
    }
    byAccount.value = next;

    try {
      await invoke<void>("set_repo_tracked", { repoId, tracked });
    } catch (err) {
      // Roll the optimistic update back so the UI reflects truth.
      byAccount.value = previous;
      lastError.value = formatReposError(err);
      throw new Error(lastError.value);
    }
  }

  function clearError(): void {
    lastError.value = null;
  }

  /**
   * Drop the cached repo list for `accountId`. Called from the accounts
   * store's `removeAccount` action so the per-account slice in `byAccount`
   * doesn't dangle after the account row (and its FK-cascaded repos) is
   * gone from SQL.
   */
  function forgetAccount(accountId: number): void {
    if (!(accountId in byAccount.value)) return;
    const next = { ...byAccount.value };
    delete next[accountId];
    byAccount.value = next;
  }

  return {
    byAccount,
    lastError,
    isLoadingAny,
    isRefreshingAny,
    totalTrackedCount,
    getRepos,
    isLoading,
    isRefreshing,
    load,
    refresh,
    setTracked,
    clearError,
    forgetAccount,
  };
});
