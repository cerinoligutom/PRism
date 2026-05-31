<script setup lang="ts">
import { computed, onMounted, ref } from "vue";

import PRismButton from "@/components/ui/PRismButton.vue";
import PRismCallout from "@/components/ui/PRismCallout.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismSwitch from "@/components/ui/PRismSwitch.vue";
import { useAutoUpdate } from "@/composables/useAutoUpdate";
import { useAppSettings } from "@/stores/settings";

const settings = useAppSettings();
const auto = useAutoUpdate();

const persisting = ref(false);
const localError = ref<string | null>(null);
const upToDateNotice = ref(false);

const toggleChecked = computed<boolean>({
  get: () => settings.autoUpdateEnabled,
  set: (value) => {
    void persistToggle(value);
  },
});

const lastCheckAt = computed(() => settings.autoUpdateLastCheckAt);
const lastFailureMessage = computed(() => settings.autoUpdateLastFailureMessage);

async function persistToggle(next: boolean): Promise<void> {
  if (persisting.value) return;
  persisting.value = true;
  localError.value = null;
  try {
    await settings.update({
      notifications_enabled: settings.settings.notifications_enabled,
      notify_on_needs_attention: settings.settings.notify_on_needs_attention,
      auto_update_enabled: next,
      auto_update_interval_seconds:
        settings.settings.auto_update_interval_seconds,
      auto_archive_days: settings.settings.auto_archive_days,
      notification_retention_max: settings.settings.notification_retention_max,
    });
  } catch (err) {
    localError.value = err instanceof Error ? err.message : String(err);
  } finally {
    persisting.value = false;
  }
}

async function onCheckNow(): Promise<void> {
  upToDateNotice.value = false;
  try {
    const result = await auto.checkNow();
    if (!result.updateAvailable) {
      upToDateNotice.value = true;
    }
  } catch (err) {
    localError.value = err instanceof Error ? err.message : String(err);
  }
}

async function onInstallOnQuit(): Promise<void> {
  try {
    await auto.installOnQuit();
  } catch (err) {
    localError.value = err instanceof Error ? err.message : String(err);
  }
}

async function onInstallNow(): Promise<void> {
  try {
    await auto.installNow();
  } catch (err) {
    localError.value = err instanceof Error ? err.message : String(err);
  }
}

onMounted(() => {
  void settings.load();
  void auto.bindListeners();
});
</script>

<template>
  <div class="updates-panel">
    <header class="updates-panel__header">
      <h1 class="updates-panel__title">Updates</h1>
    </header>

    <p class="updates-panel__explainer">
      PRism can check GitHub for new releases on a 6-hour schedule. Updates
      are signed end-to-end; nothing installs without your say-so. Linux
      AppImage installs may need your launcher's help to swap binaries.
    </p>

    <section class="updates-panel__section">
      <div class="updates-panel__section-head">
        <h3 class="updates-panel__section-title">Auto-update</h3>
        <span class="updates-panel__section-desc">
          When on, PRism checks for new releases every 6 hours.
        </span>
      </div>

      <div class="updates-panel__row-list">
        <div class="set-row">
          <div>
            <div class="set-row__name">Automatically check for updates</div>
            <div class="set-row__desc">
              Background checks only happen when this is on. You can still
              run a check at any time below.
            </div>
          </div>
          <PRismSwitch
            v-model="toggleChecked"
            :disabled="persisting"
            aria-label="Automatically check for updates"
          />
        </div>
      </div>

      <div class="updates-panel__meta">
        <PRismButton
          variant="ghost"
          size="sm"
          :disabled="auto.checking"
          @click="onCheckNow"
        >
          {{ auto.checking ? "Checking..." : "Check now" }}
        </PRismButton>

        <div class="updates-panel__meta-text">
          <template v-if="lastCheckAt !== null">
            <span class="updates-panel__meta-line">
              Last checked
              <PRismRelativeTime :value="lastCheckAt" as="span" />.
            </span>
          </template>

          <span v-if="lastFailureMessage" class="updates-panel__meta-failure">
            Last check failed: {{ lastFailureMessage }}
          </span>

          <span
            v-if="upToDateNotice && !auto.updateAvailable"
            class="updates-panel__meta-ok"
          >
            You're on the latest version.
          </span>
        </div>
      </div>

      <PRismCallout
        v-if="auto.updateAvailable"
        variant="accent"
        class="updates-panel__available"
      >
        <template #icon>
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M8 2v8" /><path d="M5 7l3 3 3-3" /><path d="M3 13h10" />
          </svg>
        </template>
        <div class="updates-panel__available-body">
          <div>
            <div class="updates-panel__available-title">
              Version {{ auto.availableVersion }} is ready.
            </div>
            <div
              v-if="auto.releaseNotes"
              class="updates-panel__available-notes"
            >
              {{ auto.releaseNotes }}
            </div>
          </div>
          <div class="updates-panel__available-actions">
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
          </div>
        </div>
      </PRismCallout>

      <p v-if="localError" class="updates-panel__error">{{ localError }}</p>
    </section>
  </div>
</template>

<style scoped>
.updates-panel__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  margin-bottom: var(--s-6);
}

.updates-panel__title {
  margin: 0;
  font-size: var(--fs-24);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}

.updates-panel__explainer {
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: 1.55;
  margin: 0 0 var(--s-6);
  max-width: 600px;
}

.updates-panel__section {
  margin-bottom: var(--s-7);
}

.updates-panel__section-head {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: var(--s-4);
}

.updates-panel__section-title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
}

.updates-panel__section-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.updates-panel__row-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
  background: var(--border-1);
  border-radius: var(--r-3);
  overflow: hidden;
}

.updates-panel__meta {
  display: flex;
  align-items: center;
  gap: var(--s-4);
  margin-top: var(--s-4);
  flex-wrap: wrap;
}

.updates-panel__meta-text {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: var(--fs-12);
}

.updates-panel__meta-line {
  color: var(--text-mute);
}

.updates-panel__meta-failure {
  color: var(--danger);
}

.updates-panel__meta-ok {
  color: var(--text-mute);
}

.updates-panel__available {
  margin-top: var(--s-5);
}

.updates-panel__available-body {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

.updates-panel__available-title {
  font-weight: 600;
  color: var(--text-strong);
  font-size: var(--fs-13);
}

.updates-panel__available-notes {
  margin-top: 2px;
  font-size: var(--fs-12);
  color: var(--text-mute);
  white-space: pre-line;
  line-height: 1.5;
}

.updates-panel__available-actions {
  display: flex;
  gap: var(--s-2);
  align-items: center;
  flex-wrap: wrap;
}

.updates-panel__error {
  margin-top: var(--s-4);
  padding: 10px 14px;
  border-radius: var(--r-2);
  background: var(--danger-bg);
  color: var(--danger);
  font-size: var(--fs-12);
}
</style>
