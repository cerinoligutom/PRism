<script setup lang="ts">
import { computed } from "vue";
import { RouterLink } from "vue-router";

type ButtonVariant = "default" | "primary" | "ghost" | "danger";
type ButtonSize = "sm" | "md" | "lg";

interface Props {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Renders as a square icon-only button. */
  icon?: boolean;
  /** Internal route — renders as `<RouterLink>`. Takes precedence over `href`. */
  to?: string;
  /** External URL — renders as `<a>`. */
  href?: string;
  /** Native button type when rendering as `<button>`. */
  type?: "button" | "submit" | "reset";
  disabled?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  variant: "default",
  size: "md",
  type: "button",
});

const classes = computed(() => [
  "btn",
  props.variant === "primary" && "btn-primary",
  props.variant === "ghost" && "btn-ghost",
  props.variant === "danger" && "btn-danger",
  props.size === "sm" && "btn-sm",
  props.size === "lg" && "btn-lg",
  props.icon && "btn-icon",
]);
</script>

<template>
  <RouterLink v-if="to" :to="to" :class="classes">
    <slot />
  </RouterLink>
  <a v-else-if="href" :href="href" :class="classes">
    <slot />
  </a>
  <button
    v-else
    :type="type"
    :disabled="disabled"
    :class="classes"
  >
    <slot />
  </button>
</template>
