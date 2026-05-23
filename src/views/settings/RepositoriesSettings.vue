<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";

import PRismButton from "@/components/ui/PRismButton.vue";
import PRismInput from "@/components/ui/PRismInput.vue";
import PRismSwitch from "@/components/ui/PRismSwitch.vue";
import { useAccountsStore, type Account } from "@/stores/accounts";
import { useReposStore, type RepoSummary } from "@/stores/repos";
import { useToastStore } from "@/stores/toast";

const accountsStore = useAccountsStore();
const reposStore = useReposStore();
const toastStore = useToastStore();

const search = ref("");
const showTrackedOnly = ref(false);

// Per-org expanded state, keyed by `${accountId}:${owner}`. Default is empty,
// meaning every org starts collapsed - so a 200-repo account stays scannable
// at a glance. Expanding is opt-in via the chevron, search, or the
// Tracked-only filter (both temporarily expand-all so matches surface).
const expandedOrgs = ref<Set<string>>(new Set());

// Snapshot of the user's manual expand state taken when search / filter
// flips ON; restored when both go OFF so the user's deliberate state isn't
// wiped by a transient search.
const expandedOrgsSnapshot = ref<Set<string> | null>(null);

const totalTracked = computed(() => {
  let count = 0;
  for (const account of accountsStore.accounts) {
    for (const repo of reposStore.getRepos(account.id)) {
      if (repo.is_tracked) count += 1;
    }
  }
  return count;
});

const sublabel = computed(() => {
  const n = totalTracked.value;
  return `${n} TRACKED REPO${n === 1 ? "" : "S"}`;
});

function avatarClass(accountId: number): string {
  const slot = ((accountId - 1) % 9) + 1;
  return `avatar repo-account__avatar av-${slot}`;
}

