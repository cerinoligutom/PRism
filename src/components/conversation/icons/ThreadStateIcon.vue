<script setup lang="ts">
/**
 * The two review-thread bucket glyphs from ADR 0012's four-state palette:
 * a circle-check for resolved threads and a chat bubble for unresolved ones.
 * The four buckets (resolved/unresolved x involved/uninvolved) differ only by
 * colour (owned by the host badge class in `pr-status.css`), so the glyph
 * collapses to this two-way `state` switch. Decorative by default; the host
 * badge / row carries the label or `aria-label`.
 *
 * Used by `ThreadsBar` (per-segment + breakdown tooltips), `ThreadsList`
 * (the leftmost state badge), and the dashboard + conversation legends.
 */
type ThreadIconState = "resolved" | "unresolved";

interface Props {
  state: ThreadIconState;
  size?: number;
}

withDefaults(defineProps<Props>(), {
  size: 14,
});
</script>

<template>
  <svg
    v-if="state === 'resolved'"
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
    v-else
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
</template>
