<script setup lang="ts">
import type { DashboardGroup } from "@/types/dashboard";

interface Props {
  modelValue: DashboardGroup;
}

defineProps<Props>();

const emit = defineEmits<{
  "update:modelValue": [value: DashboardGroup];
}>();

interface Option {
  readonly value: DashboardGroup;
  readonly label: string;
}

const options: readonly Option[] = [
  { value: "repo", label: "Repo" },
  { value: "org", label: "Org" },
  { value: "none", label: "None" },
];

function select(value: DashboardGroup): void {
  emit("update:modelValue", value);
}
</script>

<template>
  <div class="segmented" role="group" aria-label="Group rows by">
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
