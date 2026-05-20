<script setup lang="ts">
import { computed } from "vue";

type InputSize = "md" | "lg";

interface Props {
  modelValue: string;
  size?: InputSize;
  /** Renders the value in a monospaced face — used for hosts, tokens, IDs. */
  mono?: boolean;
  placeholder?: string;
  type?: "text" | "password";
  autocomplete?: string;
  disabled?: boolean;
  spellcheck?: boolean;
  id?: string;
}

const props = withDefaults(defineProps<Props>(), {
  size: "md",
  mono: false,
  type: "text",
  disabled: false,
  spellcheck: true,
});

const emit = defineEmits<{
  (event: "update:modelValue", value: string): void;
}>();

const classes = computed(() => [
  "input",
  props.size === "lg" && "input-lg",
  props.mono && "input-mono",
]);

function onInput(e: Event): void {
  const target = e.target as HTMLInputElement;
  emit("update:modelValue", target.value);
}
</script>

<template>
  <input
    :id="id"
    :class="classes"
    :value="modelValue"
    :type="type"
    :placeholder="placeholder"
    :autocomplete="autocomplete"
    :disabled="disabled"
    :spellcheck="spellcheck"
    @input="onInput"
  />
</template>
