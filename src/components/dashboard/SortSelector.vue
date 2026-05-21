<script setup lang="ts">
import type { DashboardSort } from "@/types/dashboard";

interface Props {
  modelValue: DashboardSort;
}

defineProps<Props>();

const emit = defineEmits<{
  "update:modelValue": [value: DashboardSort];
}>();

interface Option {
  readonly value: DashboardSort;
  readonly label: string;
}

const options: readonly Option[] = [
  { value: "updated", label: "Updated" },
  { value: "stale", label: "Stale" },
  { value: "needs-me", label: "Needs me" },
];

function select(value: DashboardSort): void {
  emit("update:modelValue", value);
}
</script>

<template>
  <div class="segmented" role="group" aria-label="Sort pull requests">
    <button
      v-for="option in options"
      :key="option.value"
      type="button"
      :class="{ active: modelValue === option.value }"
      :aria-pressed="modelValue === option.value"
      @click="select(option.value)"
    >
      {{ option.label }}
    </button>
  </div>
</template>
