<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, nextTick, ref, watch } from "vue";
import { storeToRefs } from "pinia";

import { useConversationStore } from "@/stores/conversation";
import { useDashboardStore } from "@/stores/dashboard";
import { threadAnchorId, useThreadDeepLink } from "@/composables/useThreadDeepLink";

import type { PullRequestThread } from "@/types/conversation";
import type { ThreadsSummary } from "@/types/dashboard";
import ThreadsBar from "@/components/dashboard/ThreadsBar.vue";
import PRismButton from "@/components/ui/PRismButton.vue";
import PRismPopover from "@/components/ui/PRismPopover.vue";
import PRismIconLegend from "@/components/ui/PRismIconLegend.vue";
import ConversationStats from "./ConversationStats.vue";
import IssueCommentsTab from "./IssueCommentsTab.vue";
import ReviewsTab from "./ReviewsTab.vue";
import StatusTimelineTab from "./StatusTimelineTab.vue";
import ThreadsList from "./ThreadsList.vue";
import ThreadStateIcon from "./icons/ThreadStateIcon.vue";

interface Props {
  pullRequestId: number;
}

const props = defineProps<Props>();

type TabKey = "threads" | "reviews" | "comments" | "timeline";

interface TabSpec {
  readonly key: TabKey;
  readonly label: string;
  readonly count: number | null;
}

const store = useConversationStore();
const dashboard = useDashboardStore();
const threadDeepLink = useThreadDeepLink();
const { cache, loading: storeLoading, errors } = storeToRefs(store);
const { pullRequests } = storeToRefs(dashboard);

const activeTab = ref<TabKey>("threads");

const conversation = computed(() => cache.value.get(props.pullRequestId) ?? null);

/**
 * The status-timeline tab needs the dashboard DTO for its `pullRequest` prop.
 * Resolving from the dashboard store keeps `PullRequestConversation`'s public
 * surface to a single `pullRequestId` prop while still satisfying the
 * `StatusTimelineTab` contract.
 */
const dashboardRow = computed(
  () => pullRequests.value.find((p) => p.id === props.pullRequestId) ?? null,
);

/**
 * Relation owners the viewer holds for this PR. The mark-seen commands take a
 * single `account_id`, so the store fans out across these (mirroring the
 * server-side `auto_mark_units_seen` fan-out). Empty for a Tracked-view PR
 * with no relation row, in which case the mark-seen affordances stay hidden -
 * there's no "you" whose watermark to advance.
 */
const accountIds = computed<readonly number[]>(
  () => dashboardRow.value?.account_ids ?? [],
);

const canMarkSeen = computed<boolean>(() => accountIds.value.length > 0);

const isLoading = computed<boolean>(() =>
  storeLoading.value.has(props.pullRequestId),
);

const error = computed<string | null>(
  () => errors.value.get(props.pullRequestId) ?? null,
);

const tabs = computed<readonly TabSpec[]>(() => {
  const c = conversation.value;
  return [
    {
      key: "threads",
      label: "Threads",
      count: c?.threads.length ?? null,
    },
    {
      key: "reviews",
      label: "Reviews",
      count: c?.reviews.length ?? null,
    },
    {
      key: "comments",
      label: "Comments",
      count: c?.issue_comments.length ?? null,
    },
    {
      key: "timeline",
      label: "Timeline",
      count: null,
    },
  ];
});

const threadsSummary = computed<string>(() => {
  const c = conversation.value;
  if (c === null) return "Loading conversation…";
  const total = c.stats.threads_total;
  const unresolved = c.stats.threads_unresolved;
  if (total === 0) return "No review threads yet.";
  const totalLabel = total === 1 ? "thread" : "threads";
  // Unread count is derived per-thread (`thread.unread` from M4-A's
  // read-state + per-thread `last_reply_at` comparison). Only surface when
  // there's something unread so the label doesn't carry redundant "0 unread".
  const unread = c.threads.filter((t) => t.unread).length;
  const base = `${total} ${totalLabel} · ${unresolved} unresolved`;
  return unread > 0 ? `${base} · ${unread} unread` : base;
});

