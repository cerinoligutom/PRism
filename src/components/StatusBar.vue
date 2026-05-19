<script setup lang="ts">
// Sync state, account count, and API budget are wired by the sync worker (#13).
// Until then the bar shows a static "idle" footprint so the layout grid
// holds its shape and we can lay out the keyboard hint chips.
</script>

<template>
  <footer class="status-bar">
    <span class="item">
      <span class="dot" />
      <span>Idle &middot; no accounts</span>
    </span>
    <span class="spacer" />
    <span class="item"><kbd>⌘</kbd><kbd>,</kbd> Settings</span>
  </footer>
</template>

<style scoped>
.status-bar {
  height: 28px;
  border-top: 1px solid var(--border-1);
  background: var(--bg-2);
  display: flex;
  align-items: center;
  padding: 0 16px;
  gap: 16px;
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--text-faint);
  letter-spacing: 0.3px;
}
.status-bar .item {
  display: inline-flex;
  align-items: center;
  gap: 5px;
}
.status-bar .spacer { flex: 1; }
.status-bar kbd { font-size: 9px; padding: 0 4px; }
</style>
