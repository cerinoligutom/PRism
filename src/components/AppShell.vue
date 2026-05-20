<script setup lang="ts">
import { RouterView } from "vue-router";
import SidebarNav from "./SidebarNav.vue";
import StatusBar from "./StatusBar.vue";
</script>

<template>
  <div class="app-shell">
    <SidebarNav class="app-shell__sidebar" />
    <main class="app-shell__main">
      <RouterView />
    </main>
    <StatusBar class="app-shell__status" />
  </div>
</template>

<style scoped>
.app-shell {
  display: grid;
  grid-template-columns: var(--sidebar-width) 1fr;
  grid-template-rows: 1fr 28px;
  grid-template-areas:
    "sidebar main"
    "status status";
  height: 100vh;
  width: 100vw;
  background: var(--bg-1);
  overflow: hidden;
}

.app-shell__sidebar {
  grid-area: sidebar;
  background: var(--bg-2);
  border-right: 1px solid var(--border-1);
  display: flex;
  flex-direction: column;
  min-height: 0;
}

.app-shell__main {
  grid-area: main;
  background: var(--bg-1);
  overflow: auto;
  min-width: 0;
  min-height: 0;
}

.app-shell__status {
  grid-area: status;
}
</style>