function initials(account: Account): string {
  const source = account.label || account.login;
  return (
    source
      .split(/[\s\-_/]+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((part) => part[0]?.toUpperCase() ?? "")
      .join("") || "?"
  );
}

interface OrgGroup {
  readonly owner: string;
  /** Repos visible after search / Tracked-only filter (what we render). */
  readonly repos: readonly RepoSummary[];
  /** Every repo in this org, regardless of filter. Drives the counter and
   * the "All / Mixed / None" track state so they reflect reality, not the
   * filtered slice. */
  readonly allRepos: readonly RepoSummary[];
  readonly trackedCount: number;
  readonly totalCount: number;
}

type OrgTrackState = "all" | "none" | "mixed";

function groupForAccount(accountId: number): OrgGroup[] {
  const all = reposStore.getRepos(accountId);
  // First bucket the FULL set by owner so we know each org's real size,
  // independent of the visible filter applied below.
  const allByOwner = new Map<string, RepoSummary[]>();
  for (const repo of all) {
    const list = allByOwner.get(repo.owner);
    if (list === undefined) allByOwner.set(repo.owner, [repo]);
    else list.push(repo);
  }
  const q = search.value.trim().toLowerCase();
  return Array.from(allByOwner.entries())
    .map<OrgGroup>(([owner, ownerRepos]) => {
      const visible = ownerRepos.filter((r) => {
        if (showTrackedOnly.value && !r.is_tracked) return false;
        if (q.length > 0
            && !r.owner.toLowerCase().includes(q)
            && !r.name.toLowerCase().includes(q)) return false;
        return true;
      });
      const sortedVisible = visible.slice().sort((a, b) => a.name.localeCompare(b.name));
      const trackedCount = ownerRepos.reduce(
        (n, r) => n + (r.is_tracked ? 1 : 0),
        0,
      );
      return {
        owner,
        repos: sortedVisible,
        allRepos: ownerRepos,
        trackedCount,
        totalCount: ownerRepos.length,
      };
    })
    .filter((g) => g.repos.length > 0)
    .sort((a, b) => a.owner.localeCompare(b.owner));
}

function orgTrackState(group: OrgGroup): OrgTrackState {
  if (group.trackedCount === 0) return "none";
  if (group.trackedCount === group.totalCount) return "all";
  return "mixed";
}

function isExpanded(accountId: number, owner: string): boolean {
  return expandedOrgs.value.has(`${accountId}:${owner}`);
}

function toggleExpanded(accountId: number, owner: string): void {
  const key = `${accountId}:${owner}`;
  const next = new Set(expandedOrgs.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  expandedOrgs.value = next;
}

function allOrgKeys(): Set<string> {
  const out = new Set<string>();
  for (const account of accountsStore.accounts) {
    for (const repo of reposStore.getRepos(account.id)) {
      out.add(`${account.id}:${repo.owner}`);
    }
  }
  return out;
}

// While search OR the Tracked-only filter is active, expand every org so
// matches surface immediately. Snapshot the user's manual state on the way
// in, restore it on the way out so a transient search doesn't wipe their
// deliberate expansions.
const isAutoExpanding = computed<boolean>(
  () => search.value.trim().length > 0 || showTrackedOnly.value,
);

watch(isAutoExpanding, (active) => {
  if (active && expandedOrgsSnapshot.value === null) {
    expandedOrgsSnapshot.value = new Set(expandedOrgs.value);
    expandedOrgs.value = allOrgKeys();
  } else if (!active && expandedOrgsSnapshot.value !== null) {
    expandedOrgs.value = expandedOrgsSnapshot.value;
    expandedOrgsSnapshot.value = null;
  }
});

async function toggleTracked(repo: RepoSummary): Promise<void> {
  const next = !repo.is_tracked;
  try {
    await reposStore.setTracked(repo.id, next);
    toastStore.show(
      next
        ? `Tracking ${repo.owner}/${repo.name}`
        : `Untracked ${repo.owner}/${repo.name}`,
      { variant: "success" },
    );
  } catch {
    // Store routed the error into `lastError`; the panel surfaces that
    // below. Skip the toast so the failure isn't masked as success.
  }
}

async function toggleOrgTracked(group: OrgGroup): Promise<void> {
  // Mixed and none both go ON (positive default for the org gesture);
  // all goes OFF. Walk the UNFILTERED org so the gesture applies to every
  // repo, not just the slice visible under the current filter.
  const target = orgTrackState(group) !== "all";
  const work = group.allRepos.filter((r) => r.is_tracked !== target);
  if (work.length === 0) return;
  try {
    await Promise.all(work.map((r) => reposStore.setTracked(r.id, target)));
    toastStore.show(
      target
        ? `Tracking ${work.length} repo${work.length === 1 ? "" : "s"} under ${group.owner}`
        : `Untracked ${work.length} repo${work.length === 1 ? "" : "s"} under ${group.owner}`,
      { variant: "success" },
    );
  } catch {
    // Partial failure - lastError already populated by the store.
  }
}

async function refreshAccount(accountId: number): Promise<void> {
  await reposStore.refresh(accountId);
}

async function loadAccounts(): Promise<void> {
  await accountsStore.refresh();
  await Promise.all(accountsStore.accounts.map((a) => reposStore.load(a.id)));
}

onMounted(loadAccounts);

watch(
  () => accountsStore.accounts.map((a) => a.id).join(","),
  async (next, prev) => {
    if (next === prev) return;
    await Promise.all(
      accountsStore.accounts
        .filter((a) => reposStore.getRepos(a.id).length === 0)
        .map((a) => reposStore.load(a.id)),
    );
  },
);
</script>

<template>
  <div class="repositories-panel">
    <header class="repositories-panel__header">
      <h1 class="repositories-panel__title">Repositories</h1>
      <span class="repositories-panel__sub">{{ sublabel }}</span>
    </header>

    <p class="repositories-panel__intro">
      Pick which repositories appear in your <strong>Tracked</strong> view.
      PRism only watches the repos you opt in, so your connected accounts
      stay focused on what you actually care about.
    </p>

    <div v-if="accountsStore.isEmpty" class="repositories-panel__empty">
      <p class="repositories-panel__empty-copy">Connect a GitHub account to see your repositories.</p>
      <PRismButton to="/onboarding" variant="primary">Connect an account</PRismButton>
    </div>

    <section
      v-for="account in accountsStore.accounts"
      :key="account.id"
      class="repo-account"
    >
      <header class="repo-account__head">
        <span :class="avatarClass(account.id)">{{ initials(account) }}</span>
        <div class="repo-account__info">
          <div class="repo-account__label">
            {{ account.label || account.login }}
            <span class="repo-account__host">{{ account.host }}</span>
          </div>
          <div class="repo-account__sub">
            <span class="repo-account__login">{{ account.login }}</span>
            <span class="repo-account__sep">·</span>
            <span>{{ reposStore.getRepos(account.id).length }} repo<template v-if="reposStore.getRepos(account.id).length !== 1">s</template></span>
          </div>
        </div>
        <PRismButton
          size="sm"
          :disabled="reposStore.isRefreshing(account.id)"
          @click="refreshAccount(account.id)"
        >
          <template v-if="reposStore.isRefreshing(account.id)">Refreshing…</template>
          <template v-else>Refresh from GitHub</template>
        </PRismButton>
      </header>

      <div
        v-if="reposStore.getRepos(account.id).length > 0"
        class="repo-account__search"
      >
        <PRismInput
          v-model="search"
          placeholder="Search repos by name or owner..."
          :spellcheck="false"
          autocomplete="off"
        />
        <button
          type="button"
          class="repo-account__filter-btn"
          :class="{ 'repo-account__filter-btn--active': showTrackedOnly }"
          :aria-pressed="showTrackedOnly"
          @click="showTrackedOnly = !showTrackedOnly"
        >
          Tracked only
        </button>
      </div>

      <div
        v-if="reposStore.isLoading(account.id) && reposStore.getRepos(account.id).length === 0"
        class="repo-account__loading"
      >
        Loading repositories…
      </div>

      <div
        v-else-if="reposStore.getRepos(account.id).length === 0"
        class="repo-account__empty"
      >
        <p class="repo-account__empty-copy">We haven't loaded any repositories yet.</p>
        <PRismButton
          size="sm"
          variant="primary"
          :disabled="reposStore.isRefreshing(account.id)"
          @click="refreshAccount(account.id)"
        >
          Load repositories
        </PRismButton>
      </div>

      <div
        v-else-if="groupForAccount(account.id).length === 0"
        class="repo-account__empty"
      >
        <p class="repo-account__empty-copy">
          No repositories match "<strong>{{ search }}</strong>".
        </p>
      </div>

      <div v-else class="repo-account__orgs">
        <div
          v-for="group in groupForAccount(account.id)"
          :key="`${account.id}:${group.owner}`"
          class="org-group"
        >
          <header
            class="org-group__head"
            :class="{
              'org-group__head--all': orgTrackState(group) === 'all',
              'org-group__head--has-tracked': group.trackedCount > 0,
            }"
          >
            <button
              type="button"
              class="org-group__toggle-btn"
              :aria-expanded="isExpanded(account.id, group.owner)"
              @click="toggleExpanded(account.id, group.owner)"
            >
              <svg
                class="org-group__chevron"
                :class="{ 'org-group__chevron--open': isExpanded(account.id, group.owner) }"
                width="10"
                height="10"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d="M5 4l5 4-5 4" />
              </svg>
              <span class="org-group__name">{{ group.owner }}</span>
              <span class="org-group__count">
                {{ group.trackedCount }} / {{ group.totalCount }}
              </span>
            </button>
            <div class="org-group__action">
              <span class="org-group__action-label">
                <template v-if="orgTrackState(group) === 'all'">All tracked</template>
                <template v-else-if="orgTrackState(group) === 'mixed'">
                  Track all
                </template>
                <template v-else>Track all</template>
              </span>
              <PRismSwitch
                :model-value="orgTrackState(group) === 'all'"
                :aria-label="`Toggle Tracked for every ${group.owner} repo`"
                @update:model-value="toggleOrgTracked(group)"
              />
            </div>
          </header>

          <ul
            v-if="isExpanded(account.id, group.owner)"
            class="org-group__list"
          >
            <li
              v-for="repo in group.repos"
              :key="repo.id"
              class="repo-row"
              :class="{ 'repo-row--tracked': repo.is_tracked }"
            >
              <div class="repo-row__info">
                <svg
                  v-if="repo.visibility === 'private'"
                  class="repo-row__lock"
                  width="12"
                  height="12"
                  viewBox="0 0 16 16"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.6"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  aria-label="Private repository"
                >
                  <rect x="3.5" y="7" width="9" height="6" rx="1" />
                  <path d="M5.5 7V5a2.5 2.5 0 015 0v2" />
                </svg>
                <span v-else class="repo-row__lock-placeholder" aria-hidden="true" />
                <span class="repo-row__name">{{ repo.name }}</span>
              </div>
              <PRismSwitch
                :model-value="repo.is_tracked"
                :aria-label="`Toggle Tracked for ${repo.owner}/${repo.name}`"
                @update:model-value="toggleTracked(repo)"
              />
            </li>
          </ul>
        </div>
      </div>
    </section>

    <div v-if="reposStore.lastError" class="repositories-panel__error">
      {{ reposStore.lastError }}
    </div>
  </div>
</template>

<style scoped>
.repositories-panel__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  margin-bottom: var(--s-3);
}

