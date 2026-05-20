<script setup lang="ts">
import { computed, onMounted, watch } from "vue";

import PRismButton from "@/components/ui/PRismButton.vue";
import { useAccountsStore, type Account } from "@/stores/accounts";
import { useReposStore, type RepoSummary } from "@/stores/repos";

const accountsStore = useAccountsStore();
const reposStore = useReposStore();

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
  // Mirror AccountsPanel's avatar palette so the same account reads the same
  // colour across settings panels.
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

async function toggleTeamTracked(repo: RepoSummary): Promise<void> {
  try {
    await reposStore.setTeamTracked(repo.id, !repo.is_team_tracked);
  } catch {
    // Error already routed to lastError by the store.
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

// If the user adds or removes accounts elsewhere, refresh the panel so the
// per-account sections stay in sync.
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

      <ul v-else class="repo-account__list">
        <li
          v-for="repo in reposStore.getRepos(account.id)"
          :key="repo.id"
          class="repo-row"
          :class="{ 'repo-row--tracked': repo.is_team_tracked }"
        >
          <div class="repo-row__info">
            <span class="repo-chip">
              <span class="org">{{ repo.owner }}</span>
              <span class="slash">/</span>
              <span class="repo">{{ repo.name }}</span>
            </span>
            <span class="repo-row__visibility badge">{{ repo.visibility }}</span>
          </div>
          <button
            type="button"
            class="toggle"
            :class="{ 'toggle--on': repo.is_team_tracked }"
            role="switch"
            :aria-checked="repo.is_team_tracked"
            :aria-label="`Toggle Team-tracked for ${repo.owner}/${repo.name}`"
            @click="toggleTeamTracked(repo)"
          >
            <span class="toggle__thumb" aria-hidden="true" />
            <span class="toggle__label">
              {{ repo.is_team_tracked ? "Team-tracked" : "Not tracked" }}
            </span>
          </button>
        </li>
      </ul>
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

.repo-account__list {
  margin: 0;
  padding: 0;
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 1px;
  background: var(--border-1);
  border-radius: var(--r-3);
  overflow: hidden;
}

/* ────── repo-row BEM block ────── */
.repo-row {
  background: var(--bg-2);
  padding: 12px 16px;
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

.repo-row__visibility {
  text-transform: lowercase;
}

/* ────── toggle BEM block ────── */
.toggle {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 4px 10px 4px 4px;
  background: transparent;
  border: 1px solid var(--border-2);
  border-radius: var(--r-pill);
  color: var(--text-mute);
  font-size: var(--fs-11);
  font-weight: 500;
  cursor: pointer;
  transition: background 0.12s, border-color 0.12s, color 0.12s;
}

.toggle:hover {
  border-color: var(--border-3);
  color: var(--text);
}

.toggle:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.toggle--on {
  background: var(--accent-bg);
  border-color: transparent;
  color: var(--accent-strong);
}

.toggle__thumb {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--bg-4);
  display: inline-block;
  transition: background 0.12s;
}

.toggle--on .toggle__thumb {
  background: var(--accent);
}

.toggle__label {
  font-family: var(--font-mono);
  letter-spacing: 0.3px;
}
</style>
