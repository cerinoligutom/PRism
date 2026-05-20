<script setup lang="ts">
import { computed } from "vue";
import type { CiSummary } from "@/types/dashboard";

type CiVariant = "passing" | "failing" | "pending";

interface Props {
  ci: CiSummary | null;
}

const props = defineProps<Props>();

/**
 * Maps the rollup state from `statusCheckRollup.state` to the three visual
 * variants in the artboard. `EXPECTED` is treated as `pending` because it
 * indicates a queued, not-yet-finished check.
 */
const variant = computed<CiVariant | null>(() => {
  if (props.ci === null) return null;
  switch (props.ci.state) {
    case "SUCCESS":
      return "passing";
    case "FAILURE":
    case "ERROR":
      return "failing";
    case "PENDING":
    case "EXPECTED":
      return "pending";
    default:
      return "pending";
  }
});
</script>

<template>
  <span v-if="ci !== null && variant !== null" :class="['ci-badge', `ci-badge--${variant}`]">
    <span class="ci-badge__ico" aria-hidden="true">
      <svg
        v-if="variant === 'passing'"
        width="10" height="10" viewBox="0 0 16 16" fill="none"
        stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"
      >
        <path d="M3 8.5l3 3 7-7" />
      </svg>
      <svg
        v-else-if="variant === 'failing'"
        width="10" height="10" viewBox="0 0 16 16" fill="none"
        stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
      >
        <path d="M4 4l8 8M12 4l-8 8" />
      </svg>
      <svg
        v-else
        width="10" height="10" viewBox="0 0 16 16" fill="none"
        stroke="currentColor" stroke-width="2"
      >
        <circle cx="8" cy="8" r="5" />
        <path d="M8 5v3l2 1.5" stroke-linecap="round" />
      </svg>
    </span>
    <span class="ci-badge__nums">{{ ci.passing }}/{{ ci.total }}</span>
  </span>
  <span v-else class="ci-badge ci-badge--empty" aria-label="No checks">—</span>
</template>

<style scoped>
.ci-badge {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  font-variant-numeric: tabular-nums;
}

.ci-badge__ico {
  width: 16px;
  height: 16px;
  border-radius: var(--r-1);
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.ci-badge--passing .ci-badge__ico { background: var(--success-bg); color: var(--success); }
.ci-badge--failing .ci-badge__ico { background: var(--danger-bg);  color: var(--danger); }
.ci-badge--pending .ci-badge__ico { background: var(--warning-bg); color: var(--warning); }

.ci-badge--passing .ci-badge__nums { color: var(--success); }
.ci-badge--failing .ci-badge__nums { color: var(--danger); }
.ci-badge--pending .ci-badge__nums { color: var(--warning); }

.ci-badge--empty {
  color: var(--text-faint);
}
</style>