/**
 * Mount the dashboard's `ThreadsBar` against the same four-bucket breakdown the
 * row consumes (issue #102, ADR 0012). The stats come from the pre-aggregated
 * `pull_requests.threads_*` rollup written by the sync worker, so the bar
 * renders identical numbers + tooltips on both surfaces by construction.
 * Previously this component re-bucketed from per-thread `state`, which mis-
 * classified outdated-but-resolved threads.
 */
const threadsSummaryForBar = computed<ThreadsSummary | null>(() => {
  const c = conversation.value;
  if (c === null) return null;
  return {
    total: c.stats.threads_total,
    unresolved_involved: c.stats.threads_unresolved_involved,
    unresolved_uninvolved: c.stats.threads_unresolved_uninvolved,
    resolved_involved: c.stats.threads_resolved_involved,
    resolved_uninvolved: c.stats.threads_resolved_uninvolved,
  };
});

/**
 * Static row scaffold for the thread badge legend. The label text mirrors the
 * prior inline markup (the `&middot;` entities resolve to the same middle dot);
 * each row's bespoke badge / chip / swatch comes through `PRismIconLegend`'s
 * `#icon` slot, keyed by `id`.
 */
const threadLegendSections = [
  {
    title: "State",
    rows: [
      { id: "unresolved-uninvolved", label: "Unresolved" },
      { id: "unresolved-involved", label: "Unresolved · you're in it" },
      { id: "resolved-uninvolved", label: "Resolved" },
      { id: "resolved-involved", label: "Resolved · was yours" },
    ],
  },
  {
    title: "Modifier",
    rows: [
      { id: "mine", label: "You're a participant" },
      { id: "outdated", label: "Line no longer exists" },
    ],
  },
  {
    title: "Colour key",
    rows: [
      { id: "warm", label: "Warm · involves you" },
      { id: "cool", label: "Cool · others only" },
    ],
  },
] as const;

async function loadConversation(): Promise<void> {
  try {
    await store.load(props.pullRequestId);
    await scrollToPendingThread();
  } catch {
    // Error message lands in the store; UI surfaces it via the `error` computed.
  }
}

/**
 * Best-effort deep-link scroll (ADR 0031, issue #437). A notification open
 * path may have recorded a thread `node_id` to land on. After threads load,
 * switch to the Threads tab, wait a paint for the cards to mount, then
 * `scrollIntoView` the matching anchor and briefly highlight it. If the thread
 * isn't present (pruned / closed / legacy row without a node_id), the lookup
 * misses and the open degrades to just showing the PR - no error.
 */
async function scrollToPendingThread(): Promise<void> {
  const target = threadDeepLink.takePendingThread();
  if (target === null) return;
  const targetThread = (conversation.value?.threads ?? []).find(
    (t) => t.node_id === target,
  );
  if (targetThread === undefined) return;
  activeTab.value = "threads";
  // ADR 0033 deep-link-to-seen: arriving at a thread via a notification deep
  // link is genuine attention, so mark it seen (same handler as the manual
  // button / expand-to-seen). No-op when already seen or with no relation owner.
  if (canMarkSeen.value && targetThread.unread) {
    void onMarkThreadSeen(targetThread);
  }
  await nextTick();
  const el = document.getElementById(threadAnchorId(target));
  if (el === null) return;
  el.scrollIntoView({ behavior: "smooth", block: "center" });
  el.classList.add("thread-card--deep-link");
  window.setTimeout(() => el.classList.remove("thread-card--deep-link"), 2000);
}

async function onMarkThreadSeen(thread: PullRequestThread): Promise<void> {
  await store.markThreadSeen(
    props.pullRequestId,
    accountIds.value,
    thread.node_id,
  );
}

async function onMarkGeneralStreamSeen(): Promise<void> {
  await store.markGeneralStreamSeen(props.pullRequestId, accountIds.value);
}

async function onMarkReviewsSeen(): Promise<void> {
  await store.markReviewsSeen(props.pullRequestId, accountIds.value);
}

function setTab(next: TabKey): void {
  activeTab.value = next;
}

function retry(): void {
  store.invalidate(props.pullRequestId);
  void loadConversation();
}

