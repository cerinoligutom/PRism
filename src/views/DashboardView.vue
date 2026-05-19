<script setup lang="ts">
import { RouterLink } from "vue-router";
</script>

<template>
  <section class="dashboard">
    <header class="top">
      <h1>Authored by me</h1>
      <span class="count">0 open</span>
    </header>

    <div class="empty">
      <div class="empty-card">
        <span class="empty-mark" aria-hidden="true">
          <svg width="48" height="48" viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" stroke-linecap="round">
            <line x1="2" y1="16" x2="9.5" y2="16" opacity="0.55" />
            <path d="M16 4 L28 26 L4 26 Z" />
            <line x1="20.5" y1="17.5" x2="30" y2="11" stroke="oklch(0.72 0.18 25)" />
            <line x1="21" y1="19" x2="30" y2="16" stroke="oklch(0.78 0.15 80)" />
            <line x1="21.5" y1="20.5" x2="30" y2="21" stroke="oklch(0.74 0.16 145)" />
            <line x1="22" y1="22" x2="29" y2="26" stroke="oklch(0.72 0.14 320)" />
          </svg>
        </span>
        <h2>Connect a GitHub account to get started</h2>
        <p>
          PRism watches the pull requests you care about across every repo and
          org you choose. Your token never leaves your machine.
        </p>
        <RouterLink to="/onboarding" class="btn btn-primary btn-lg">
          Connect GitHub
        </RouterLink>
      </div>
    </div>
  </section>
</template>

<style scoped>
.dashboard {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.top {
  display: flex;
  align-items: baseline;
  gap: 12px;
  padding: 20px 24px 16px;
  border-bottom: 1px solid var(--border-1);
}
.top h1 {
  margin: 0;
  font-size: 22px;
  font-weight: 600;
  letter-spacing: -0.5px;
  color: var(--text-strong);
}
.top .count {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}

.empty {
  flex: 1;
  display: grid;
  place-items: center;
  padding: 48px 24px;
}
.empty-card {
  max-width: 420px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 14px;
}
.empty-mark {
  color: var(--text-strong);
  margin-bottom: 4px;
}
.empty-card h2 {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
  letter-spacing: -0.3px;
  color: var(--text-strong);
}
.empty-card p {
  margin: 0;
  font-size: 13px;
  color: var(--text-mute);
  line-height: 1.55;
}
</style>
