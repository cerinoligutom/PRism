<script setup lang="ts">
import { computed } from "vue";

type CalloutVariant = "accent" | "info" | "warning" | "danger";

interface Props {
  variant?: CalloutVariant;
}

const props = withDefaults(defineProps<Props>(), {
  variant: "accent",
});

const classes = computed(() => ["callout", `callout--${props.variant}`]);
</script>

<template>
  <div :class="classes">
    <span v-if="$slots.icon" class="callout__icon" aria-hidden="true">
      <slot name="icon" />
    </span>
    <div class="callout__body">
      <slot />
    </div>
  </div>
</template>

<style scoped>
.callout {
  display: flex;
  gap: 10px;
  padding: 12px 14px;
  border-radius: var(--r-3);
  border: 1px solid transparent;
  font-size: var(--fs-12);
  line-height: var(--lh-body);
}

.callout__icon {
  display: inline-flex;
  flex: 0 0 16px;
  margin-top: 1px;
}

.callout__body {
  flex: 1;
}

.callout__body :deep(strong) {
  color: var(--text-strong);
  font-weight: 600;
}

.callout--accent {
  background: var(--accent-bg);
  border-color: oklch(0.4 0.12 var(--accent-h) / 0.4);
  color: var(--accent-strong);
}

.callout--accent .callout__icon {
  color: var(--accent);
}

.callout--info {
  background: var(--info-bg);
  border-color: oklch(0.32 0.1 240 / 0.3);
  color: var(--info);
}

.callout--info .callout__icon {
  color: var(--info);
}

.callout--warning {
  background: var(--warning-bg);
  border-color: oklch(0.4 0.1 80 / 0.4);
  color: var(--warning);
}

.callout--warning .callout__icon {
  color: var(--warning);
}

.callout--danger {
  background: var(--danger-bg);
  border-color: oklch(0.4 0.12 25 / 0.4);
  color: var(--danger);
}

.callout--danger .callout__icon {
  color: var(--danger);
}
</style>
