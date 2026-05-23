<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { RouterLink } from "vue-router";

import { useAccountsStore } from "@/stores/accounts";
import { useDashboardStore, type DashboardView } from "@/stores/dashboard";

// PRism brand mark — the logo carries refraction lines in semantic colours.
// Kept inline so the strokes inherit `currentColor` from the surrounding nav.

const dashboard = useDashboardStore();
const accountsStore = useAccountsStore();

interface NavLink {
  readonly view: DashboardView;
  readonly to: string;
  readonly label: string;
}

const links: readonly NavLink[] = [
  { view: "authored", to: "/dashboard/authored", label: "Authored by me" },
  { view: "assigned", to: "/dashboard/assigned", label: "Assigned to me" },
  { view: "watching", to: "/dashboard/watching", label: "Watching" },
  { view: "tracked", to: "/dashboard/tracked", label: "Tracked" },
  { view: "archive", to: "/dashboard/archive", label: "Archive" },
];

interface SidebarAttentionCounts {
  readonly authored: number;
  readonly assigned: number;
  readonly watching: number;
  readonly tracked: number;
}

interface SyncStatusEvent {
  readonly account_id: number;
  readonly phase: string;
}

const SYNC_STATUS_EVENT = "sync://status";

// Per-view attention totals: PRs whose `needs_attention = 1` for the
// account(s) currently in scope. Aggregated across every tracked account when
// no account filter is active so the badge mirrors what the dashboard list
// shows. See `docs/contracts/triage-ux.md` and ADR 0015.
const attention = ref<SidebarAttentionCounts>({
  authored: 0,
  assigned: 0,
  watching: 0,
  tracked: 0,
});

let statusUnlisten: UnlistenFn | null = null;

// ADR 0018: archived rows are excluded from `count_sidebar_attention` server-
// side, so the Archive entry never carries an attention dot. Keeping it out
// of the map and reading via `hasAttention[view] ?? false` in the template
// avoids manufacturing a state the backend doesn't compute.
const hasAttention = computed<Partial<Record<DashboardView, boolean>>>(() => ({
  authored: attention.value.authored > 0,
  assigned: attention.value.assigned > 0,
  watching: attention.value.watching > 0,
  tracked: attention.value.tracked > 0,
}));

async function refreshAttention(): Promise<void> {
  // Mirror the dashboard's `accountScope`. `null` (the union case) sums
  // across every tracked account so the badge stays accurate before the
  // user has narrowed to one. The Rust command is per-account so we fan
  // out and accumulate client-side.
  const scope = dashboard.accountScope;
  const ids =
    scope === null
      ? accountsStore.accounts.map((a) => a.id)
      : [scope];
  if (ids.length === 0) {
    attention.value = { authored: 0, assigned: 0, watching: 0, tracked: 0 };
    return;
  }
  try {
    const results = await Promise.all(
      ids.map((accountId) =>
        invoke<SidebarAttentionCounts>("list_sidebar_attention_counts", {
          accountId,
        }),
      ),
    );
    attention.value = results.reduce<SidebarAttentionCounts>(
      (acc, next) => ({
        authored: acc.authored + next.authored,
        assigned: acc.assigned + next.assigned,
        watching: acc.watching + next.watching,
        tracked: acc.tracked + next.tracked,
      }),
      { authored: 0, assigned: 0, watching: 0, tracked: 0 },
    );
  } catch {
    // Counts are advisory - on failure the badge falls back to the unstyled
    // count chip without taking the panel down.
    attention.value = { authored: 0, assigned: 0, watching: 0, tracked: 0 };
  }
}

watch(() => dashboard.view, () => void refreshAttention());
watch(() => dashboard.accountScope, () => void refreshAttention());
watch(
  () => accountsStore.accounts.length,
  () => void refreshAttention(),
);

onMounted(async () => {
  await refreshAttention();
  statusUnlisten = await listen<SyncStatusEvent>(SYNC_STATUS_EVENT, (event) => {
    if (event.payload.phase === "synced") {
      void refreshAttention();
    }
  });
});

onBeforeUnmount(() => {
  if (statusUnlisten !== null) {
    statusUnlisten();
    statusUnlisten = null;
  }
});
</script>

