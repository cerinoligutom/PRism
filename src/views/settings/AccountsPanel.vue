<script setup lang="ts">
import PRismButton from "@/components/ui/PRismButton.vue";

// Real Accounts panel (PAT add/list/remove, expired-token banner, scopes
// hint, re-auth flow) lands with issue #10. This placeholder shows the
// section chrome so the routing + scroll behaviour can be verified.
</script>

<template>
  <div class="accounts-panel">
    <header class="accounts-panel__header">
      <h1 class="accounts-panel__title">Accounts &amp; sync</h1>
      <span class="accounts-panel__sub">0 ACCOUNTS</span>
    </header>

    <section class="accounts-panel__section">
      <div class="accounts-panel__section-head">
        <h3 class="accounts-panel__section-title">GitHub accounts</h3>
        <span class="accounts-panel__section-desc">
          PATs are stored in the OS keychain. PRism never writes to disk or logs.
        </span>
      </div>

      <div class="accounts-panel__empty">
        <p class="accounts-panel__empty-copy">No accounts connected yet.</p>
        <PRismButton to="/onboarding" variant="primary">Connect an account</PRismButton>
      </div>
    </section>
  </div>
</template>

<style scoped>
.accounts-panel__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  margin-bottom: var(--s-6);
}

.accounts-panel__title {
  margin: 0;
  font-size: var(--fs-24);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}

.accounts-panel__sub {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  letter-spacing: 0.5px;
}

.accounts-panel__section {
  margin-bottom: var(--s-7);
}

.accounts-panel__section-head {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: var(--s-4);
}

.accounts-panel__section-title {
  margin: 0;
  font-size: var(--fs-14);
  font-weight: 600;
  color: var(--text-strong);
}

.accounts-panel__section-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.accounts-panel__empty {
  padding: var(--s-7) var(--s-6);
  background: var(--bg-2);
  border: 1px dashed var(--border-2);
  border-radius: var(--r-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  align-items: center;
}

.accounts-panel__empty-copy {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
}
</style>
