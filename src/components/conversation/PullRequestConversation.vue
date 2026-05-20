<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { storeToRefs } from "pinia";

import { useConversationStore } from "@/stores/conversation";
import { useDashboardStore } from "@/stores/dashboard";

import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import ConversationStats from "./ConversationStats.vue";
import ReviewsTab from "./ReviewsTab.vue";
import StatusTimelineTab from "./StatusTimelineTab.vue";
import ThreadsList from "./ThreadsList.vue";

interface Props {
  pullRequestId: number;
}

const props = defineProps<Props>();

type TabKey = "threads" | "reviews" | "timeline";

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
  return `${total} ${totalLabel} · ${unresolved} unresolved`;
});

type Bucket =
  | "unresolved-uninvolved"
  | "unresolved-involved"
  | "resolved-uninvolved"
  | "resolved-involved";

interface BarSegment {
  readonly bucket: Bucket;
  readonly count: number;
  readonly width: number;
  readonly tooltip: string;
}

/**
 * Four-bucket segment widths matching the dashboard threads bar (ADR 0012).
 * Buckets are derived from per-thread `(state, is_involved)`; the conversation
 * surface needs the per-thread breakdown rather than the rollup because the
 * cached stats only carry the global counts. Outdated threads sort into the
 * matching (resolved x involved) bucket - they're no longer carved out.
 */
const barSegments = computed<readonly BarSegment[]>(() => {
  const c = conversation.value;
  if (c === null || c.stats.threads_total === 0) return [];
  const counts = {
    "unresolved-uninvolved": 0,
    "unresolved-involved": 0,
    "resolved-uninvolved": 0,
    "resolved-involved": 0,
  } satisfies Record<Bucket, number>;
  for (const t of c.threads) {
    const resolved = t.state === "resolved";
    const involvedKey = t.is_involved ? "involved" : "uninvolved";
    const stateKey = resolved ? "resolved" : "unresolved";
    counts[`${stateKey}-${involvedKey}` as Bucket] += 1;
  }
  const total = c.stats.threads_total;
  const raw: { bucket: Bucket; count: number; tooltip: string }[] = (
    [
      ["unresolved-uninvolved", "Unresolved"],
      ["unresolved-involved", "Unresolved (involved)"],
      ["resolved-uninvolved", "Resolved"],
      ["resolved-involved", "Resolved (involved)"],
    ] as const
  )
    .map(([bucket, label]) => ({
      bucket,
      count: counts[bucket],
      tooltip: `${label} · ${counts[bucket]} ${counts[bucket] === 1 ? "thread" : "threads"}`,
    }))
    .filter((s) => s.count > 0);
  const SLIVER_PCT = 5;
  const remaining = Math.max(0, 100 - SLIVER_PCT * raw.length);
  return raw.map((b) => ({
    ...b,
    width: SLIVER_PCT + (b.count / total) * remaining,
  }));
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
  void loadConversation();
});

// React to host swapping the id without an unmount. The cache returns
// immediately when warm, so swapping between PRs in the same drawer is cheap.
watch(
  () => props.pullRequestId,
  () => {
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
      <section v-if="activeTab === 'threads'" class="pr-conversation__layout">
        <div class="pr-conversation__threads-col">
          <div class="pr-conversation__col-head">
            <span class="pr-conversation__col-title">Conversation · {{ threadsSummary }}</span>
          </div>

          <div v-if="conversation.stats.threads_total > 0" class="pr-conversation__rollup">
            <div class="pr-conversation__bar">
              <PRismTooltip
                v-for="segment in barSegments"
                :key="segment.bucket"
                :text="segment.tooltip"
                :as-child="true"
              >
                <div
                  :class="['pr-conversation__bar-seg', `pr-conversation__bar-seg--${segment.bucket}`]"
                  :style="{ width: `${segment.width}%` }"
                ></div>
              </PRismTooltip>
            </div>
          </div>

          <ThreadsList :threads="conversation.threads" />
        </div>

        <aside class="pr-conversation__meta-col">
          <ConversationStats :stats="conversation.stats" />
        </aside>
      </section>

      <section v-else-if="activeTab === 'reviews'" class="pr-conversation__tab-body">
        <ReviewsTab :reviews="conversation.reviews" />
      </section>

      <section v-else class="pr-conversation__tab-body">
        <StatusTimelineTab v-if="dashboardRow !== null" :pull-request="dashboardRow" />
        <div v-else class="pr-conversation__placeholder">
          Timeline unavailable until the dashboard list has loaded.
        </div>
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
  min-height: 0;
  overflow: auto;
}

.pr-conversation__layout {
  display: grid;
  grid-template-columns: 1fr 320px;
  gap: 0;
}

.pr-conversation__threads-col {
  padding: 18px 24px 20px;
  border-right: 1px solid var(--border-1);
  min-width: 0;
}

.pr-conversation__meta-col {
  padding: 18px 24px 20px;
  display: flex;
  flex-direction: column;
  gap: 18px;
  background: var(--bg-2);
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

.pr-conversation__bar {
  display: flex;
  height: 8px;
  border-radius: 4px;
  overflow: hidden;
  background: var(--bg-4);
}

.pr-conversation__bar-seg {
  height: 100%;
}

.pr-conversation__bar-seg--unresolved-uninvolved {
  background: var(--danger);
}

.pr-conversation__bar-seg--unresolved-involved {
  background: var(--warning);
}

.pr-conversation__bar-seg--resolved-uninvolved {
  background: var(--info);
}

.pr-conversation__bar-seg--resolved-involved {
  background: var(--success);
}

.pr-conversation__tab-body {
  padding: var(--s-5) var(--s-6) var(--s-6);
}

@media (max-width: 900px) {
  .pr-conversation__layout {
    grid-template-columns: 1fr;
  }

  .pr-conversation__threads-col {
    border-right: 0;
    border-bottom: 1px solid var(--border-1);
  }
}
</style>
