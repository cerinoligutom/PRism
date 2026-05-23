<script setup lang="ts">
import { onMounted } from "vue";
import { RouterView } from "vue-router";

import PRismButton from "@/components/ui/PRismButton.vue";
import { useAutoUpdate } from "@/composables/useAutoUpdate";
import SidebarNav from "./SidebarNav.vue";
import StatusBar from "./StatusBar.vue";

// Auto-update store (ADR-0024, issue #308). Bound here so the banner
// hears the worker's `update://available` event regardless of which
// view is mounted.
const auto = useAutoUpdate();

onMounted(() => {
  void auto.bindListeners();
});

async function onInstallOnQuit(): Promise<void> {
  try {
    await auto.installOnQuit();
    // Drop the banner once queued; the Settings panel still shows the
    // pending state.
    auto.dismissBanner();
  } catch {
    // Error already populated lastCheckError; the Settings panel surfaces it.
  }
}

async function onInstallNow(): Promise<void> {
  try {
    await auto.installNow();
  } catch {
    // Error already populated lastCheckError; the Settings panel surfaces it.
  }
}
</script>

<template>
  <div class="app-shell">
    <SidebarNav class="app-shell__sidebar" />
    <main class="app-shell__main">
      <div
        v-if="auto.bannerVisible"
        class="app-shell__update-banner"
        role="status"
        aria-live="polite"
      >
        <span class="app-shell__update-banner-text">
          PRism {{ auto.availableVersion }} is ready to install.
        </span>
        <div class="app-shell__update-banner-actions">
          <PRismButton
            variant="primary"
            size="sm"
            :disabled="auto.installing"
            @click="onInstallOnQuit"
          >
            Install on next quit
          </PRismButton>
          <PRismButton
            variant="ghost"
            size="sm"
            :disabled="auto.installing"
            @click="onInstallNow"
          >
            {{ auto.installing ? "Installing..." : "Install now" }}
          </PRismButton>
          <button
            class="app-shell__update-banner-dismiss"
            type="button"
            aria-label="Dismiss update banner"
            @click="auto.dismissBanner"
          >
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </div>
      </div>
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

.app-shell__update-banner {
  position: sticky;
  top: 0;
  z-index: 5;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-4);
  padding: 10px 16px;
  background: var(--accent-bg);
  border-bottom: 1px solid oklch(0.4 0.12 var(--accent-h) / 0.4);
  color: var(--accent-strong);
  font-size: var(--fs-12);
  flex-wrap: wrap;
}

.app-shell__update-banner-text {
  font-weight: 500;
}

.app-shell__update-banner-actions {
  display: flex;
  gap: var(--s-2);
  align-items: center;
}

.app-shell__update-banner-dismiss {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  background: transparent;
  border: none;
  border-radius: var(--r-2);
  color: var(--accent-strong);
  cursor: pointer;
  padding: 0;
}

.app-shell__update-banner-dismiss:hover {
  background: oklch(0.4 0.12 var(--accent-h) / 0.15);
}
</style>
