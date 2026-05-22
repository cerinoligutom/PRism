<script setup lang="ts">
import { RouterLink, RouterView } from "vue-router";
</script>

<template>
  <section class="settings">
    <aside class="settings__side">
      <h6 class="section-title settings__heading">Settings</h6>
      <nav class="settings__nav">
        <RouterLink to="/settings/appearance" class="nav-item" :class="{ active: $route.name === 'settings.appearance' }">
          <span class="nav-icon">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><circle cx="8" cy="8" r="5" /><path d="M8 3a5 5 0 010 10" fill="currentColor" /></svg>
          </span>
          Appearance
        </RouterLink>
        <RouterLink to="/settings/notifications" class="nav-item" :class="{ active: $route.name === 'settings.notifications' }">
          <span class="nav-icon">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6a4 4 0 018 0v3l1 2H3l1-2V6z" /><path d="M6.5 13a1.5 1.5 0 003 0" /></svg>
          </span>
          Notifications
        </RouterLink>
        <RouterLink to="/settings/sync" class="nav-item" :class="{ active: $route.name === 'settings.sync' }">
          <span class="nav-icon">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M13 4.5a5.5 5.5 0 00-9.7 1.7" /><path d="M3 8.5V4.5h4" /><path d="M3 11.5a5.5 5.5 0 009.7-1.7" /><path d="M13 7.5v4h-4" /></svg>
          </span>
          Sync
        </RouterLink>
        <RouterLink to="/settings/accounts" class="nav-item" :class="{ active: $route.name === 'settings.accounts' }">
          <span class="nav-icon">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><circle cx="8" cy="6" r="2.5" /><path d="M3 14c.5-2.5 2.5-4 5-4s4.5 1.5 5 4" /></svg>
          </span>
          Accounts
        </RouterLink>
        <RouterLink to="/settings/repositories" class="nav-item" :class="{ active: $route.name === 'settings.repositories' }">
          <span class="nav-icon">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3 2.5h7l3 3v8H3z" /><path d="M3 11h10" /><path d="M6 2.5v3h4v-3" /></svg>
          </span>
          Repositories
        </RouterLink>
      </nav>
    </aside>

    <section class="settings__main scroll">
      <RouterView />
    </section>
  </section>
</template>

<style scoped>
.settings {
  display: grid;
  grid-template-columns: 220px 1fr;
  height: 100%;
  min-height: 0;
  background: var(--bg-1);
}

.settings__side {
  border-right: 1px solid var(--border-1);
  padding: var(--s-4) var(--s-3) var(--s-3);
  display: flex;
  flex-direction: column;
  background: var(--bg-2);
}

.settings__heading {
  margin: var(--s-4) 6px 6px;
}

.settings__nav {
  display: flex;
  flex-direction: column;
  gap: 1px;
  margin-top: var(--s-1);
}

.settings__main {
  padding: var(--s-7) var(--s-8);
  overflow-y: auto;
  min-width: 0;
}

/* Cap the inner content width and centre it so the panels don't stretch
 * across an ultra-wide window. Matches the comfortable line lengths used
 * inside `.set-row` and the appearance / repositories headers. */
.settings__main > * {
  max-width: 720px;
  margin-inline: auto;
}
</style>
