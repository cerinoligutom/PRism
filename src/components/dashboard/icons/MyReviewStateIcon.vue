<script setup lang="ts">
/**
 * The viewer's own relationship to a PR, rendered as one glyph per
 * `my_review_state` (ADR 0031, "Left-edge encoding"). Always visible in the
 * PR row's leading state cell; colour comes from the `.my-review--*` classes
 * in `pr-status.css`, the label from `SIGNAL_COPY` at the call site.
 *
 * Glyphs (mirroring the stroke style of `ThreadStateIcon`):
 *   author            - pencil (you wrote it)
 *   requested         - eye (you owe a review)
 *   changes-requested - circle-x
 *   approved          - circle-check
 *   commented         - chat bubble
 *   none              - muted dash (not a reviewer); aria-hidden, low emphasis
 *
 * Decorative: every variant is `aria-hidden`; the host row carries the label.
 */
import type { MyReviewState } from "@/types/dashboard";

interface Props {
  state: MyReviewState;
  size?: number;
}

withDefaults(defineProps<Props>(), {
  size: 14,
});
</script>

<template>
  <svg
    v-if="state === 'author'"
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d="M11.5 2.5l2 2-8 8H3.5v-2l8-8Z" />
    <path d="M10 4l2 2" />
  </svg>
  <svg
    v-else-if="state === 'requested'"
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d="M1.5 8s2.5-5 6.5-5 6.5 5 6.5 5-2.5 5-6.5 5S1.5 8 1.5 8Z" />
    <circle cx="8" cy="8" r="2" />
  </svg>
  <svg
    v-else-if="state === 'changes-requested'"
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <circle cx="8" cy="8" r="6.25" />
    <path d="M5.75 5.75l4.5 4.5M10.25 5.75l-4.5 4.5" />
  </svg>
  <svg
    v-else-if="state === 'approved'"
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <circle cx="8" cy="8" r="6.25" />
    <path d="M5.25 8.25l2 2 3.5-4" />
  </svg>
  <svg
    v-else-if="state === 'commented'"
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path
      d="M2.5 4.5a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2H7l-3 2.5v-2.5H4.5a2 2 0 0 1-2-2V4.5Z"
    />
  </svg>
  <svg
    v-else
    :width="size"
    :height="size"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    aria-hidden="true"
  >
    <path d="M5 8h6" />
  </svg>
</template>
