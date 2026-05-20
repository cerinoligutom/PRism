<script setup lang="ts">
import { computed } from "vue";

type BadgeVariant =
  | "default"
  | "success"
  | "danger"
  | "warning"
  | "info"
  | "merged"
  | "draft"
  | "accent";

interface Props {
  variant?: BadgeVariant;
}

const props = withDefaults(defineProps<Props>(), {
  variant: "default",
});

const classes = computed(() => [
  "badge",
  props.variant === "success" && "badge-success",
  props.variant === "danger" && "badge-danger",
  props.variant === "warning" && "badge-warning",
  props.variant === "info" && "badge-info",
  props.variant === "merged" && "badge-merged",
  props.variant === "draft" && "badge-draft",
  props.variant === "accent" && "badge-accent",
]);
</script>

<template>
  <span :class="classes">
    <slot />
  </span>
</template>
