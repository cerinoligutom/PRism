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

const visible = computed<readonly ReviewerEntry[]>(() =>
  props.reviewers.slice(0, props.max),
);

const hidden = computed<readonly ReviewerEntry[]>(() =>
  props.reviewers.slice(props.max),
);

const overflow = computed<number>(() => hidden.value.length);

const overflowTooltip = computed<string>(() =>
  hidden.value.map((r) => titleFor(r)).join("\n"),
);

const approvedCount = computed<number>(
  () => props.reviewers.filter((r) => r.state === "approved").length,
);

const changesCount = computed<number>(
  () => props.reviewers.filter((r) => r.state === "changes-requested").length,
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

function titleFor(reviewer: ReviewerEntry): string {
  switch (reviewer.state) {
    case "approved":
      return `${reviewer.login} approved`;
    case "changes-requested":
      return `${reviewer.login} requested changes`;
    case "commented":
      return `${reviewer.login} commented`;
    case "pending":
    default:
      return `${reviewer.login} pending`;
  }
}
</script>

<template>
  <span v-if="reviewers.length === 0" class="reviewer-stack reviewer-stack--empty">
    no reviewers
  </span>
  <span v-else class="reviewer-stack">
    <PRismTooltip
      v-for="reviewer in visible"
      :key="reviewer.login"
      :text="titleFor(reviewer)"
      :as-child="true"
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
    </PRismTooltip>
    <PRismTooltip
      v-if="overflow > 0"
      :text="overflowTooltip"
      :as-child="true"
    >
      <span class="reviewer-stack__overflow">+{{ overflow }}</span>
    </PRismTooltip>
    <span class="reviewer-stack__summary">
      <span v-if="changesCount > 0" class="reviewer-stack__summary-changes">{{ changesCount }}</span>
      <span v-else-if="approvedCount > 0" class="reviewer-stack__summary-ok">{{ approvedCount }}</span>
      <span v-else>0</span><span class="reviewer-stack__summary-total">/{{ reviewers.length }}</span>
    </span>
  </span>
</template>

<style scoped>
.reviewer-stack {
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
