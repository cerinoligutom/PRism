<script setup lang="ts">
type ScopeRowState = "pending" | "granted" | "missing" | "unknown";

interface Props {
  state: ScopeRowState;
  /**
   * Label to render in the pending / granted states (e.g. "Read-only").
   * When omitted (classic scopes) the tag is only rendered for non-pending
   * states — pending classic rows would otherwise show an empty tag.
   */
  defaultLabel?: string;
}

defineProps<Props>();
</script>

<template>
  <span
    v-if="state !== 'pending' || defaultLabel"
    class="scope-tag"
    :class="`scope-tag--${state}`"
  >
    <template v-if="state === 'granted'">{{ defaultLabel ?? "Granted" }}</template>
    <template v-else-if="state === 'missing'">Missing</template>
    <template v-else-if="state === 'unknown'">Unverified</template>
    <template v-else>{{ defaultLabel }}</template>
  </span>
</template>

<style scoped>
.scope-tag {
  font-family: var(--font-mono);
  font-size: var(--fs-9);
  text-transform: uppercase;
  letter-spacing: 0.6px;
  padding: 1px 6px;
  border-radius: var(--r-1);
}

.scope-tag--pending {
  color: var(--text-faint);
  background: var(--bg-4);
}

.scope-tag--granted {
  color: var(--success);
  background: var(--success-bg);
}

.scope-tag--missing {
  color: var(--danger);
  background: var(--danger-bg);
}

.scope-tag--unknown {
  color: var(--warning);
  background: var(--warning-bg);
}
</style>
