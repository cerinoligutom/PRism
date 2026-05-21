<script setup lang="ts">
import { computed } from "vue";
import type { ReviewerEntry, ReviewerState } from "@/types/dashboard";
import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

interface Props {
  reviewers: readonly ReviewerEntry[];
  /** Overflow into a "+N" pill once the reviewer count exceeds this. */
  max?: number;
}

const props = withDefaults(defineProps<Props>(), {
  max: 4,
});

/** Cap on rows rendered inside the overflow tooltip; remainder collapses
 * into a muted "+M more" footer so the chip stays readable for huge PRs. */
const OVERFLOW_TOOLTIP_CAP = 12;

const visible = computed<readonly ReviewerEntry[]>(() =>
  props.reviewers.slice(0, props.max),
);

const overflow = computed<number>(
  () => Math.max(0, props.reviewers.length - props.max),
);

const tooltipReviewers = computed<readonly ReviewerEntry[]>(() =>
  props.reviewers.slice(0, OVERFLOW_TOOLTIP_CAP),
);

const moreCount = computed<number>(
  () => props.reviewers.length - tooltipReviewers.value.length,
);

const approvedCount = computed<number>(
  () => props.reviewers.filter((r) => r.state === "approved").length,
);

const changesCount = computed<number>(
  () => props.reviewers.filter((r) => r.state === "changes-requested").length,
);

const commentedCount = computed<number>(
  () => props.reviewers.filter((r) => r.state === "commented").length,
);

const pendingCount = computed<number>(
  () => props.reviewers.filter((r) => r.state === "pending").length,
);

interface SummaryRow {
  readonly key: ReviewerState;
  readonly label: string;
  readonly count: number;
}

const summaryRows = computed<readonly SummaryRow[]>(() =>
  (
    [
      { key: "approved", label: "Approved", count: approvedCount.value },
      { key: "changes-requested", label: "Changes requested", count: changesCount.value },
      { key: "commented", label: "Commented", count: commentedCount.value },
      { key: "pending", label: "Pending", count: pendingCount.value },
    ] as const
  ).filter((row) => row.count > 0),
);

function stateClass(state: ReviewerState): string {
  switch (state) {
    case "approved":
      return "reviewer-stack__avatar--approved";
    case "changes-requested":
      return "reviewer-stack__avatar--changes";
    case "commented":
      return "reviewer-stack__avatar--commented";
    case "pending":
    default:
      return "reviewer-stack__avatar--pending";
  }
}

function statusLabel(state: ReviewerState): string {
  switch (state) {
    case "approved":
      return "Approved";
    case "changes-requested":
      return "Changes";
    case "commented":
      return "Commented";
    case "pending":
    default:
      return "Pending";
  }
}
</script>

