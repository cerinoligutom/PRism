<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { computed, onMounted } from "vue";
import { useRouter } from "vue-router";

import NotificationRow from "@/components/notifications/NotificationRow.vue";
import PRismButton from "@/components/ui/PRismButton.vue";
import { useAppearanceStore } from "@/stores/appearance";
import { useDashboardStore } from "@/stores/dashboard";
import { useNotificationsStore, type Notification } from "@/stores/notifications";
import { useAppSettings } from "@/stores/settings";
import { useToastStore } from "@/stores/toast";

/**
 * Persistent notifications inbox (issue #378). Lists the rows the dispatch
 * hook (ADR 0017) mirrored alongside each OS toast; clicking a row
 * deep-links into the local detail surface when the PR is still cached,
 * falls back to GitHub web when it isn't, and surfaces a "no longer
 * available" toast on the second failure.
 *
 * The view is read-mostly: the inbox row is written from Rust at dispatch
 * time. The header offers per-row dismissal via the row's own affordance
 * and "Clear all" at the top once the list is non-empty.
 */
interface PrCoordinatesMatch {
  readonly account_id: number;
  readonly pull_request_id: number;
  readonly number: number;
  readonly owner: string;
  readonly name: string;
  readonly view: "authored" | "assigned" | "watching" | "archive";
}

interface PrLookupErrorPayload {
  readonly kind: "not_found" | "internal";
}

const router = useRouter();
const notifications = useNotificationsStore();
const dashboard = useDashboardStore();
const appearance = useAppearanceStore();
const settings = useAppSettings();
const toast = useToastStore();

onMounted(async () => {
  // Load the settings row alongside the inbox so the cap-reached footer
  // has a real cap to compare against. The store ignores a duplicate
  // load if something else got there first.
  await Promise.all([notifications.load(), settings.load()]);
});

/**
 * Whether the inbox is at or near its configured retention cap (ADR 0028,
 * issue #380). "Near" is anything inside the top 5% so a user sitting on
 * 480 / 500 sees the footer one notification before the prune kicks in.
 * The footer is purely informational; the prune itself runs on every
 * insert from the Rust side.
 */
const isAtRetentionCap = computed<boolean>(() => {
  const cap = settings.notificationRetentionMax;
  if (cap <= 0) return false;
  if (notifications.count === 0) return false;
  return notifications.count >= Math.ceil(cap * 0.95);
});

async function openNotification(notification: Notification): Promise<void> {
  // Mark-on-click (ADR 0028 decision 3, issue #379). Fire-and-forget so
  // the deep-link work below doesn't wait on the backend write; the store
  // updates the row locally before the IPC round-trip lands.
  void notifications.markRead(notification.id);

  // State A: try the local lookup against `(host, owner, repo, pr_number)`.
  // The snapshot row doesn't carry the host directly (the v1 surface
  // assumes `github.com`); the lookup matches the host column on the
  // account, so we ride the default. A future enterprise-host surface
  // either denormalises host onto notifications or extends the lookup
  // signature - either way the call site stays.
  const match = await lookupPr(notification);
  if (match !== null) {
    dashboard.setAccountScope(match.account_id);
    await dashboard.setView(match.view);
    if (appearance.prDetailSurface === "route") {
      await router.push({
        name: "pr-detail",
        params: { view: match.view, id: match.pull_request_id },
      });
      return;
    }
    // Drawer surface: `PullRequestDrawer` is mounted from `DashboardView`,
    // not from this view (issue #400). Push to the matching dashboard route
    // first so the host exists, then set the expanded id - the store value
    // survives the navigation, and the drawer opens on the next paint.
    await router.push({ name: dashboardRouteName(match.view) });
    dashboard.setExpandedPullRequest(match.pull_request_id);
    return;
  }

  // State B: PR not in the local cache. Fall through to the canonical
  // GitHub URL via the opener plugin.
  const url = githubPrUrl(notification);
  try {
    await openUrl(url);
  } catch {
    // State C: opener failed. Surface a one-line toast so the user knows
    // the row's target is unreachable; the row itself stays put so they
    // can retry or dismiss it manually.
    toast.show("This PR is no longer available.", {
      variant: "warning",
      duration: 4000,
    });
  }
}

async function lookupPr(
  notification: Notification,
): Promise<PrCoordinatesMatch | null> {
  try {
    return await invoke<PrCoordinatesMatch>("pr_lookup_by_coordinates", {
      host: "github.com",
      owner: notification.owner,
      repo: notification.repo,
      number: notification.pr_number,
    });
  } catch (err) {
    if (isNotFound(err)) return null;
    // Internal error: treat as a miss so the State-B fallback still
    // attempts to open the canonical URL.
    return null;
  }
}

function isNotFound(err: unknown): boolean {
  if (typeof err !== "object" || err === null) return false;
  const payload = err as Partial<PrLookupErrorPayload>;
  return payload.kind === "not_found";
}

