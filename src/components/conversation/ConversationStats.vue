<script setup lang="ts">
import { computed } from "vue";

import type { ConversationStats } from "@/types/conversation";

import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import { EM_DASH, formatDurationParts, secondsSince } from "@/lib/format";

interface Props {
  stats: ConversationStats;
}

const props = defineProps<Props>();

interface TilePresentation {
  readonly value: string;
  readonly sub: string | null;
}

const oldestUnresolved = computed<TilePresentation>(() => {
  const ts = props.stats.oldest_unresolved_at;
  if (ts === null) return { value: EM_DASH, sub: null };
  const elapsed = secondsSince(ts);
  if (elapsed <= 0) return { value: "now", sub: null };
  return formatDurationParts(elapsed);
});

const avgResponse = computed<TilePresentation>(() => {
  const seconds = props.stats.avg_response_seconds;
  if (seconds === null || seconds <= 0) return { value: EM_DASH, sub: null };
  return formatDurationParts(seconds);
});

const resolutionRate = computed<TilePresentation>(() => {
  // ADR 0012: resolved / total, with outdated threads counted normally.
  if (props.stats.threads_total <= 0) return { value: EM_DASH, sub: null };
  const pct = Math.round(props.stats.resolution_rate * 100);
  return { value: String(pct), sub: "%" };
});

const totalComments = computed<TilePresentation>(() => {
  const total = props.stats.comment_breakdown.total;
  return { value: String(total), sub: null };
});

const participants = computed<TilePresentation>(() => ({
  value: String(props.stats.participants),
  sub: null,
}));

const reviewsSubmitted = computed<TilePresentation>(() => ({
  value: String(props.stats.reviews_summary.total),
  sub: null,
}));

const lastActivity = computed<TilePresentation>(() => {
  const ts = props.stats.last_activity_at;
  if (ts === null) return { value: EM_DASH, sub: null };
  const elapsed = secondsSince(ts);
  if (elapsed <= 0) return { value: "now", sub: null };
  return formatDurationParts(elapsed);
});

const breakdown = computed(() => props.stats.comment_breakdown);
const reviewsBreakdown = computed(() => props.stats.reviews_summary);
</script>

<template>
  <section class="stat-card">
    <h6 class="stat-card__title">Conversation stats</h6>
    <div class="stat-card__stack">
      <PRismTooltip
        text="Time since the earliest unresolved review thread on this PR was opened. Empty when all threads are resolved."
        :as-child="true"
      >
        <div class="stat-tile">
          <div class="stat-tile__value">
            {{ oldestUnresolved.value
            }}<span v-if="oldestUnresolved.sub !== null" class="stat-tile__sub">
              {{ oldestUnresolved.sub }}
            </span>
          </div>
          <div class="stat-tile__label">Oldest unresolved</div>
        </div>
      </PRismTooltip>

      <PRismTooltip
        text="Median time between consecutive comments inside review threads, across the PR's lifetime."
        :as-child="true"
      >
        <div class="stat-tile">
          <div class="stat-tile__value">
            {{ avgResponse.value
            }}<span v-if="avgResponse.sub !== null" class="stat-tile__sub">
              {{ avgResponse.sub }}
            </span>
          </div>
          <div class="stat-tile__label">Avg response</div>
        </div>
      </PRismTooltip>

      <PRismTooltip
        text="Percentage of review threads marked resolved, including outdated threads (ADR 0012)."
        :as-child="true"
      >
        <div class="stat-tile">
          <div class="stat-tile__value">
            {{ resolutionRate.value
            }}<span v-if="resolutionRate.sub !== null" class="stat-tile__sub">{{
              resolutionRate.sub
            }}</span>
          </div>
          <div class="stat-tile__label">Resolution rate</div>
        </div>
      </PRismTooltip>

      <PRismTooltip :as-child="true">
        <div class="stat-tile">
          <div class="stat-tile__value">
            {{ totalComments.value }}
          </div>
          <div class="stat-tile__label">Comments total</div>
        </div>
        <template #content>
          <div class="stat-tile__tooltip">
            <div class="stat-tile__tooltip-head">
              Sum of review comments, top-level issue comments, and review summary bodies.
            </div>
            <ul class="stat-tile__tooltip-list">
              <li>{{ breakdown.review }} review (per-line comments and replies)</li>
              <li>{{ breakdown.issue }} issue (top-level PR conversation)</li>
              <li>{{ breakdown.summary }} summary (review prose bodies)</li>
            </ul>
          </div>
        </template>
      </PRismTooltip>

      <PRismTooltip
        text="Distinct authors who've commented or submitted a review on this PR. Each person counts once even if they appear on multiple surfaces."
        :as-child="true"
      >
        <div class="stat-tile">
          <div class="stat-tile__value">
            {{ participants.value }}
          </div>
          <div class="stat-tile__label">Participants</div>
        </div>
      </PRismTooltip>

      <PRismTooltip :as-child="true">
        <div class="stat-tile">
          <div class="stat-tile__value">
            {{ reviewsSubmitted.value }}
          </div>
          <div class="stat-tile__label">Reviews submitted</div>
        </div>
        <template #content>
          <div class="stat-tile__tooltip">
            <div class="stat-tile__tooltip-head">
              Count of submitted reviews. Pending reviews aren't included.
            </div>
            <ul class="stat-tile__tooltip-list">
              <li>{{ reviewsBreakdown.approved }} approved</li>
              <li>{{ reviewsBreakdown.changes_requested }} changes requested</li>
              <li>{{ reviewsBreakdown.commented }} commented</li>
              <li>{{ reviewsBreakdown.dismissed }} dismissed</li>
            </ul>
          </div>
        </template>
      </PRismTooltip>

      <PRismTooltip
        text="Time since the most recent comment, reply, or submitted review on this PR."
        :as-child="true"
      >
        <div class="stat-tile">
          <div class="stat-tile__value">
            {{ lastActivity.value
            }}<span v-if="lastActivity.sub !== null" class="stat-tile__sub">
              {{ lastActivity.sub }}
            </span>
          </div>
          <div class="stat-tile__label">Last activity</div>
        </div>
      </PRismTooltip>
    </div>
  </section>
</template>

<style scoped>
.stat-card {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

.stat-card__title {
  margin: 0;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  letter-spacing: 1px;
  text-transform: uppercase;
  color: var(--text-faint);
}

.stat-card__stack {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

.stat-tile {
  background: var(--bg-3);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  padding: var(--s-3) var(--s-4);
}

.stat-tile__value {
  font-family: var(--font-mono);
  font-size: var(--fs-32);
  font-weight: 600;
  color: var(--text-strong);
  font-variant-numeric: tabular-nums;
  line-height: 1;
}

.stat-tile__sub {
  font-size: var(--fs-14);
  color: var(--text-faint);
  margin-left: var(--s-1);
}

.stat-tile__label {
  font-size: var(--fs-10);
  color: var(--text-mute);
  margin-top: var(--s-2);
  line-height: var(--lh-body);
}

.stat-tile__tooltip {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.stat-tile__tooltip-head {
  font-weight: 500;
}

.stat-tile__tooltip-list {
  margin: 0;
  padding-left: var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-1);
  color: var(--text-mute);
  list-style: disc;
}
</style>