<template>
  <span v-if="reviewers.length === 0" class="reviewer-stack reviewer-stack--empty">
    no reviewers
  </span>
  <span v-else class="reviewer-stack">
    <PRismTooltip :as-child="true">
      <span class="reviewer-stack__avatars">
        <span
          v-for="(reviewer, index) in visible"
          :key="reviewer.login"
          class="reviewer-stack__avatar-slot"
          :style="{ zIndex: visible.length - index }"
        >
          <PRismAvatar
            :login="reviewer.login"
            :avatar-url="reviewer.avatar_url"
            size="sm"
            :title="null"
            :class="[
              'reviewer-stack__avatar',
              stateClass(reviewer.state),
              reviewer.is_you && 'reviewer-stack__avatar--you',
            ]"
          />
        </span>
        <span v-if="overflow > 0" class="reviewer-stack__overflow-slot">
          <span class="reviewer-stack__overflow">+{{ overflow }}</span>
        </span>
      </span>
      <template #content>
        <ul class="reviewer-stack__tooltip-list" style="max-width: 360px">
          <li
            v-for="reviewer in tooltipReviewers"
            :key="reviewer.login"
            class="reviewer-stack__tooltip-row"
          >
            <PRismAvatar
              :login="reviewer.login"
              :avatar-url="reviewer.avatar_url"
              size="sm"
              :title="null"
            />
            <span class="reviewer-stack__tooltip-login">{{ reviewer.login }}</span>
            <span
              :class="[
                'reviewer-stack__tooltip-status',
                `reviewer-stack__tooltip-status--${reviewer.state}`,
              ]"
            >
              {{ statusLabel(reviewer.state) }}
            </span>
          </li>
          <li v-if="moreCount > 0" class="reviewer-stack__tooltip-footer">
            +{{ moreCount }} more {{ moreCount === 1 ? "reviewer" : "reviewers" }} - open PR for full list
          </li>
        </ul>
      </template>
    </PRismTooltip>
    <PRismTooltip :as-child="true">
      <span class="reviewer-stack__summary">
        <span class="reviewer-stack__summary-changes">{{ changesCount }}</span>
        <span aria-hidden="true">/</span>
        <span class="reviewer-stack__summary-ok">{{ approvedCount }}</span>
        <span aria-hidden="true">/</span>
        <span class="reviewer-stack__summary-total">{{ reviewers.length }}</span>
      </span>
      <template #content>
        <div class="reviewer-stack__breakdown">
          <ul
            v-if="summaryRows.length > 0"
            class="reviewer-stack__breakdown-list"
          >
            <li
              v-for="row in summaryRows"
              :key="row.key"
              :class="[
                'reviewer-stack__breakdown-row',
                `reviewer-stack__breakdown-row--${row.key}`,
              ]"
            >
              <span
                :class="[
                  'reviewer-stack__breakdown-dot',
                  `reviewer-stack__breakdown-dot--${row.key}`,
                ]"
                aria-hidden="true"
              ></span>
              <span class="reviewer-stack__breakdown-count">{{ row.count }}</span>
              <span class="reviewer-stack__breakdown-label">{{ row.label }}</span>
            </li>
          </ul>
          <div class="reviewer-stack__breakdown-total">
            <span class="reviewer-stack__breakdown-count">{{ reviewers.length }}</span>
            <span class="reviewer-stack__breakdown-label">Total</span>
          </div>
        </div>
      </template>
    </PRismTooltip>
  </span>
</template>

<style scoped>
.reviewer-stack {
  display: grid;
  grid-template-columns: 1fr auto;
  align-items: center;
  width: 100%;
  column-gap: 6px;
}

.reviewer-stack__avatars {
  display: inline-flex;
  align-items: center;
}

/* Each avatar (and the overflow pill) sits inside a positioned slot so the
 * per-item z-index actually applies. PRismAvatar has a multi-root template,
 * so inline `:style` passed to it doesn't fall through to the rendered DOM -
 * the slot wrapper is what gives us reliable stack-order control. */
.reviewer-stack__avatar-slot,
.reviewer-stack__overflow-slot {
  position: relative;
  display: inline-flex;
  align-items: center;
}

/* Jira-style stacked deck: each slot after the first crowds into the previous
 * one. The 1.5px border in --bg-1 on the avatar/pill paints the ring-separator. */
.reviewer-stack__avatar-slot:not(:first-child),
.reviewer-stack__overflow-slot {
  margin-left: -6px;
}

.reviewer-stack__overflow-slot {
  z-index: 0;
}

.reviewer-stack--empty {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
}

.reviewer-stack__avatar {
  position: relative;
  border-width: 1.5px;
  border-color: var(--bg-1);
}

/* Reviewer state dot anchored to the bottom-right of each avatar. */
.reviewer-stack__avatar::after {
  content: "";
  position: absolute;
  width: 8px;
  height: 8px;
  bottom: -2px;
  right: -2px;
  border-radius: 50%;
  border: 1.5px solid var(--bg-1);
  background: var(--bg-4);
}

.reviewer-stack__avatar--approved::after { background: var(--success); }
.reviewer-stack__avatar--changes::after  { background: var(--danger); }
.reviewer-stack__avatar--commented::after { background: var(--info); }
.reviewer-stack__avatar--pending::after  { background: var(--text-faint); }

.reviewer-stack__avatar--you {
  box-shadow: 0 0 0 2px var(--accent);
}