/**
 * Tab-dwell auto-seen (ADR 0033). Staying on the Comments tab for ~1s marks the
 * general stream seen; staying on the Reviews tab marks the reviews unit seen.
 * Dwell (not scroll-to-end) is the deliberate-interaction signal; hover is
 * explicitly not a trigger. The pending timer is cleared on every tab change
 * and on unmount so no mark-seen fires for a tab the user only passed through.
 */
const DWELL_MS = 1000;
let dwellTimer: ReturnType<typeof setTimeout> | null = null;

function clearDwellTimer(): void {
  if (dwellTimer !== null) {
    clearTimeout(dwellTimer);
    dwellTimer = null;
  }
}

watch(
  activeTab,
  (tab) => {
    clearDwellTimer();
    if (!canMarkSeen.value) return;
    if (tab !== "comments" && tab !== "reviews") return;
    dwellTimer = setTimeout(() => {
      dwellTimer = null;
      // Re-check the active tab: a fast switch-and-back could land the timer
      // on the wrong unit otherwise. Mark-seen is MAX-only / idempotent, so
      // firing on an already-seen unit is harmless.
      if (activeTab.value !== tab) return;
      if (tab === "comments") void onMarkGeneralStreamSeen();
      else void onMarkReviewsSeen();
    }, DWELL_MS);
  },
  { immediate: true },
);

onMounted(() => {
  store.acquire(props.pullRequestId);
  void loadConversation();
});

onBeforeUnmount(() => {
  clearDwellTimer();
  store.release(props.pullRequestId);
});

// React to host swapping the id without an unmount. The cache returns
// immediately when warm, so swapping between PRs in the same drawer is cheap.
// The acquire / release pair shifts to the new id so the sync-cycle refresh
// targets the conversation the user is actually looking at.
watch(
  () => props.pullRequestId,
  (next, previous) => {
    if (previous !== undefined) store.release(previous);
    store.acquire(next);
    activeTab.value = "threads";
    void loadConversation();
  },
);
</script>

