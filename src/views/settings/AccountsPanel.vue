<script setup lang="ts">
// Real Accounts panel (PAT add/list/remove, expired-token banner, scopes
// hint, re-auth flow) lands with issue #10. This placeholder shows the
// section chrome so the routing + scroll behaviour can be verified.
</script>

<template>
  <div>
    <header class="head">
      <h1>Accounts &amp; sync</h1>
      <span class="sub">0 ACCOUNTS</span>
    </header>

    <div class="section">
      <div class="section-head">
        <h3>GitHub accounts</h3>
        <span class="desc">PATs are stored in the OS keychain. PRism never writes to disk or logs.</span>
      </div>

      <div class="empty">
        <p>No accounts connected yet.</p>
        <a class="btn btn-primary" href="/onboarding">Connect an account</a>
      </div>
    </div>
  </div>
</template>

<style scoped>
.head {
  display: flex;
  align-items: baseline;
  gap: 12px;
  margin-bottom: 24px;
}
.head h1 {
  margin: 0;
  font-size: 24px;
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}
.head .sub {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--text-faint);
  letter-spacing: 0.5px;
}

.section { margin-bottom: 36px; }
.section-head {
  display: flex;
  align-items: baseline;
  gap: 12px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: 18px;
}
.section-head h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-strong);
}
.section-head .desc {
  font-size: 12px;
  color: var(--text-mute);
}

.empty {
  padding: 32px 24px;
  background: var(--bg-2);
  border: 1px dashed var(--border-2);
  border-radius: var(--r-3);
  display: flex;
  flex-direction: column;
  gap: 12px;
  align-items: center;
}
.empty p {
  margin: 0;
  font-size: 13px;
  color: var(--text-mute);
}
</style>
