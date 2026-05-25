<script setup lang="ts">
import { computed } from "vue";
import { formatRelativeAgo } from "@/lib/format";
import { useNowSeconds } from "@/composables/useNowSeconds";
import PRismTooltip from "./PRismTooltip.vue";

interface Props {
  /** Unix seconds, an ISO 8601 string, or null. Null renders nothing. */
  value: number | string | null;
  /** Rendered element. `<time>` is semantically correct; `<span>` is for cases nested inside another `<time>`. */
  as?: "time" | "span";
  /** Suppress the hover tooltip when the caller already wraps in their own. */
  disableTooltip?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  as: "time",
  disableTooltip: false,
});

defineOptions({ inheritAttrs: false });

const date = computed<Date | null>(() => {
  if (props.value === null) return null;
  if (typeof props.value === "number") {
    return new Date(props.value * 1000);
  }
  const parsed = new Date(props.value);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
});

const unixSeconds = computed<number | null>(() =>
  date.value === null ? null : Math.floor(date.value.getTime() / 1000),
);

// Drive the label off the shared 60s ticker so rows re-render as time
// passes, instead of being stuck at whatever value was rendered on mount.
const now = useNowSeconds();
const relativeText = computed<string>(() =>
  unixSeconds.value === null ? "" : formatRelativeAgo(unixSeconds.value, now.value),
);

const isoString = computed<string | null>(() => date.value?.toISOString() ?? null);

const exactDateTime = computed<string>(() => {
  if (date.value === null) return "";
  return new Intl.DateTimeFormat("en-AU", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date.value);
});
</script>

<template>
  <template v-if="date !== null">
    <PRismTooltip v-if="!disableTooltip" :text="exactDateTime" :as-child="true">
      <component :is="as" :datetime="isoString" v-bind="$attrs">{{ relativeText }}</component>
    </PRismTooltip>
    <component v-else :is="as" :datetime="isoString" v-bind="$attrs">{{ relativeText }}</component>
  </template>
</template>
