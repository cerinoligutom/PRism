<script setup lang="ts">
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import type {
  DashboardSort,
  DashboardSortDirection,
} from "@/stores/dashboard";

interface Props {
  modelValue: DashboardSort;
  direction: DashboardSortDirection;
}

defineProps<Props>();

const emit = defineEmits<{
  "update:modelValue": [value: DashboardSort];
  "update:direction": [value: DashboardSortDirection];
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
  <div class="sort-selector">
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
    <PRismTooltip
      :text="direction === 'desc' ? 'Ascending' : 'Descending'"
      :as-child="true"
    >
      <button
        type="button"
        class="btn btn-icon sort-selector__dir"
        :aria-label="
          direction === 'desc'
            ? 'Switch to ascending order'
            : 'Switch to descending order'
        "
        :aria-pressed="direction === 'asc'"
        @click="
          emit(
            'update:direction',
            direction === 'desc' ? 'asc' : 'desc',
          )
        "
      >
        <svg
          v-if="direction === 'desc'"
          width="12"
          height="12"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M3 4h8" />
          <path d="M3 8h6" />
          <path d="M3 12h4" />
          <path d="M13 5v8" />
          <path d="M10.5 10.5L13 13l2.5-2.5" />
        </svg>
        <svg
          v-else
          width="12"
          height="12"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M3 12h8" />
          <path d="M3 8h6" />
          <path d="M3 4h4" />
          <path d="M13 11V3" />
          <path d="M10.5 5.5L13 3l2.5 2.5" />
        </svg>
      </button>
    </PRismTooltip>
  </div>
</template>

<style scoped>
.sort-selector {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.sort-selector__dir {
  width: 24px;
  height: 22px;
}
</style>