<template>
  <div class="pr-conversation">
    <nav class="pr-conversation__tabs" role="tablist" aria-label="Pull request conversation">
      <button
        v-for="tab in tabs"
        :key="tab.key"
        type="button"
        role="tab"
        :aria-selected="activeTab === tab.key"
        :class="['pr-conversation__tab', activeTab === tab.key && 'pr-conversation__tab--active']"
        @click="setTab(tab.key)"
      >
        {{ tab.label }}
        <span v-if="tab.count !== null" class="pr-conversation__tab-count">{{ tab.count }}</span>
      </button>
    </nav>

    <div v-if="error !== null" class="pr-conversation__error" role="alert">
      <p class="pr-conversation__error-text">{{ error }}</p>
      <button type="button" class="btn btn-ghost btn-sm" @click="retry">Retry</button>
    </div>

    <div v-else-if="isLoading && conversation === null" class="pr-conversation__loading" aria-busy="true">
      <span class="dot dot-pulse" aria-hidden="true"></span>
      <span>Loading conversation…</span>
    </div>

    <div v-else-if="conversation !== null" class="pr-conversation__body">
      <!-- Stats sidebar stays mounted as the outer right rail so every tab
           (Threads, Reviews, Comments, Timeline) renders against the same
           "Conversation stats" surface. The active tab swaps the left column. -->
      <section class="pr-conversation__layout">
        <div class="pr-conversation__main-col">
          <template v-if="activeTab === 'threads'">
            <div class="pr-conversation__col-head">
              <div class="pr-conversation__col-title-block">
                <span class="pr-conversation__col-title">Conversation · {{ threadsSummary }}</span>
                <PRismPopover :as-child="true" side="bottom" align="start">
                  <button
                    type="button"
                    class="btn btn-icon btn-sm pr-conversation__legend-btn"
                    aria-label="Thread badge legend"
                  >
                    <svg
                      width="14"
                      height="14"
                      viewBox="0 0 16 16"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.5"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      aria-hidden="true"
                    >
                      <circle cx="8" cy="8" r="6.5" />
                      <line x1="8" y1="11.25" x2="8" y2="7.25" />
                      <circle cx="8" cy="5" r="0.6" fill="currentColor" stroke="none" />
                    </svg>
                  </button>
                  <template #content>
                    <PRismIconLegend
                      region-label="Thread badge legend"
                      :sections="threadLegendSections"
                    >
                      <template #icon="{ id }">
                        <span
                          v-if="id === 'unresolved-uninvolved'"
                          class="thread-card__state thread-card__state--unresolved-uninvolved"
                        >
                          <ThreadStateIcon state="unresolved" />
                        </span>
                        <span
                          v-else-if="id === 'unresolved-involved'"
                          class="thread-card__state thread-card__state--unresolved-involved"
                        >
                          <ThreadStateIcon state="unresolved" />
                        </span>
                        <span
                          v-else-if="id === 'resolved-uninvolved'"
                          class="thread-card__state thread-card__state--resolved-uninvolved"
                        >
                          <ThreadStateIcon state="resolved" />
                        </span>
                        <span
                          v-else-if="id === 'resolved-involved'"
                          class="thread-card__state thread-card__state--resolved-involved"
                        >
                          <ThreadStateIcon state="resolved" />
                        </span>
                        <span
                          v-else-if="id === 'mine'"
                          class="thread-card__chip thread-card__chip--mine"
                        >INVOLVED</span>
                        <span
                          v-else-if="id === 'outdated'"
                          class="thread-card__chip thread-card__chip--outdated"
                        >OUTDATED</span>
                        <span
                          v-else-if="id === 'warm'"
                          class="legend-swatch-pair"
                          aria-hidden="true"
                        >
                          <span class="legend-swatch legend-swatch--warning"></span>
                          <span class="legend-swatch legend-swatch--success"></span>
                        </span>
                        <span
                          v-else-if="id === 'cool'"
                          class="legend-swatch-pair"
                          aria-hidden="true"
                        >
                          <span class="legend-swatch legend-swatch--danger"></span>
                          <span class="legend-swatch legend-swatch--info"></span>
                        </span>
                      </template>
                    </PRismIconLegend>
                    <RouterLink
                      class="legend-footer-link"
                      :to="{ name: 'signals', hash: '#threads' }"
                    >
                      See how signals work &rarr;
                    </RouterLink>
                  </template>
                </PRismPopover>
              </div>
            </div>

            <div v-if="conversation.stats.threads_total > 0" class="pr-conversation__rollup">
              <ThreadsBar :threads="threadsSummaryForBar" />
            </div>

            <ThreadsList
              :threads="conversation.threads"
              :thread-comments="conversation.thread_comments"
              :can-mark-seen="canMarkSeen"
              @mark-seen="onMarkThreadSeen"
            />
          </template>

          <template v-else-if="activeTab === 'reviews'">
            <div
              v-if="canMarkSeen && conversation.reviews.length > 0"
              class="pr-conversation__col-head pr-conversation__col-head--end"
            >
              <PRismButton
                variant="ghost"
                size="sm"
                @click="onMarkReviewsSeen"
              >
                Mark all seen
              </PRismButton>
            </div>
            <ReviewsTab :reviews="conversation.reviews" />
          </template>

          <template v-else-if="activeTab === 'comments'">
            <div
              v-if="canMarkSeen && conversation.issue_comments.length > 0"
              class="pr-conversation__col-head pr-conversation__col-head--end"
            >
              <PRismButton
                variant="ghost"
                size="sm"
                @click="onMarkGeneralStreamSeen"
              >
                Mark all seen
              </PRismButton>
            </div>
            <IssueCommentsTab :issue-comments="conversation.issue_comments" />
          </template>

          <template v-else>
            <StatusTimelineTab v-if="dashboardRow !== null" :pull-request="dashboardRow" />
            <div v-else class="pr-conversation__placeholder">
              Timeline unavailable until the dashboard list has loaded.
            </div>
          </template>
        </div>

        <aside class="pr-conversation__meta-col">
          <ConversationStats :stats="conversation.stats" />
        </aside>
      </section>
    </div>

    <div v-else class="pr-conversation__placeholder">
      No conversation data available.
    </div>
  </div>
</template>

