<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";

import PRismButton from "@/components/ui/PRismButton.vue";
import PRismInput from "@/components/ui/PRismInput.vue";
import PRismSwitch from "@/components/ui/PRismSwitch.vue";
import { useAccountsStore, type Account } from "@/stores/accounts";
import { useReposStore, type RepoSummary } from "@/stores/repos";

const accountsStore = useAccountsStore();
const reposStore = useReposStore();

const search = ref("");

// Per-org collapsed state, keyed by `${accountId}:${owner}`. A Set in a ref so
// toggling triggers reactivity via reassignment.
const collapsedOrgs = ref<Set<string>>(new Set());

interface ToastState {
  readonly kind: "success" | "info";
  readonly text: string;
}
const toast = ref<ToastState | null>(null);
let toastTimer: number | null = null;
function showToast(text: string, kind: "success" | "info" = "success"): void {
  toast.value = { kind, text };
  if (toastTimer !== null) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.value = null;
    toastTimer = null;
  }, 2200);
}

const totalTeamTracked = computed(() => {
  let count = 0;
  for (const account of accountsStore.accounts) {
    for (const repo of reposStore.getRepos(account.id)) {
      if (repo.is_team_tracked) count += 1;
    }
  }
  return count;
});

const sublabel = computed(() => {
  const n = totalTeamTracked.value;
  return `${n} TEAM REPO${n === 1 ? "" : "S"}`;
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
  readonly repos: readonly RepoSummary[];
  readonly trackedCount: number;
}

type OrgTrackState = "all" | "none" | "mixed";

function groupForAccount(accountId: number): OrgGroup[] {
  const all = reposStore.getRepos(accountId);
  const q = search.value.trim().toLowerCase();
  const filtered = q.length === 0
    ? all
    : all.filter(
        (r) =>
          r.owner.toLowerCase().includes(q) ||
          r.name.toLowerCase().includes(q),
      );
  const byOwner = new Map<string, RepoSummary[]>();
  for (const repo of filtered) {
    const list = byOwner.get(repo.owner);
    if (list === undefined) byOwner.set(repo.owner, [repo]);
    else list.push(repo);
  }
  return Array.from(byOwner.entries())
    .map<OrgGroup>(([owner, repos]) => {
      const sorted = repos.slice().sort((a, b) => a.name.localeCompare(b.name));
      const trackedCount = sorted.reduce(
        (n, r) => n + (r.is_team_tracked ? 1 : 0),
        0,
      );
      return { owner, repos: sorted, trackedCount };
    })
    .sort((a, b) => a.owner.localeCompare(b.owner));
}

function orgTrackState(group: OrgGroup): OrgTrackState {
  if (group.trackedCount === 0) return "none";
  if (group.trackedCount === group.repos.length) return "all";
  return "mixed";
}

function isCollapsed(accountId: number, owner: string): boolean {
  return collapsedOrgs.value.has(`${accountId}:${owner}`);
}

function toggleCollapsed(accountId: number, owner: string): void {
  const key = `${accountId}:${owner}`;
  const next = new Set(collapsedOrgs.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  collapsedOrgs.value = next;
}

// Auto-expand all orgs while a search is active so matches aren't hidden
// behind a collapsed header.
watch(search, (q) => {
  if (q.trim().length > 0 && collapsedOrgs.value.size > 0) {
    collapsedOrgs.value = new Set();
  }
});

async function toggleTeamTracked(repo: RepoSummary): Promise<void> {
  const next = !repo.is_team_tracked;
  try {
    await reposStore.setTeamTracked(repo.id, next);
    showToast(
      next
        ? `Tracking ${repo.owner}/${repo.name}`
        : `Untracked ${repo.owner}/${repo.name}`,
    );
  } catch {
    // Store routed the error into `lastError`; the panel surfaces that
    // below. Skip the toast so the failure isn't masked as success.
  }
}

async function toggleOrgTracked(group: OrgGroup): Promise<void> {
  // Mixed and none both go ON (positive default for the org gesture);
  // all goes OFF.
  const target = orgTrackState(group) !== "all";
  const work = group.repos.filter((r) => r.is_team_tracked !== target);
  if (work.length === 0) return;
  try {
    await Promise.all(work.map((r) => reposStore.setTeamTracked(r.id, target)));
    showToast(
      target
        ? `Tracking ${work.length} repo${work.length === 1 ? "" : "s"} under ${group.owner}`
        : `Untracked ${work.length} repo${work.length === 1 ? "" : "s"} under ${group.owner}`,
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
      Pick which repositories appear in the <strong>Team</strong> view. PRism only fetches Team-view
      data for repos you opt in. Discovery uses GitHub's
      <code class="repositories-panel__code">/user/repos</code> endpoint.
    </p>

    <Transition name="repo-toast">
      <div
        v-if="toast"
        class="repositories-panel__toast"
        :class="`repositories-panel__toast--${toast.kind}`"
        role="status"
        aria-live="polite"
      >
        {{ toast.text }}
      </div>
    </Transition>

    <div v-if="accountsStore.isEmpty" class="repositories-panel__empty">
      <p class="repositories-panel__empty-copy">Connect a GitHub account to discover repositories.</p>
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
        <p class="repo-account__empty-copy">No repositories discovered yet.</p>
        <PRismButton
          size="sm"
          variant="primary"
          :disabled="reposStore.isRefreshing(account.id)"
          @click="refreshAccount(account.id)"
        >
          Discover repositories
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
            :class="{ 'org-group__head--all': orgTrackState(group) === 'all' }"
          >
            <button
              type="button"
              class="org-group__toggle-btn"
              :aria-expanded="!isCollapsed(account.id, group.owner)"
              @click="toggleCollapsed(account.id, group.owner)"
            >
              <svg
                class="org-group__chevron"
                :class="{ 'org-group__chevron--open': !isCollapsed(account.id, group.owner) }"
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
                {{ group.trackedCount }} / {{ group.repos.length }}
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
                :aria-label="`Toggle Team-tracked for every ${group.owner} repo`"
                @update:model-value="toggleOrgTracked(group)"
              />
            </div>
          </header>

          <ul
            v-if="!isCollapsed(account.id, group.owner)"
            class="org-group__list"
          >
            <li
              v-for="repo in group.repos"
              :key="repo.id"
              class="repo-row"
              :class="{ 'repo-row--tracked': repo.is_team_tracked }"
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
                :model-value="repo.is_team_tracked"
                :aria-label="`Toggle Team-tracked for ${repo.owner}/${repo.name}`"
                @update:model-value="toggleTeamTracked(repo)"
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

.repositories-panel__toast {
  margin: calc(-1 * var(--s-3)) 0 var(--s-4);
  padding: 8px 14px;
  border-radius: var(--r-2);
  background: var(--accent-bg);
  color: var(--accent-strong);
  font-size: var(--fs-12);
  font-weight: 500;
  border: 1px solid color-mix(in oklch, var(--accent) 30%, transparent);
}

.repo-toast-enter-active,
.repo-toast-leave-active {
  transition: opacity 0.2s, transform 0.2s;
}

.repo-toast-enter-from,
.repo-toast-leave-to {
  opacity: 0;
  transform: translateY(-4px);
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
  margin-bottom: var(--s-4);
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

.org-group__head--all .org-group__count {
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
