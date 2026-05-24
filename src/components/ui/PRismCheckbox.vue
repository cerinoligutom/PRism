<script setup lang="ts">
import { computed } from "vue";
import { CheckboxIndicator, CheckboxRoot } from "reka-ui";

interface Props {
  /** Two-way bound checked state. */
  modelValue: boolean;
  disabled?: boolean;
  /** Accessible label for screen readers when no visible `<label for>` exists. */
  ariaLabel?: string;
}

const props = withDefaults(defineProps<Props>(), {
  disabled: false,
});

const emit = defineEmits<{
  (e: "update:modelValue", value: boolean): void;
}>();

const checked = computed<boolean>({
  get: () => props.modelValue,
  set: (value) => emit("update:modelValue", value),
});
</script>

<template>
  <CheckboxRoot
    v-model="checked"
    :disabled="disabled"
    :aria-label="ariaLabel"
    class="prism-checkbox"
  >
    <CheckboxIndicator class="prism-checkbox__indicator">
      <svg
        viewBox="0 0 16 16"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M3.5 8.5l3 3 6-6.5" />
      </svg>
    </CheckboxIndicator>
  </CheckboxRoot>
</template>

<style scoped>
.prism-checkbox {
  width: 16px;
  height: 16px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--r-1);
  border: 1.5px solid var(--border-2);
  background: var(--bg-1);
  padding: 0;
  cursor: pointer;
  transition:
    background 0.12s,
    border-color 0.12s,
    color 0.12s;
}

.prism-checkbox:hover:not(:disabled) {
  border-color: var(--text-faint);
}

.prism-checkbox:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: 2px;
}

.prism-checkbox[data-state="checked"] {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--accent-fg);
}

.prism-checkbox[data-state="checked"]:hover:not(:disabled) {
  background: var(--accent-strong);
  border-color: var(--accent-strong);
}

.prism-checkbox:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

.prism-checkbox__indicator {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
}

.prism-checkbox__indicator svg {
  width: 12px;
  height: 12px;
}
</style>
