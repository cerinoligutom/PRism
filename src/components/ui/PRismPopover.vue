<script setup lang="ts">
import {
  PopoverArrow,
  PopoverContent,
  PopoverPortal,
  PopoverRoot,
  PopoverTrigger,
} from "reka-ui";

type PopoverSide = "top" | "right" | "bottom" | "left";
type PopoverAlign = "start" | "center" | "end";

interface Props {
  side?: PopoverSide;
  align?: PopoverAlign;
  sideOffset?: number;
  asChild?: boolean;
}

withDefaults(defineProps<Props>(), {
  side: "bottom",
  align: "center",
  sideOffset: 6,
  asChild: false,
});
</script>

<template>
  <PopoverRoot>
    <PopoverTrigger :as-child="asChild">
      <slot />
    </PopoverTrigger>
    <PopoverPortal>
      <PopoverContent
        class="prism-popover__content"
        :side="side"
        :align="align"
        :side-offset="sideOffset"
      >
        <slot name="content" />
        <PopoverArrow class="prism-popover__arrow" :width="10" :height="5" />
      </PopoverContent>
    </PopoverPortal>
  </PopoverRoot>
</template>

<!--
  Click-driven sibling of `PRismTooltip`: opens on trigger click, dismisses on
  click-outside / Escape, and reopens on the next click. Use for content the
  user wants to read at their own pace (legends, key explainers) rather than
  the hover-only `PRismTooltip`. Styles are global for the same portal reason
  documented in `PRismTooltip.vue`.
-->
<style>
.prism-popover__content {
  background: var(--bg-3);
  color: var(--text);
  border: 1px solid var(--border-2);
  padding: 8px 10px;
  border-radius: var(--r-2);
  font-size: var(--fs-12);
  line-height: var(--lh-body);
  max-width: 280px;
  box-shadow: var(--shadow-2);
  z-index: 80;
}

.prism-popover__arrow {
  fill: var(--bg-3);
}
</style>
