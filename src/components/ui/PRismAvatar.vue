<script setup lang="ts">
import { computed, ref, watch } from "vue";

import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import { avatarSeed, initials } from "@/lib/format";

/**
 * GitHub avatar primitive. Renders the cached `avatarUrl` when present; falls
 * back to the initials-in-coloured-circle pattern on `null` URL or `<img>`
 * onerror (e.g. a stale URL, offline, a deleted account). Single source of
 * truth for avatar rendering across the dashboard and conversation surface.
 *
 * Sizes mirror the `.avatar` primitive in `primitives.css`:
 *  - `sm` = 16px (dashboard row inline).
 *  - `md` = 20px (default; reviewer stack, thread snippet).
 *  - `lg` = 28px (drawer header, review card).
 */
type AvatarSize = "sm" | "md" | "lg";

interface Props {
  /** GitHub login. Drives the initials + seed fallback. */
  login: string;
  /** Cached avatar URL from the local `users` table. */
  avatarUrl?: string | null;
  size?: AvatarSize;
  /**
   * Optional tooltip text. Defaults to `login`. Pass `null` to suppress the
   * internal `PRismTooltip` entirely - e.g. when a caller wraps the avatar
   * in its own outer `PRismTooltip` and doesn't want a nested chip.
   */
  title?: string | null;
}

const props = withDefaults(defineProps<Props>(), {
  avatarUrl: null,
  size: "md",
  title: undefined,
});

// Track whether the <img> has failed to load; reset whenever the URL changes
// so a recovered avatar URL (next sync cycle) re-renders the image.
const imgFailed = ref(false);
watch(
  () => props.avatarUrl,
  () => {
    imgFailed.value = false;
  },
);

const showImage = computed<boolean>(
  () => typeof props.avatarUrl === "string" && props.avatarUrl.length > 0 && !imgFailed.value,
);

const fallbackInitials = computed<string>(() => initials(props.login));
const fallbackSeed = computed<string>(() => avatarSeed(props.login));
const tooltip = computed<string | null>(() => {
  // Explicit null = caller wants no internal tooltip (typically because a
  // PRismTooltip is wrapping the avatar). Undefined falls back to login.
  if (props.title === null) return null;
  return props.title ?? props.login;
});
const showTooltip = computed<boolean>(
  () => tooltip.value !== null && tooltip.value !== "",
);

const sizeClass = computed<string | null>(() => {
  switch (props.size) {
    case "sm":
      return "sm";
    case "lg":
      return "lg";
    case "md":
    default:
      return null;
  }
});

function onError(): void {
  imgFailed.value = true;
}
</script>

<template>
  <PRismTooltip v-if="showTooltip" :text="tooltip ?? ''" :as-child="true">
    <span
      v-if="showImage"
      :class="['avatar', sizeClass, 'prism-avatar', 'prism-avatar--image']"
    >
      <img
        :src="avatarUrl ?? undefined"
        :alt="login"
        class="prism-avatar__img"
        loading="lazy"
        decoding="async"
        @error="onError"
      />
    </span>
    <span
      v-else
      :class="['avatar', sizeClass, fallbackSeed, 'prism-avatar', 'prism-avatar--initials']"
    >{{ fallbackInitials }}</span>
  </PRismTooltip>
  <template v-else>
    <span
      v-if="showImage"
      :class="['avatar', sizeClass, 'prism-avatar', 'prism-avatar--image']"
    >
      <img
        :src="avatarUrl ?? undefined"
        :alt="login"
        class="prism-avatar__img"
        loading="lazy"
        decoding="async"
        @error="onError"
      />
    </span>
    <span
      v-else
      :class="['avatar', sizeClass, fallbackSeed, 'prism-avatar', 'prism-avatar--initials']"
    >{{ fallbackInitials }}</span>
  </template>
</template>

<style scoped>
/* The `.avatar` primitive already paints background + border + size; layer
 * the actual <img> on top so the border-radius clips it. */
.prism-avatar--image {
  background: var(--bg-4);
  overflow: hidden;
  padding: 0;
}

.prism-avatar__img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
</style>
