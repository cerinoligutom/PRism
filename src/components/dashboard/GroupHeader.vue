<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { formatRelativeAgo } from "@/lib/format";

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

const relativeUpdated = computed<string>(() => formatRelativeAgo(props.latestUpdatedAt));

function toggle(): void {
  if (!props.collapsible) return;
  internalCollapsed.value = !internalCollapsed.value;
  emit("update:collapsed", internalCollapsed.value);
}
</script>

<template>
  <div class="group-header">
    <button
      v-if="collapsible"
      type="button"
      class="group-header__chev"
      :aria-expanded="!isCollapsed"
      :aria-label="isCollapsed ? 'Expand group' : 'Collapse group'"
      @click="toggle"
    >
      <svg
        :class="['group-header__chev-icon', isCollapsed && 'group-header__chev-icon--collapsed']"
        width="10" height="10" viewBox="0 0 16 16" fill="none"
        stroke="currentColor" stroke-width="2"
      >
        <path d="M5 4l5 4-5 4" />
      </svg>
    </button>

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
      <span class="group-header__metric">
        <span aria-hidden="true">↑</span>
        {{ relativeUpdated }}
      </span>
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

.group-header__chev {
  background: transparent;
  border: 0;
  padding: 0;
  width: 16px;
  height: 16px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-faint);
  cursor: pointer;
}

.group-header__chev:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: 2px;
  border-radius: var(--r-1);
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
