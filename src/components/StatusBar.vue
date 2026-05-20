<script setup lang="ts">
// Sync state, account count, and API budget are wired by the sync worker (#13).
// Until then the bar shows a static "idle" footprint so the layout grid
// holds its shape and we can lay out the keyboard hint chips.
</script>

<template>
  <footer class="status-bar">
    <span class="status-bar__item">
      <span class="dot" />
      <span>Idle &middot; no accounts</span>
    </span>
    <span class="status-bar__spacer" />
    <span class="status-bar__item"><kbd>⌘</kbd><kbd>,</kbd> Settings</span>
  </footer>
</template>

<style scoped>
.status-bar {
  height: 28px;
  border-top: 1px solid var(--border-1);
  background: var(--bg-2);
  display: flex;
  align-items: center;
  padding: 0 var(--s-4);
  gap: var(--s-4);
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  letter-spacing: 0.3px;
}

.status-bar__item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}

.status-bar__spacer {
  flex: 1;
}

.status-bar kbd {
  font-size: var(--fs-9);
  padding: 0 4px;
}
</style>
