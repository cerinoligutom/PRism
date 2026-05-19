<script setup lang="ts">
import { RouterLink, RouterView } from "vue-router";
</script>

<template>
  <section class="settings">
    <aside class="settings-side">
      <div class="settings-back">
        <RouterLink to="/" class="nav-item">
          <span class="nav-icon">
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M7 4L3 8l4 4M3 8h10" /></svg>
          </span>
          Back to dashboard
        </RouterLink>
      </div>

      <h6 class="section-title">Settings</h6>
      <nav class="settings-nav">
        <RouterLink to="/settings/accounts" class="nav-item" :class="{ active: $route.name === 'settings.accounts' }">
          <span class="nav-icon">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><circle cx="8" cy="6" r="2.5" /><path d="M3 14c.5-2.5 2.5-4 5-4s4.5 1.5 5 4" /></svg>
          </span>
          Accounts
        </RouterLink>
        <span class="nav-item nav-item--disabled" aria-disabled="true" title="Lands in M3/M4">Sync &amp; data</span>
        <span class="nav-item nav-item--disabled" aria-disabled="true" title="Lands in M3">Appearance</span>
        <span class="nav-item nav-item--disabled" aria-disabled="true" title="Lands in M5">Notifications</span>
        <span class="nav-item nav-item--disabled" aria-disabled="true" title="Lands in M6">Views &amp; layout</span>
      </nav>
    </aside>

    <section class="settings-main scroll">
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

.settings-side {
  border-right: 1px solid var(--border-1);
  padding: 16px 12px 12px;
  display: flex;
  flex-direction: column;
  background: var(--bg-2);
}

.settings-back {
  margin-bottom: 8px;
}

.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 1px;
  margin-top: 4px;
}

.settings-main {
  padding: 28px 48px;
  overflow-y: auto;
  min-width: 0;
}

.nav-item--disabled {
  color: var(--text-disabled);
  cursor: not-allowed;
}
.nav-item--disabled:hover {
  background: transparent;
  color: var(--text-disabled);
}
</style>
