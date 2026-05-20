<script setup lang="ts">
import type { RowDensity } from "@/types/dashboard";

interface Props {
  modelValue: RowDensity;
}

defineProps<Props>();

const emit = defineEmits<{
  "update:modelValue": [value: RowDensity];
}>();

interface Option {
  readonly value: RowDensity;
  readonly label: string;
  readonly title: string;
}

const options: readonly Option[] = [
  { value: "comfortable", label: "Comfortable", title: "Comfortable" },
  { value: "tight", label: "Tight", title: "Tight" },
  { value: "roomy", label: "Roomy", title: "Roomy" },
];

function select(value: RowDensity): void {
  emit("update:modelValue", value);
}
</script>

<template>
  <div class="segmented" role="group" aria-label="Row density">
    <button
      v-for="option in options"
      :key="option.value"
      type="button"
      :class="{ active: modelValue === option.value }"
      :title="option.title"
      :aria-pressed="modelValue === option.value"
      @click="select(option.value)"
    >
      <span class="ico" aria-hidden="true">
        <svg
          v-if="option.value === 'tight'"
          width="12" height="12" viewBox="0 0 16 16" fill="none"
          stroke="currentColor" stroke-width="1.5"
        >
          <path d="M2 4h12M2 7h12M2 10h12M2 13h12" />
        </svg>
        <svg
          v-else-if="option.value === 'comfortable'"
          width="12" height="12" viewBox="0 0 16 16" fill="none"
          stroke="currentColor" stroke-width="1.5"
        >
          <path d="M2 5h12M2 9h12M2 13h12" />
        </svg>
        <svg
          v-else
          width="12" height="12" viewBox="0 0 16 16" fill="none"
          stroke="currentColor" stroke-width="1.5"
        >
          <path d="M2 4h12M2 12h12" />
        </svg>
      </span>
      <span>{{ option.label }}</span>
    </button>
  </div>
</template>