.repositories-panel__title {
  margin: 0;
  font-size: var(--fs-24);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}

.repositories-panel__sub {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  letter-spacing: 0.5px;
}

.repositories-panel__intro {
  margin: 0 0 var(--s-6);
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
}

.repositories-panel__code {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  padding: 1px 5px;
  background: var(--bg-3);
  border-radius: var(--r-1);
  color: var(--text);
}

.repositories-panel__empty {
  padding: var(--s-7) var(--s-6);
  background: var(--bg-2);
  border: 1px dashed var(--border-2);
  border-radius: var(--r-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  align-items: center;
}

.repositories-panel__empty-copy {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
}

.repositories-panel__error {
  margin-top: var(--s-4);
  padding: 10px 14px;
  border-radius: var(--r-2);
  background: var(--danger-bg);
  color: var(--danger);
  font-size: var(--fs-12);
}

/* ────── repo-account BEM block ────── */
.repo-account {
  margin-bottom: var(--s-7);
}

.repo-account__head {
  display: grid;
  grid-template-columns: 36px 1fr auto;
  gap: var(--s-4);
  align-items: center;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: var(--s-4);
}

.repo-account__avatar {
  width: 36px;
  height: 36px;
  font-size: var(--fs-12);
  border-radius: 8px;
}

.repo-account__label {
  font-size: var(--fs-13);
  font-weight: 600;
  color: var(--text-strong);
  display: flex;
  align-items: center;
  gap: 6px;
}

.repo-account__host {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  font-weight: 400;
  padding: 1px 5px;
  background: var(--bg-3);
  border-radius: var(--r-1);
  letter-spacing: 0.3px;
}

.repo-account__sub {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-mute);
  margin-top: 3px;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 2px;
}

