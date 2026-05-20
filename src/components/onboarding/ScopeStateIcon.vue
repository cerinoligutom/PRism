<script setup lang="ts">
type ScopeRowState = "pending" | "granted" | "missing" | "unknown";

interface Props {
  state: ScopeRowState;
}

defineProps<Props>();
</script>

<template>
  <span class="scope-state" :class="`scope-state--${state}`" aria-hidden="true">
    <svg
      v-if="state === 'pending'"
      width="6"
      height="6"
      viewBox="0 0 16 16"
      fill="currentColor"
    >
      <circle cx="8" cy="8" r="4" />
    </svg>
    <svg
      v-else-if="state === 'granted'"
      width="9"
      height="9"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="2.4"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M3 8.5l3 3 7-7" />
    </svg>
    <svg
      v-else-if="state === 'missing'"
      width="9"
      height="9"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="2.4"
      stroke-linecap="round"
    >
      <path d="M4 4l8 8" />
      <path d="M12 4l-8 8" />
    </svg>
    <svg
      v-else
      width="9"
      height="9"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
    >
      <path d="M6 6a2 2 0 0 1 4 0c0 1.4-2 1.6-2 3" />
      <circle cx="8" cy="11.5" r="0.6" fill="currentColor" stroke="none" />
    </svg>
  </span>
</template>

<style scoped>
.scope-state {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
}

.scope-state--pending {
  background: var(--bg-4);
  color: var(--text-faint);
}

.scope-state--granted {
  background: var(--success-bg);
  color: var(--success);
}

.scope-state--missing {
  background: var(--danger-bg);
  color: var(--danger);
}

.scope-state--unknown {
  background: var(--warning-bg);
  color: var(--warning);
}
</style>