function githubPrUrl(notification: Notification): string {
  const owner = encodeURIComponent(notification.owner);
  const repo = encodeURIComponent(notification.repo);
  return `https://github.com/${owner}/${repo}/pull/${notification.pr_number}`;
}

function dashboardRouteName(view: PrCoordinatesMatch["view"]): string {
  // The `assigned` Tauri view name maps to the `review-requested` route
  // slug (see `src/router/index.ts`); the other three views share their
  // name with the route suffix.
  return view === "assigned"
    ? "dashboard.review-requested"
    : `dashboard.${view}`;
}

async function dismissNotification(notification: Notification): Promise<void> {
  try {
    await notifications.deleteOne(notification.id);
  } catch (err) {
    toast.show("Couldn't dismiss that notification.", { variant: "danger" });
    // Surface the underlying error to the console; the store already
    // captured it on `lastError` for any future diagnostic surface.
    console.warn("notifications.deleteOne failed", err);
  }
}

async function clearAll(): Promise<void> {
  try {
    await notifications.clearAll();
  } catch (err) {
    toast.show("Couldn't clear notifications.", { variant: "danger" });
    console.warn("notifications.clearAll failed", err);
  }
}

async function markAllRead(): Promise<void> {
  try {
    await notifications.markAllRead();
  } catch (err) {
    toast.show("Couldn't mark notifications as read.", { variant: "danger" });
    console.warn("notifications.markAllRead failed", err);
  }
}
</script>

<template>
  <section class="notifications-view">
    <header class="notifications-view__header">
      <div>
        <h1 class="notifications-view__title">Notifications</h1>
        <p class="notifications-view__subtitle">
          Mirror of the OS toasts PRism dispatched. Older entries stay until
          you dismiss them.
        </p>
      </div>
      <div class="notifications-view__actions">
        <PRismButton
          variant="ghost"
          size="sm"
          :disabled="notifications.unreadCount === 0"
          @click="markAllRead"
        >
          Mark all as read
        </PRismButton>
        <PRismButton
          variant="ghost"
          size="sm"
          :disabled="notifications.count === 0"
          @click="clearAll"
        >
          Clear all
        </PRismButton>
      </div>
    </header>

    <div v-if="notifications.loading && notifications.isEmpty" class="notifications-view__status">
      Loading...
    </div>

    <div v-else-if="notifications.isEmpty" class="notifications-view__empty">
      <span class="notifications-view__empty-mark" aria-hidden="true">
        <svg width="36" height="36" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3.5 6.5a4.5 4.5 0 019 0v3l1 2H2.5l1-2z" />
          <path d="M6.5 13.5a1.5 1.5 0 003 0" />
        </svg>
      </span>
      <h2 class="notifications-view__empty-title">No notifications yet</h2>
      <p class="notifications-view__empty-copy">
        When a PR needs your attention or someone mentions you, the OS toast
        will land here too so you can pick it up later.
      </p>
    </div>

    <ul v-else class="notifications-view__list">
      <li v-for="n in notifications.list" :key="n.id">
        <NotificationRow
          :notification="n"
          @open="openNotification"
          @delete="dismissNotification"
        />
      </li>
    </ul>

    <p
      v-if="!notifications.isEmpty && isAtRetentionCap"
      class="notifications-view__cap-footer text-xs text-fg-mute"
    >
      Showing latest {{ notifications.count }} notifications - older are
      dropped automatically.
    </p>
  </section>
</template>

<style scoped>
.notifications-view {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
  padding: var(--s-5);
  max-width: 880px;
  margin: 0 auto;
  width: 100%;
}

.notifications-view__header {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--s-4);
}

.notifications-view__actions {
  display: flex;
  align-items: center;
  gap: var(--s-2);
}

.notifications-view__title {
  margin: 0;
  font-size: var(--fs-20);
  font-weight: 600;
  letter-spacing: -0.01em;
  color: var(--text-strong);
}

.notifications-view__subtitle {
  margin: 4px 0 0;
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
  max-width: 540px;
}

.notifications-view__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.notifications-view__status {
  color: var(--text-mute);
  font-size: var(--fs-13);
  padding: var(--s-3) 0;
}

.notifications-view__empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  gap: var(--s-2);
  padding: var(--s-6) var(--s-4);
  background: var(--bg-2);
  border: 1px dashed var(--border-1);
  border-radius: var(--r-2);
}

.notifications-view__empty-mark {
  color: var(--text-faint);
  display: inline-flex;
}

.notifications-view__empty-title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  letter-spacing: -0.2px;
  color: var(--text-strong);
}

.notifications-view__empty-copy {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
  max-width: 460px;
  line-height: var(--lh-body);
}

.notifications-view__cap-footer {
  margin: var(--s-2) 0 0;
  text-align: center;
  line-height: var(--lh-body);
}
</style>