/* Locked to the `sm` avatar size (16px) so the pill reads as a peer in the
 * stack: perfect circle at single-digit counts, horizontal capsule at two or
 * three digits, never taller than the avatars it sits beside. Stack-order
 * and overlap are handled by `.reviewer-stack__overflow-slot`. */
.reviewer-stack__overflow {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  box-sizing: border-box;
  height: 16px;
  min-width: 16px;
  padding: 0 5px;
  border-radius: var(--r-pill);
  background: var(--bg-3);
  border: 1.5px solid var(--bg-1);
  font-family: var(--font-mono);
  font-size: var(--fs-9);
  line-height: 1;
  color: var(--text-mute);
}

.reviewer-stack__summary {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-mute);
  font-variant-numeric: tabular-nums;
}

.reviewer-stack__summary-ok { color: var(--success); }
.reviewer-stack__summary-changes { color: var(--danger); }
.reviewer-stack__summary-total { color: var(--text-faint); }

.reviewer-stack__summary-changes,
.reviewer-stack__summary-ok,
.reviewer-stack__summary-total {
  display: inline-block;
  min-width: 1.4em;
  text-align: center;
}
</style>

<!--
  Tooltip-list styles are global (not scoped) because Reka's TooltipPortal
  teleports the rendered slot content to `document.body`, and Vue's scoped
  `data-v-*` selectors don't follow it across the portal. Matches the same
  pattern used by `PRismTooltip` itself.
-->
<style>
.reviewer-stack__tooltip-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.reviewer-stack__tooltip-row {
  display: grid;
  grid-template-columns: auto 1fr auto;
  align-items: center;
  gap: 8px;
  font-size: var(--fs-11);
  color: var(--text);
}

.reviewer-stack__tooltip-login {
  font-family: var(--font-mono);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.reviewer-stack__tooltip-status {
  display: inline-flex;
  align-items: center;
  height: 16px;
  padding: 0 6px;
  border-radius: var(--r-1);
  font-size: var(--fs-9);
  font-weight: 500;
  font-family: var(--font-mono);
  letter-spacing: 0.2px;
  flex: 0 0 auto;
}

.reviewer-stack__tooltip-status--approved {
  background: var(--success-bg);
  color: var(--success);
}

.reviewer-stack__tooltip-status--changes-requested {
  background: var(--danger-bg);
  color: var(--danger);
}

.reviewer-stack__tooltip-status--commented {
  background: var(--info-bg);
  color: var(--info);
}

.reviewer-stack__tooltip-status--pending {
  background: var(--bg-4);
  color: var(--text-faint);
}

.reviewer-stack__tooltip-footer {
  margin-top: 4px;
  padding-top: 6px;
  border-top: 1px solid var(--border-1);
  font-size: var(--fs-10);
  color: var(--text-faint);
  font-style: italic;
}

/* Summary breakdown: coloured dot + count + label per state, with a divider
 * above the always-rendered total row. Same pattern as `ThreadsBar`. */
.reviewer-stack__breakdown {
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 180px;
}

.reviewer-stack__breakdown-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.reviewer-stack__breakdown-row {
  display: grid;
  grid-template-columns: auto auto 1fr;
  align-items: center;
  gap: 8px;
  font-size: var(--fs-11);
}

.reviewer-stack__breakdown-row--approved { color: var(--success); }
.reviewer-stack__breakdown-row--changes-requested { color: var(--danger); }
.reviewer-stack__breakdown-row--commented { color: var(--info); }
.reviewer-stack__breakdown-row--pending { color: var(--text-faint); }

.reviewer-stack__breakdown-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex: 0 0 auto;
  background: currentColor;
}

.reviewer-stack__breakdown-count {
  font-family: var(--font-mono);
  font-variant-numeric: tabular-nums;
}

.reviewer-stack__breakdown-label {
  color: inherit;
  opacity: 0.85;
}

.reviewer-stack__breakdown-total {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 2px;
  padding-top: 6px;
  padding-left: 16px;
  border-top: 1px solid var(--border-1);
  font-size: var(--fs-11);
  color: var(--text);
}
</style>