<template>
  <aside class="sidebar">
    <div class="sidebar__brand">
      <span class="sidebar__brand-mark" aria-hidden="true">
        <svg viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round">
          <line x1="2" y1="16" x2="9.5" y2="16" opacity="0.55" />
          <path d="M16 4 L28 26 L4 26 Z" />
          <line x1="20.5" y1="17.5" x2="30" y2="11" stroke="oklch(0.72 0.18 25)" />
          <line x1="21" y1="19" x2="30" y2="16" stroke="oklch(0.78 0.15 80)" />
          <line x1="21.5" y1="20.5" x2="30" y2="21" stroke="oklch(0.74 0.16 145)" />
          <line x1="22" y1="22" x2="29" y2="26" stroke="oklch(0.72 0.14 320)" />
        </svg>
      </span>
      <span class="sidebar__brand-name"><span>PR</span><span class="sidebar__brand-suffix">ism</span></span>
    </div>

    <h6 class="section-title sidebar__section-heading">Views</h6>
    <nav class="sidebar__nav" aria-label="Primary views">
      <RouterLink
        v-for="link in links"
        :key="link.view"
        :to="link.to"
        class="nav-item"
        active-class="active"
      >
        <span class="nav-icon">
          <template v-if="link.view === 'authored'">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 7l6-4 6 4-6 4z" /><path d="M2 11l6 4 6-4" /></svg>
          </template>
          <template v-else-if="link.view === 'assigned'">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="6" r="2.5" /><path d="M3 14c.5-2.5 2.5-4 5-4s4.5 1.5 5 4" /></svg>
          </template>
          <template v-else-if="link.view === 'watching'">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="8" r="2.5" /><path d="M1.5 8C3 4.5 5.5 3 8 3s5 1.5 6.5 5C13 11.5 10.5 13 8 13s-5-1.5-6.5-5z" /></svg>
          </template>
          <template v-else-if="link.view === 'tracked'">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="5" cy="6" r="2" /><circle cx="11" cy="6" r="2" /><path d="M1 14c.5-2 2-3 4-3s3.5 1 4 3M7 14c.5-2 2-3 4-3s3.5 1 4 3" /></svg>
          </template>
          <template v-else>
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 4h12v3H2z" /><path d="M3 7v6h10V7" /><path d="M6.5 9.5h3" /></svg>
          </template>
        </span>
        {{ link.label }}
        <span
          class="count"
          :class="{ 'has-attention': hasAttention[link.view] ?? false }"
        >{{ dashboard.counts[link.view] }}</span>
      </RouterLink>
    </nav>

    <div class="sidebar__foot">
      <RouterLink to="/settings" class="nav-item" :class="{ active: $route.name?.toString().startsWith('settings') }">
        <span class="nav-icon">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="8" r="2.5" /><path d="M13 8a5 5 0 01-.1 1l1.4 1.1-1 1.7-1.7-.5a5 5 0 01-1.7 1L9.5 14h-2L7 12.3a5 5 0 01-1.7-1l-1.7.5-1-1.7L3 9a5 5 0 01-.1-1 5 5 0 01.1-1L1.6 5.9l1-1.7 1.7.5a5 5 0 011.7-1L6.5 2h2l.5 1.7a5 5 0 011.7 1l1.7-.5 1 1.7L11.9 7c.1.3.1.7.1 1z" /></svg>
        </span>
        Settings
      </RouterLink>
    </div>
  </aside>
</template>

<style scoped>
.sidebar {
  display: flex;
  flex-direction: column;
  padding: var(--s-3) var(--s-3) 0;
  min-height: 0;
  overflow: hidden;
}

.sidebar__brand {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 6px var(--s-4);
}

.sidebar__brand-mark {
  width: 28px;
  height: 28px;
  color: var(--text-strong);
  flex: 0 0 28px;
}

.sidebar__brand-name {
  font-size: var(--fs-20);
  font-weight: 600;
  letter-spacing: -0.01em;
  color: var(--text-strong);
}

.sidebar__brand-suffix {
  font-weight: 400;
  color: var(--text-mute);
}

.sidebar__section-heading {
  margin: var(--s-4) 6px 6px;
}

.sidebar__nav {
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.sidebar__foot {
  margin-top: auto;
  border-top: 1px solid var(--border-1);
  padding: 10px 0;
}
</style>
