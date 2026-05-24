<script setup lang="ts">
import { computed } from "vue";

import PRismAvatar from "@/components/ui/PRismAvatar.vue";

/**
 * Reusable avatar-stack primitive. Renders a row of `PRismAvatar`s with
 * configurable overlap and an optional `+N` overflow pill.
 *
 * Layered above `PRismAvatar` (atomic) and below the consumer surfaces
 * (`ReviewerStack`, thread participants in `ThreadsList`). The component
 * owns the stacking + overlap mechanics; the consumer owns any per-avatar
 * decoration via the `avatar` slot.
 *
 * Z-order trick: each avatar (and the overflow pill) sits inside a
 * positioned `<span>` so per-item z-index applies reliably regardless of
 * how the slotted avatar renders. Same approach as the original
 * `ReviewerStack` (#140).
 */
type StackUser = { login: string; avatar_url: string | null };
type StackSize = "sm" | "md" | "lg";
type StackLayout = "overlap" | "inline";

interface Props {
  /** Users to render in left-to-right order. Leftmost stacks on top. */
  users: readonly StackUser[];
  size?: StackSize;
  /** `overlap` = Jira-style `-6px` crowd. `inline` = small horizontal gap. */
  layout?: StackLayout;
  /** When set and `users.length > max`, render the first `max - 1` avatars
   * plus a `+N` overflow pill. Undefined or 0 means no overflow - show all. */
  max?: number;
}

const props = withDefaults(defineProps<Props>(), {
  size: "md",
  layout: "overlap",
  max: undefined,
});

const hasOverflow = computed<boolean>(() => {
  if (props.max === undefined || props.max <= 0) return false;
  return props.users.length > props.max;
});

const visible = computed<readonly StackUser[]>(() => {
  if (!hasOverflow.value) return props.users;
  // Reserve the last slot for the overflow pill - render `max - 1` real avatars.
  return props.users.slice(0, Math.max(0, (props.max ?? 0) - 1));
});

const overflowCount = computed<number>(() => {
  if (!hasOverflow.value) return 0;
  return props.users.length - visible.value.length;
});

/** Total slot count including the overflow pill, drives the z-index ladder. */
const slotCount = computed<number>(
  () => visible.value.length + (hasOverflow.value ? 1 : 0),
);
</script>

<template>
  <span
    :class="[
      'prism-avatar-stack',
      `prism-avatar-stack--${layout}`,
      `prism-avatar-stack--${size}`,
    ]"
  >
    <span
      v-for="(user, index) in visible"
      :key="user.login"
      class="prism-avatar-stack__slot"
      :style="{ zIndex: slotCount - index }"
    >
      <slot name="avatar" :user="user" :index="index">
        <PRismAvatar
          :login="user.login"
          :avatar-url="user.avatar_url"
          :size="size"
          :tooltip="null"
          class="prism-avatar-stack__avatar"
        />
      </slot>
    </span>
    <span
      v-if="hasOverflow"
      class="prism-avatar-stack__slot prism-avatar-stack__slot--overflow"
    >
      <span :class="['prism-avatar-stack__overflow', `prism-avatar-stack__overflow--${size}`]">
        +{{ overflowCount }}
      </span>
    </span>
  </span>
</template>

<style scoped>
.prism-avatar-stack {
  display: inline-flex;
  align-items: center;
}

/* Each avatar (and the overflow pill) sits inside a positioned slot so the
 * per-item z-index applies reliably regardless of how the slotted avatar
 * renders. */
.prism-avatar-stack__slot {
  position: relative;
  display: inline-flex;
  align-items: center;
}

/* Overlap variant: Jira-style stacked deck. Each slot after the first
 * crowds into the previous one by `-6px`. The 1.5px border in `--bg-1` on
 * the avatar / pill paints the ring-separator between overlapping circles. */
.prism-avatar-stack--overlap .prism-avatar-stack__slot:not(:first-child) {
  margin-left: -6px;
}

/* Inline variant: small horizontal gap, no overlap. Useful for higher counts
 * where overlap reads crowded (e.g. team mention chips). */
.prism-avatar-stack--inline .prism-avatar-stack__slot:not(:first-child) {
  margin-left: 4px;
}

/* Overflow pill always sits at the back of the stack so the rightmost real
 * avatar reads as the boundary. */
.prism-avatar-stack__slot--overflow {
  z-index: 0;
}

/* The avatar inside the default slot picks up the same `--bg-1` ring as the
 * overflow pill so overlapping circles read as separated. Consumers that
 * supply the `avatar` slot are expected to manage their own ring (e.g.
 * `ReviewerStack` layers state dots and reuses the same border). */
.prism-avatar-stack__avatar {
  position: relative;
  border-width: 1.5px;
  border-color: var(--bg-1);
}

/* Overflow pill: height-locked to the corresponding avatar size so the pill
 * reads as a peer in the stack - perfect circle at single-digit counts,
 * horizontal capsule at two or three digits, never taller than its neighbours.
 * Matches the lockup from the original `ReviewerStack` (#137). */
.prism-avatar-stack__overflow {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  box-sizing: border-box;
  border-radius: var(--r-pill);
  background: var(--bg-3);
  border: 1.5px solid var(--bg-1);
  font-family: var(--font-mono);
  line-height: 1;
  color: var(--text-mute);
}

.prism-avatar-stack__overflow--sm {
  height: 16px;
  min-width: 16px;
  padding: 0 5px;
  font-size: var(--fs-9);
}

.prism-avatar-stack__overflow--md {
  height: 20px;
  min-width: 20px;
  padding: 0 6px;
  font-size: var(--fs-10);
}

.prism-avatar-stack__overflow--lg {
  height: 28px;
  min-width: 28px;
  padding: 0 8px;
  font-size: var(--fs-11);
}
</style>
