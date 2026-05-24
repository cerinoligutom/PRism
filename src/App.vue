<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";

import AppShell from "@/components/AppShell.vue";
import PRismToastViewport from "@/components/ui/PRismToastViewport.vue";
import WhatsNewDialog from "@/components/WhatsNewDialog.vue";
import { useDeepLinkRouter } from "@/composables/useDeepLinkRouter";
import { useKeyboardShortcuts } from "@/composables/useKeyboardShortcuts";
import { useAppMetadata } from "@/composables/useAppMetadata";
import { useNotificationRouter } from "@/composables/useNotificationRouter";
import changelogContent from "../CHANGELOG.md?raw";
import {
  parseChangelog,
  sectionsSince,
  type ChangelogEntry,
} from "@/lib/changelog";
import { useAppSettings } from "@/stores/settings";
import { useConversationStore } from "@/stores/conversation";

useKeyboardShortcuts();
useNotificationRouter();
useDeepLinkRouter();

// Bind the conversation cache to `sync://status` once at app scope so it
// stays subscribed across drawer / route navigation (issue #337). The store
// owns the listener lifecycle via `useTauriListener`; `unbind()` on app
// teardown releases the registration.
const conversation = useConversationStore();
void conversation.bind();

// In-app "What's new" wiring (ADR 0025).
//
// Order matters: both `app_metadata.version` and the DB-backed
// `last_seen_version` cursor must be in hand before we decide whether to
// open the dialog. The settings store's default `last_seen_version = null`
// is the same sentinel the first-install path uses, so reading it before
// `load()` resolves would silently clobber a real cursor with the current
// version on every launch.
//
//   1. Fresh install (`last_seen_version === null` post-load): silently
//      echo the current version into the DB. No dialog. The next version
//      transition is what actually opens it.
//   2. Otherwise compute `sectionsSince(...)`. If it returns >= 1 entry,
//      open the dialog with the concatenated slice.
//   3. On dismiss (Got it / Esc / backdrop click), advance the cursor to
//      the running version.
const { metadata } = useAppMetadata();
const settings = useAppSettings();
const dialogEntries = ref<readonly ChangelogEntry[]>([]);
const currentVersion = ref<string>("");

// Track whether the gate has already run for this launch. Both inputs
// arrive asynchronously and might resolve in either order; we only want
// to fire the cursor logic once they've both landed.
let gateEvaluated = false;

// Parse once on first render. The bundled `?raw` import is a constant, so
// the entries are stable across the app lifetime.
const allEntries = parseChangelog(changelogContent);

onMounted(async () => {
  // Load the DB-backed cursor up front. The metadata fetch is fire-and-
  // forget under `useAppMetadata`'s singleton; we wait on it reactively in
  // the watcher below.
  await settings.load();
  maybeEvaluateGate();
});

watch(
  () => metadata.value?.version ?? null,
  () => {
    maybeEvaluateGate();
  },
);

async function maybeEvaluateGate(): Promise<void> {
  if (gateEvaluated) return;
  if (settings.loading) return;
  const meta = metadata.value;
  if (meta === null) return; // metadata fetch not yet resolved
  gateEvaluated = true;

  currentVersion.value = meta.version;
  const lastSeen = settings.lastSeenVersion;
  if (lastSeen === null) {
    try {
      await settings.setLastSeenVersion(meta.version);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("could not initialise last_seen_version", err);
    }
    return;
  }

  const slice = sectionsSince(allEntries, lastSeen, meta.version);
  if (slice.length > 0) {
    dialogEntries.value = slice;
  }
}

onBeforeUnmount(() => {
  conversation.unbind();
});

async function handleDismiss(): Promise<void> {
  const current = currentVersion.value;
  dialogEntries.value = [];
  if (current === "") return;
  try {
    await settings.setLastSeenVersion(current);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("could not advance last_seen_version", err);
  }
}
</script>

<template>
  <AppShell />
  <PRismToastViewport />
  <WhatsNewDialog
    :entries="dialogEntries"
    :current-version="currentVersion"
    @dismissed="handleDismiss"
  />
</template>
