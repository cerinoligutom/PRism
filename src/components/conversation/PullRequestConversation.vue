<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { storeToRefs } from "pinia";

import { useConversationStore } from "@/stores/conversation";
import { useDashboardStore } from "@/stores/dashboard";

import type { ThreadsSummary } from "@/types/dashboard";
import ThreadsBar from "@/components/dashboard/ThreadsBar.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import ConversationStats from "./ConversationStats.vue";
import IssueCommentsTab from "./IssueCommentsTab.vue";
import ReviewsTab from "./ReviewsTab.vue";
import StatusTimelineTab from "./StatusTimelineTab.vue";
import ThreadsList from "./ThreadsList.vue";

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

async function loadConversation(): Promise<void> {
  try {
    await store.load(props.pullRequestId);
  } catch {
    // Error message lands in the store; UI surfaces it via the `error` computed.
  }
}

function setTab(next: TabKey): void {
  activeTab.value = next;
}

function retry(): void {
  store.invalidate(props.pullRequestId);
  void loadConversation();
}

onMounted(() => {
  store.acquire(props.pullRequestId);
  void loadConversation();
});

onBeforeUnmount(() => {
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
                <PRismTooltip :as-child="true" side="bottom" align="start">
                  <button
                    type="button"
                    class="pr-conversation__legend-btn"
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
                    <div class="thread-state-legend">
                      <div class="thread-state-legend__section-title">State</div>
                      <ul class="thread-state-legend__rows">
                        <li class="thread-state-legend__row">
                          <span class="thread-card__state thread-card__state--unresolved-uninvolved">
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
                              <path
                                d="M2.5 4.5a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2H7l-3 2.5v-2.5H4.5a2 2 0 0 1-2-2V4.5Z"
                              />
                            </svg>
                          </span>
                          <span>Unresolved</span>
                        </li>
                        <li class="thread-state-legend__row">
                          <span class="thread-card__state thread-card__state--unresolved-involved">
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
                              <path
                                d="M2.5 4.5a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2H7l-3 2.5v-2.5H4.5a2 2 0 0 1-2-2V4.5Z"
                              />
                            </svg>
                          </span>
                          <span>Unresolved &middot; you're in it</span>
                        </li>
                        <li class="thread-state-legend__row">
                          <span class="thread-card__state thread-card__state--resolved-uninvolved">
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
                              <circle cx="8" cy="8" r="6.25" />
                              <path d="M5.25 8.25l2 2 3.5-4" />
                            </svg>
                          </span>
                          <span>Resolved</span>
                        </li>
                        <li class="thread-state-legend__row">
                          <span class="thread-card__state thread-card__state--resolved-involved">
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
                              <circle cx="8" cy="8" r="6.25" />
                              <path d="M5.25 8.25l2 2 3.5-4" />
                            </svg>
                          </span>
                          <span>Resolved &middot; was yours</span>
                        </li>
                      </ul>
                      <div class="thread-state-legend__section-title">Modifier</div>
                      <ul class="thread-state-legend__rows">
                        <li class="thread-state-legend__row">
                          <span class="thread-card__chip thread-card__chip--mine">INVOLVED</span>
                          <span>You're a participant</span>
                        </li>
                        <li class="thread-state-legend__row">
                          <span class="thread-card__chip thread-card__chip--outdated">OUTDATED</span>
                          <span>Line no longer exists</span>
                        </li>
                      </ul>
                    </div>
                  </template>
                </PRismTooltip>
              </div>
            </div>

            <div v-if="conversation.stats.threads_total > 0" class="pr-conversation__rollup">
              <ThreadsBar :threads="threadsSummaryForBar" />
            </div>

            <ThreadsList
              :threads="conversation.threads"
              :thread-comments="conversation.thread_comments"
            />
          </template>

          <ReviewsTab
            v-else-if="activeTab === 'reviews'"
            :reviews="conversation.reviews"
          />

          <IssueCommentsTab
            v-else-if="activeTab === 'comments'"
            :issue-comments="conversation.issue_comments"
          />

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

.pr-conversation__legend-btn {
  background: transparent;
  border: 0;
  padding: 2px;
  border-radius: var(--r-1);
  color: var(--text-faint);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  line-height: 0;
}

.pr-conversation__legend-btn:hover {
  color: var(--text);
  background: var(--bg-3);
}

.pr-conversation__legend-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
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

<!--
  Unscoped because Reka's `TooltipPortal` teleports the legend content out of
  the scoped-CSS attribute boundary (same constraint as the participants /
  state-badge tooltips in ThreadsList). The badge / chip swatches inside the
  legend share the unscoped classes declared at the bottom of `ThreadsList.vue`.
-->
<style>
.thread-state-legend {
  display: flex;
  flex-direction: column;
  gap: 10px;
  font-size: var(--fs-12);
  color: var(--text);
  min-width: 220px;
}

.thread-state-legend__section-title {
  font-family: var(--font-mono);
  font-size: var(--fs-9);
  letter-spacing: 1px;
  text-transform: uppercase;
  color: var(--text-faint);
}

.thread-state-legend__rows {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.thread-state-legend__row {
  display: flex;
  align-items: center;
  gap: 8px;
}
</style>
