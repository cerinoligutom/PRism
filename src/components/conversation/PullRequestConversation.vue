<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { storeToRefs } from "pinia";

import { useConversationStore } from "@/stores/conversation";
import { useDashboardStore } from "@/stores/dashboard";

import type { ThreadsSummary } from "@/types/dashboard";
import ThreadsBar from "@/components/dashboard/ThreadsBar.vue";
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
              <span class="pr-conversation__col-title">Conversation · {{ threadsSummary }}</span>
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

.pr-conversation__col-title {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  letter-spacing: 1px;
  text-transform: uppercase;
  color: var(--text-faint);
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
