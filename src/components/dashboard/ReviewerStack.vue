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

const totalLabel = computed<string>(
  () =>
    `${props.reviewers.length} total ${
      props.reviewers.length === 1 ? "reviewer" : "reviewers"
    }`,
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
        <PRismAvatar
          v-for="reviewer in visible"
          :key="reviewer.login"
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
        <span v-if="overflow > 0" class="reviewer-stack__overflow">+{{ overflow }}</span>
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
    <span class="reviewer-stack__summary">
      <PRismTooltip :text="`${changesCount} requested changes`" :as-child="true">
        <span class="reviewer-stack__summary-changes">{{ changesCount }}</span>
      </PRismTooltip>
      <span aria-hidden="true">/</span>
      <PRismTooltip :text="`${approvedCount} approved`" :as-child="true">
        <span class="reviewer-stack__summary-ok">{{ approvedCount }}</span>
      </PRismTooltip>
      <span aria-hidden="true">/</span>
      <PRismTooltip :text="totalLabel" :as-child="true">
        <span class="reviewer-stack__summary-total">{{ reviewers.length }}</span>
      </PRismTooltip>
    </span>
  </span>
</template>

<style scoped>
.reviewer-stack {
  display: inline-flex;
  align-items: center;
}

.reviewer-stack__avatars {
  display: inline-flex;
  align-items: center;
  gap: 4px;
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

.reviewer-stack__overflow {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 20px;
  height: 16px;
  padding: 0 5px;
  border-radius: var(--r-pill);
  background: var(--bg-3);
  border: 1px solid var(--border-1);
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-mute);
}

.reviewer-stack__summary {
  margin-left: 6px;
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-mute);
  font-variant-numeric: tabular-nums;
}

.reviewer-stack__summary-ok { color: var(--success); }
.reviewer-stack__summary-changes { color: var(--danger); }
.reviewer-stack__summary-total { color: var(--text-faint); }
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
</style>
