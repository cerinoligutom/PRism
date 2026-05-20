<script setup lang="ts">
import { computed } from "vue";

import type { ConversationStats } from "@/types/conversation";

import { EM_DASH, formatDurationParts, secondsSince } from "@/lib/format";

interface Props {
  stats: ConversationStats;
}

const props = defineProps<Props>();

const oldestUnresolved = computed<{ value: string; sub: string | null }>(() => {
  const ts = props.stats.oldest_unresolved_at;
  if (ts === null) return { value: EM_DASH, sub: null };
  const elapsed = secondsSince(ts);
  if (elapsed <= 0) return { value: "now", sub: null };
  return formatDurationParts(elapsed);
});

const avgResponse = computed<{ value: string; sub: string | null }>(() => {
  const seconds = props.stats.avg_response_seconds;
  if (seconds === null || seconds <= 0) return { value: EM_DASH, sub: null };
  return formatDurationParts(seconds);
});

const resolutionRate = computed<{ value: string; sub: string | null }>(() => {
  const ratable = props.stats.threads_total - props.stats.threads_outdated;
  if (ratable <= 0) return { value: EM_DASH, sub: null };
  const pct = Math.round(props.stats.resolution_rate * 100);
  return { value: String(pct), sub: "%" };
});

const totalComments = computed<{ value: string; sub: string | null }>(() => {
  const total = props.stats.comment_breakdown.total;
  return { value: String(total), sub: null };
});

const breakdownLabel = computed<string>(() => {
  const b = props.stats.comment_breakdown;
  return `${b.review} review · ${b.issue} issue · ${b.summary} summary`;
});
</script>

<template>
  <section class="stat-card">
    <h6 class="stat-card__title">Conversation stats</h6>
    <div class="stat-card__grid">
      <div class="stat-tile">
        <div class="stat-tile__value">
          {{ oldestUnresolved.value
          }}<span v-if="oldestUnresolved.sub !== null" class="stat-tile__sub">
            {{ oldestUnresolved.sub }}
          </span>
        </div>
        <div class="stat-tile__label">Oldest unresolved</div>
      </div>

      <div class="stat-tile">
        <div class="stat-tile__value">
          {{ avgResponse.value
          }}<span v-if="avgResponse.sub !== null" class="stat-tile__sub">
            {{ avgResponse.sub }}
          </span>
        </div>
        <div class="stat-tile__label">Avg response</div>
      </div>

      <div class="stat-tile">
        <div class="stat-tile__value">
          {{ resolutionRate.value
          }}<span v-if="resolutionRate.sub !== null" class="stat-tile__sub">{{
            resolutionRate.sub
          }}</span>
        </div>
        <div class="stat-tile__label">Resolution rate</div>
      </div>

      <div class="stat-tile">
        <div class="stat-tile__value">
          {{ totalComments.value }}
        </div>
        <div class="stat-tile__label">
          Comments total
          <span class="stat-tile__breakdown">{{ breakdownLabel }}</span>
        </div>
      </div>
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

.stat-card__grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--s-3);
}

.stat-tile {
  background: var(--bg-3);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  padding: 10px var(--s-3);
}

.stat-tile__value {
  font-family: var(--font-mono);
  font-size: 18px;
  font-weight: 600;
  color: var(--text-strong);
  font-variant-numeric: tabular-nums;
  line-height: 1;
}

.stat-tile__sub {
  font-size: var(--fs-11);
  color: var(--text-faint);
  margin-left: 2px;
}

.stat-tile__label {
  font-size: var(--fs-10);
  color: var(--text-mute);
  margin-top: 6px;
  line-height: var(--lh-body);
}

.stat-tile__breakdown {
  display: block;
  color: var(--text-faint);
}
</style>
