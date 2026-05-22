<script setup lang="ts">
import { computed } from "vue";
import { SwitchRoot } from "reka-ui";

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

const classes = computed(() => ["toggle", { on: checked.value }]);
</script>

<template>
  <SwitchRoot
    v-model="checked"
    :disabled="disabled"
    :aria-label="ariaLabel"
    :class="classes"
  />
</template>