<style scoped>
.pr-conversation {
  display: flex;
  flex-direction: column;
  min-height: 0;
  background: var(--bg-2);
}

.pr-conversation__tabs {
  display: flex;
  align-items: center;
  gap: 0;
  padding: 0 var(--s-6);
  border-bottom: 1px solid var(--border-1);
  background: var(--bg-2);
}

.pr-conversation__tab {
  background: transparent;
  border: 0;
  padding: 10px 14px;
  color: var(--text-mute);
  font-size: var(--fs-12);
  font-weight: 500;
  border-bottom: 2px solid transparent;
  cursor: pointer;
  margin-bottom: -1px;
  display: flex;
  align-items: center;
  gap: 6px;
}

.pr-conversation__tab:hover {
  color: var(--text);
}

.pr-conversation__tab:focus-visible {
  outline: none;
  box-shadow: inset 0 0 0 2px var(--focus-ring);
  border-radius: 2px;
}

.pr-conversation__tab--active {
  color: var(--text-strong);
  border-bottom-color: var(--accent);
}

.pr-conversation__tab-count {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  padding: 0 5px;
  border-radius: 9px;
  background: var(--bg-3);
}

.pr-conversation__tab--active .pr-conversation__tab-count {
  background: var(--accent-bg);
  color: var(--accent-strong);
}

.pr-conversation__loading {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  padding: var(--s-7) var(--s-6);
  color: var(--text-mute);
  font-size: var(--fs-12);
}

.pr-conversation__error {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--s-3);
  padding: var(--s-7) var(--s-6);
  text-align: center;
}

.pr-conversation__error-text {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--danger);
}

.pr-conversation__placeholder {
  padding: var(--s-7) var(--s-6);
  text-align: center;
  color: var(--text-faint);
  font-size: var(--fs-12);
}

.pr-conversation__body {
  flex: 1;
  min-height: 0;
  /* Wide-viewport layout: each column owns its own scroll so the stats
   * sidebar stays visible while the active tab scrolls under it. Narrow
   * viewport (<= 900px) flips this back to a single body scroll, see the
   * media query below. */
  overflow: hidden;
}

.pr-conversation__layout {
  display: grid;
  grid-template-columns: 1fr 320px;
  gap: 0;
  height: 100%;
  min-height: 0;
}

.pr-conversation__main-col {
  padding: 18px 24px 20px;
  border-right: 1px solid var(--border-1);
  min-width: 0;
  min-height: 0;
  overflow-y: auto;
}

.pr-conversation__meta-col {
  padding: 18px 24px 20px;
  display: flex;
  flex-direction: column;
  gap: 18px;
  background: var(--bg-2);
  min-height: 0;
  overflow-y: auto;
}

.pr-conversation__col-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: var(--s-3);
  gap: var(--s-3);
  flex-wrap: wrap;
}

.pr-conversation__col-head--end {
  justify-content: flex-end;
}

.pr-conversation__col-title-block {
  display: flex;
  align-items: center;
  gap: 6px;
}

.pr-conversation__col-title {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  letter-spacing: 1px;
  text-transform: uppercase;
  color: var(--text-faint);
}

/* Pulls colour towards the accent so it reads as "tap me for help" rather
 * than the default ghosted icon-button look. Inherits all other chrome from
 * `.btn.btn-icon.btn-sm` in `primitives.css`. */
.pr-conversation__legend-btn {
  color: var(--accent);
}

.pr-conversation__legend-btn:hover:not(:disabled) {
  color: var(--accent-strong);
}

.pr-conversation__rollup {
  margin-bottom: var(--s-4);
}

@media (max-width: 900px) {
  /* Stacked layout reverts to a single body scroll: an inner per-column scroll
   * would trap the sidebar behind a long active tab on narrow viewports. */
  .pr-conversation__body {
    overflow-y: auto;
  }

  .pr-conversation__layout {
    grid-template-columns: 1fr;
    height: auto;
  }

  .pr-conversation__main-col {
    border-right: 0;
    border-bottom: 1px solid var(--border-1);
    overflow-y: visible;
  }

  .pr-conversation__meta-col {
    overflow-y: visible;
  }
}
</style>