.repo-account__login {
  color: var(--text-mute);
}

.repo-account__sep {
  color: var(--text-disabled);
  margin: 0 4px;
}

.repo-account__loading {
  padding: var(--s-5);
  color: var(--text-mute);
  font-size: var(--fs-12);
}

.repo-account__empty {
  padding: var(--s-6);
  background: var(--bg-2);
  border: 1px dashed var(--border-2);
  border-radius: var(--r-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  align-items: center;
}

.repo-account__empty-copy {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.repo-account__search {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  margin-bottom: var(--s-4);
}

.repo-account__search > :first-child {
  flex: 1 1 auto;
  min-width: 0;
}

.repo-account__filter-btn {
  flex: 0 0 auto;
  height: 30px;
  padding: 0 12px;
  border-radius: var(--r-2);
  border: 1px solid var(--border-2);
  background: var(--bg-2);
  color: var(--text-mute);
  font-size: var(--fs-12);
  font-weight: 500;
  cursor: pointer;
  transition: background 0.12s, color 0.12s, border-color 0.12s;
}

.repo-account__filter-btn:hover {
  background: var(--bg-3);
  color: var(--text);
}

.repo-account__filter-btn--active {
  background: var(--accent-bg);
  color: var(--accent-strong);
  border-color: transparent;
}

.repo-account__filter-btn--active:hover {
  background: var(--accent-bg);
  color: var(--accent-strong);
}

.repo-account__orgs {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

/* ────── org-group BEM block ────── */
.org-group {
  border: 1px solid var(--border-1);
  border-radius: var(--r-3);
  background: var(--bg-2);
  overflow: hidden;
}

.org-group__head {
  display: grid;
  grid-template-columns: 1fr auto;
  align-items: center;
  gap: var(--s-3);
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-1);
}

.org-group__head--all {
  background: var(--bg-3);
}

.org-group__toggle-btn {
  display: inline-flex;
  align-items: center;
  gap: var(--s-3);
  background: transparent;
  border: 0;
  padding: 0;
  cursor: pointer;
  color: inherit;
  font: inherit;
  text-align: left;
}

.org-group__toggle-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
  border-radius: var(--r-1);
}

.org-group__chevron {
  color: var(--text-mute);
  transition: transform 0.12s;
  flex: 0 0 10px;
}

.org-group__chevron--open {
  transform: rotate(90deg);
}

.org-group__name {
  font-size: var(--fs-13);
  font-weight: 600;
  color: var(--text-strong);
}

.org-group__count {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  letter-spacing: 0.3px;
  padding: 1px 6px;
  background: var(--bg-3);
  border: 1px solid var(--border-1);
  border-radius: var(--r-1);
}

.org-group__head--has-tracked .org-group__count {
  color: var(--accent-strong);
  background: var(--accent-bg);
  border-color: transparent;
}

.org-group__action {
  display: inline-flex;
  align-items: center;
  gap: var(--s-3);
}

.org-group__action-label {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-mute);
  letter-spacing: 0.3px;
}

.org-group__list {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 1px;
  background: var(--border-1);
}

/* ────── repo-row BEM block ────── */
.repo-row {
  background: var(--bg-2);
  padding: 10px 14px 10px 18px;
  display: grid;
  grid-template-columns: 1fr auto;
  gap: var(--s-4);
  align-items: center;
}

.repo-row--tracked {
  background: var(--bg-3);
}

.repo-row__info {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  min-width: 0;
}

.repo-row__lock {
  color: var(--text-mute);
  flex: 0 0 12px;
}

.repo-row__lock-placeholder {
  width: 12px;
  height: 12px;
  flex: 0 0 12px;
}

.repo-row__name {
  font-family: var(--font-mono);
  font-size: var(--fs-12);
  color: var(--text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
