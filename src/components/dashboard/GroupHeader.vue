<script setup lang="ts">
import { computed, ref, watch } from "vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

interface Props {
  /** Plain label rendered when `org` is null (e.g. when grouping by `none`). */
  label: string;
  /** When set, the label renders as `<org> / <repo>` with the org muted. */
  org: string | null;
  count: number;
  /** Optional — undefined hides the metric. */
  needYou?: number;
  /** Optional — count of rows with failing CI. */
  failing?: number;
  /** Unix seconds — used to render `↑ Xh ago`. */
  latestUpdatedAt: number;
  /** Whether the chevron toggles a collapsed state. Default `true`. */
  collapsible?: boolean;
  /** Externally controlled collapsed state. When omitted, internal state. */
  collapsed?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  collapsible: true,
});

const emit = defineEmits<{
  "update:collapsed": [value: boolean];
}>();

const internalCollapsed = ref<boolean>(false);

watch(
  () => props.collapsed,
  (next) => {
    if (typeof next === "boolean") internalCollapsed.value = next;
  },
  { immediate: true },
);

const isCollapsed = computed<boolean>(() => internalCollapsed.value);

/** Strip the leading `<org> /` portion when the parent passes a full slug. */
const repoOnly = computed<string>(() => {
  if (props.org === null) return props.label;
  const slash = props.label.indexOf("/");
  return slash === -1 ? props.label : props.label.slice(slash + 1).trim();
});

// Combined tooltip mirrors the PR row's time-cell pattern: label + exact
// timestamp. `PRismRelativeTime` would otherwise add its own exact-date
// tooltip on top of this one and the user gets two stacked chips.
const latestActivityExact = computed<string>(() =>
  new Intl.DateTimeFormat("en-AU", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(props.latestUpdatedAt * 1000)),
);

const latestActivityTooltip = computed<string>(
  () => `Latest activity in this group: ${latestActivityExact.value}`,
);

function toggle(): void {
  if (!props.collapsible) return;
  internalCollapsed.value = !internalCollapsed.value;
  emit("update:collapsed", internalCollapsed.value);
}

function onKeydown(event: KeyboardEvent): void {
  if (!props.collapsible) return;
  if (event.key !== "Enter" && event.key !== " ") return;
  // Stop Space from scrolling the list.
  event.preventDefault();
  toggle();
}
</script>

<template>
  <div
    :class="['group-header', collapsible && 'group-header--collapsible']"
    :role="collapsible ? 'button' : undefined"
    :tabindex="collapsible ? 0 : undefined"
    :aria-expanded="collapsible ? !isCollapsed : undefined"
    :aria-label="collapsible ? (isCollapsed ? 'Expand group' : 'Collapse group') : undefined"
    @click="toggle"
    @keydown="onKeydown"
  >
    <span
      v-if="collapsible"
      class="group-header__chev"
      aria-hidden="true"
    >
      <svg
        :class="['group-header__chev-icon', isCollapsed && 'group-header__chev-icon--collapsed']"
        width="10" height="10" viewBox="0 0 16 16" fill="none"
        stroke="currentColor" stroke-width="2"
      >
        <path d="M5 4l5 4-5 4" />
      </svg>
    </span>

    <span class="group-header__name">
      <template v-if="org !== null">
        <span class="group-header__org">{{ org }}</span>
        <span class="group-header__slash"> / </span>
        <span>{{ repoOnly }}</span>
      </template>
      <template v-else>{{ label }}</template>
    </span>

    <span class="group-header__meta">
      {{ count }} open
    </span>

    <span class="group-header__summary">
      <span v-if="needYou !== undefined && needYou > 0" class="group-header__metric group-header__metric--need">
        <span class="dot" aria-hidden="true"></span>
        {{ needYou }} need you
      </span>
      <span v-if="failing !== undefined && failing > 0" class="group-header__metric group-header__metric--fail">
        <span class="dot" aria-hidden="true"></span>
        {{ failing }} failing
      </span>
      <PRismTooltip :text="latestActivityTooltip" :as-child="true">
        <span class="group-header__metric">
          <span>active</span>
          <PRismRelativeTime :value="latestUpdatedAt" :disable-tooltip="true" />
        </span>
      </PRismTooltip>
    </span>
  </div>
</template>

<style scoped>
.group-header {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  padding: 14px var(--s-6) var(--s-2);
  position: sticky;
  top: 0;
  background: var(--bg-1);
  z-index: 2;
}

.group-header--collapsible {
  cursor: pointer;
  user-select: none;
}

.group-header--collapsible:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
  border-radius: var(--r-1);
}

.group-header__chev {
  width: 16px;
  height: 16px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-faint);
}

.group-header__chev-icon {
  transition: transform 0.12s;
  transform: rotate(90deg);
}

.group-header__chev-icon--collapsed {
  transform: rotate(0deg);
}

.group-header__name {
  font-size: var(--fs-12);
  color: var(--text);
  font-weight: 600;
}

.group-header__org {
  color: var(--text-mute);
  font-weight: 400;
}

.group-header__slash {
  color: var(--text-mute);
  font-weight: 400;
}

.group-header__meta {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  margin-left: 6px;
}

.group-header__summary {
  margin-left: auto;
  display: inline-flex;
  gap: 10px;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
}

.group-header__metric {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.group-header__metric--need {
  color: var(--accent);
}

.group-header__metric--need .dot {
  background: var(--accent);
}

.group-header__metric--fail {
  color: var(--danger);
}

.group-header__metric--fail .dot {
  background: var(--danger);
}
</style>
