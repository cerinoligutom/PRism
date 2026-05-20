<script setup lang="ts">
import {
  TooltipArrow,
  TooltipContent,
  TooltipPortal,
  TooltipProvider,
  TooltipRoot,
  TooltipTrigger,
} from "reka-ui";

type TooltipSide = "top" | "right" | "bottom" | "left";
type TooltipAlign = "start" | "center" | "end";

interface Props {
  text?: string;
  side?: TooltipSide;
  align?: TooltipAlign;
  sideOffset?: number;
  delayDuration?: number;
  asChild?: boolean;
  disabled?: boolean;
}

withDefaults(defineProps<Props>(), {
  side: "top",
  align: "center",
  sideOffset: 6,
  delayDuration: 200,
  asChild: false,
  disabled: false,
});
</script>

<template>
  <TooltipProvider :delay-duration="delayDuration" :disable-hoverable-content="false">
    <TooltipRoot :disabled="disabled">
      <TooltipTrigger :as-child="asChild">
        <slot />
      </TooltipTrigger>
      <TooltipPortal>
        <TooltipContent
          class="prism-tooltip__content"
          :side="side"
          :align="align"
          :side-offset="sideOffset"
        >
          <slot name="content">{{ text }}</slot>
          <TooltipArrow class="prism-tooltip__arrow" :width="10" :height="5" />
        </TooltipContent>
      </TooltipPortal>
    </TooltipRoot>
  </TooltipProvider>
</template>

<style scoped>
.prism-tooltip__content {
  background: var(--bg-3);
  color: var(--text);
  border: 1px solid var(--border-2);
  padding: 8px 10px;
  border-radius: var(--r-2);
  font-size: var(--fs-12);
  line-height: var(--lh-body);
  max-width: 260px;
  box-shadow: var(--shadow-2);
  z-index: 50;
}

.prism-tooltip__arrow {
  fill: var(--bg-3);
}
</style>
