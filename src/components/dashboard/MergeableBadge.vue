<script setup lang="ts">
import { computed } from "vue";
import PRismBadge from "@/components/ui/PRismBadge.vue";

type ResolvedBadge = "DRAFT" | "CONFLICTS" | "MERGEABLE" | null;

interface Props {
  /** GraphQL `mergeable`: `"MERGEABLE" | "CONFLICTING" | "UNKNOWN"`. */
  state: string | null;
  /** GraphQL `reviewDecision`: `"APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED"`. */
  reviewDecision: string | null;
  isDraft: boolean;
}

const props = defineProps<Props>();

/**
 * Priority order from `docs/contracts/dashboard-data.md`:
 *   isDraft > state === "CONFLICTING" > mergeable + approved > nothing.
 */
const resolved = computed<ResolvedBadge>(() => {
  if (props.isDraft) return "DRAFT";
  if (props.state === "CONFLICTING") return "CONFLICTS";
  if (props.state === "MERGEABLE" && props.reviewDecision === "APPROVED") {
    return "MERGEABLE";
  }
  return null;
});

const variant = computed<"draft" | "danger" | "success">(() => {
  switch (resolved.value) {
    case "DRAFT":
      return "draft";
    case "CONFLICTS":
      return "danger";
    case "MERGEABLE":
    default:
      return "success";
  }
});
</script>

<template>
  <PRismBadge v-if="resolved !== null" :variant="variant" class="mergeable-badge">
    {{ resolved }}
  </PRismBadge>
</template>

<style scoped>
.mergeable-badge {
  height: 14px;
  font-size: var(--fs-9);
  padding: 0 5px;
}
</style>
