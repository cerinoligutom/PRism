<script setup lang="ts">
import PRismButton from "@/components/ui/PRismButton.vue";
</script>

<template>
  <section class="dashboard">
    <header class="dashboard__header">
      <h1 class="dashboard__title">Authored by me</h1>
      <span class="dashboard__count">0 open</span>
    </header>

    <div class="dashboard__empty">
      <div class="dashboard-empty">
        <span class="dashboard-empty__mark" aria-hidden="true">
          <svg width="48" height="48" viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" stroke-linecap="round">
            <line x1="2" y1="16" x2="9.5" y2="16" opacity="0.55" />
            <path d="M16 4 L28 26 L4 26 Z" />
            <line x1="20.5" y1="17.5" x2="30" y2="11" stroke="oklch(0.72 0.18 25)" />
            <line x1="21" y1="19" x2="30" y2="16" stroke="oklch(0.78 0.15 80)" />
            <line x1="21.5" y1="20.5" x2="30" y2="21" stroke="oklch(0.74 0.16 145)" />
            <line x1="22" y1="22" x2="29" y2="26" stroke="oklch(0.72 0.14 320)" />
          </svg>
        </span>
        <h2 class="dashboard-empty__title">Connect a GitHub account to get started</h2>
        <p class="dashboard-empty__copy">
          PRism watches the pull requests you care about across every repo and
          org you choose. Your token never leaves your machine.
        </p>
        <PRismButton to="/onboarding" variant="primary" size="lg">
          Connect GitHub
        </PRismButton>
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

.dashboard__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  padding: var(--s-5) var(--s-6) var(--s-4);
  border-bottom: 1px solid var(--border-1);
}

.dashboard__title {
  margin: 0;
  font-size: var(--fs-20);
  font-weight: 600;
  letter-spacing: -0.5px;
  color: var(--text-strong);
}

.dashboard__count {
  font-family: var(--font-mono);
  font-size: var(--fs-12);
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}

.dashboard__empty {
  flex: 1;
  display: grid;
  place-items: center;
  padding: var(--s-8) var(--s-6);
}

.dashboard-empty {
  max-width: 420px;
  text-align: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--s-3);
}

.dashboard-empty__mark {
  color: var(--text-strong);
  margin-bottom: var(--s-1);
}

.dashboard-empty__title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  letter-spacing: -0.3px;
  color: var(--text-strong);
}

.dashboard-empty__copy {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
  line-height: var(--lh-body);
}
</style>
