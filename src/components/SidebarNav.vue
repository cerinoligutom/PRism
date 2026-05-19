<script setup lang="ts">
import { RouterLink } from "vue-router";

// PRism brand mark — the logo carries refraction lines in semantic colours.
// Kept inline so the strokes inherit `currentColor` from the surrounding nav.
</script>

<template>
  <aside class="side">
    <div class="side-brand">
      <span class="mark" aria-hidden="true">
        <svg viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round">
          <line x1="2" y1="16" x2="9.5" y2="16" opacity="0.55" />
          <path d="M16 4 L28 26 L4 26 Z" />
          <line x1="20.5" y1="17.5" x2="30" y2="11" stroke="oklch(0.72 0.18 25)" />
          <line x1="21" y1="19" x2="30" y2="16" stroke="oklch(0.78 0.15 80)" />
          <line x1="21.5" y1="20.5" x2="30" y2="21" stroke="oklch(0.74 0.16 145)" />
          <line x1="22" y1="22" x2="29" y2="26" stroke="oklch(0.72 0.14 320)" />
        </svg>
      </span>
      <span class="name"><span>PR</span><span class="ism">ism</span></span>
    </div>

    <h6 class="section-title">Views</h6>
    <nav class="side-nav" aria-label="Primary views">
      <RouterLink to="/" class="nav-item" :class="{ active: $route.name === 'dashboard' }" exact-active-class="">
        <span class="nav-icon">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M2 7l6-4 6 4-6 4z" /><path d="M2 11l6 4 6-4" /></svg>
        </span>
        Authored by me
      </RouterLink>
      <span class="nav-item nav-item--disabled" aria-disabled="true" title="Lands in M2">
        <span class="nav-icon">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="6" r="2.5" /><path d="M3 14c.5-2.5 2.5-4 5-4s4.5 1.5 5 4" /></svg>
        </span>
        Assigned to me
      </span>
      <span class="nav-item nav-item--disabled" aria-disabled="true" title="Lands in M2">
        <span class="nav-icon">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="8" cy="8" r="2.5" /><path d="M1.5 8C3 4.5 5.5 3 8 3s5 1.5 6.5 5C13 11.5 10.5 13 8 13s-5-1.5-6.5-5z" /></svg>
        </span>
        Watching
      </span>
      <span class="nav-item nav-item--disabled" aria-disabled="true" title="Lands in M2">
        <span class="nav-icon">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="5" cy="6" r="2" /><circle cx="11" cy="6" r="2" /><path d="M1 14c.5-2 2-3 4-3s3.5 1 4 3M7 14c.5-2 2-3 4-3s3.5 1 4 3" /></svg>
        </span>
        Team
      </span>
    </nav>

    <div class="side-foot">
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
.side {
  display: flex;
  flex-direction: column;
  padding: 12px 12px 0;
  min-height: 0;
  overflow: hidden;
}

.side-brand {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 6px 6px 14px;
}
.side-brand .mark {
  width: 22px;
  height: 22px;
  color: var(--text-strong);
  flex: 0 0 22px;
}
.side-brand .name {
  font-size: 14px;
  font-weight: 600;
  letter-spacing: -0.4px;
  color: var(--text-strong);
}
.side-brand .name .ism {
  font-weight: 400;
  color: var(--text-mute);
}

.section-title {
  margin: 16px 6px 6px;
}

.side-nav {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.side-foot {
  margin-top: auto;
  border-top: 1px solid var(--border-1);
  padding: 10px 0;
}

/* Disabled nav-items for M2+ surfaces. Visual cue mirrors the design's
 * de-emphasised text without removing the row from the layout — when the
 * destinations land in later milestones they swap to RouterLink. */
.nav-item--disabled {
  color: var(--text-disabled);
  cursor: not-allowed;
}
.nav-item--disabled:hover {
  background: transparent;
  color: var(--text-disabled);
}
.nav-item--disabled .nav-icon {
  color: var(--text-disabled);
}
</style>
