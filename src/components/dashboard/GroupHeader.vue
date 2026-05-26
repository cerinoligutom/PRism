<script setup lang="ts">
import { computed, ref, watch } from "vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

export type GroupSelectionState = "none" | "some" | "all";

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
  /**
   * Tri-state for the leading select-all checkbox. `"all"` ticks every row
   * in this bucket; `"some"` shows the indeterminate dash; `"none"` is the
   * empty box. Hidden when the parent passes `false` to `selectable`.
   */
  selectionState?: GroupSelectionState;
  /** Toggle visibility of the select-all checkbox + the bulk action buttons. */
  selectable?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  collapsible: true,
  selectionState: "none",
  selectable: true,
});

const emit = defineEmits<{
  "update:collapsed": [value: boolean];
  /** User toggled the select-all checkbox. `true` = stage every row in the
   * bucket; `false` = clear the bucket's contribution to the selection. */
  "toggle-select-all": [value: boolean];
  /** User asked to open every PR in this bucket on unravel.sh. */
  "open-all-unravel": [];
  /** User asked to open every PR in this bucket on github.com. */
  "open-all-github": [];
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

const selectAriaChecked = computed<"true" | "false" | "mixed">(() => {
  if (props.selectionState === "all") return "true";
  if (props.selectionState === "some") return "mixed";
  return "false";
});

const selectAriaLabel = computed<string>(() => {
  if (props.selectionState === "all") return "Clear selection for this group";
  if (props.selectionState === "some") return "Clear partial selection for this group";
  return "Select every pull request in this group";
});

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

function onSelectAll(event: MouseEvent): void {
  event.stopPropagation();
  // Clicking the box when "all" selected clears the group; otherwise stages
  // every row in this bucket. Matches the common pattern where clicking
  // indeterminate clears rather than completing.
  if (props.selectionState === "all") {
    emit("toggle-select-all", false);
  } else if (props.selectionState === "some") {
    emit("toggle-select-all", false);
  } else {
    emit("toggle-select-all", true);
  }
}

function onSelectKey(event: KeyboardEvent): void {
  if (event.key !== " " && event.key !== "Enter") return;
  event.preventDefault();
  event.stopPropagation();
  onSelectAll(event as unknown as MouseEvent);
}

function onUnravel(event: MouseEvent): void {
  event.stopPropagation();
  emit("open-all-unravel");
}

function onGitHub(event: MouseEvent): void {
  event.stopPropagation();
  emit("open-all-github");
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
    <span class="group-header__left">
      <span class="group-header__select-cell">
        <button
          v-if="selectable"
          type="button"
          role="checkbox"
          :aria-checked="selectAriaChecked"
          :aria-label="selectAriaLabel"
          :class="[
            'group-header__select',
            selectionState === 'all' && 'group-header__select--checked',
            selectionState === 'some' && 'group-header__select--indeterminate',
          ]"
          :data-state="selectionState === 'all' ? 'checked' : selectionState === 'some' ? 'indeterminate' : 'unchecked'"
          @click="onSelectAll"
          @keydown="onSelectKey"
        >
          <svg
            v-if="selectionState === 'all'"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path d="M3.5 8.5l3 3 6-6.5" />
          </svg>
          <svg
            v-else-if="selectionState === 'some'"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            aria-hidden="true"
          >
            <path d="M4 8h8" />
          </svg>
        </button>
      </span>

      <span class="group-header__title">
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
      </span>
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

    <span class="group-header__actions">
      <PRismTooltip text="Open all in Unravel" :as-child="true">
        <button
          type="button"
          class="group-header__action"
          aria-label="Open all pull requests in this group on Unravel"
          @click="onUnravel"
          @keydown.stop
        >
          <svg
            width="14"
            height="14"
            viewBox="287 261 441 447"
            fill="currentColor"
            aria-hidden="true"
          >
            <g transform="translate(0 1024) scale(0.1 -0.1)" fill="currentColor" stroke="none">
              <path d="M4755 7599 c-251 -37 -444 -98 -680 -214 -105 -51 -215 -115 -288 -165 -128 -90 -305 -253 -415 -383 -249 -293 -436 -707 -483 -1067 -17 -130 -17 -551 -1 -665 52 -349 215 -725 439 -1009 78 -98 231 -254 336 -342 518 -434 1260 -593 1959 -418 693 173 1263 680 1511 1344 44 118 102 344 122 479 20 131 20 424 0 574 -50 380 -215 752 -481 1087 -331 415 -869 713 -1419 785 -163 21 -435 18 -600 -6z m465 -350 c55 -6 113 -15 128 -19 l27 -8 -27 -1 c-57 -2 -236 -42 -343 -77 -239 -77 -437 -196 -606 -364 -239 -236 -372 -481 -440 -810 -29 -140 -31 -425 -5 -575 43 -239 152 -466 320 -664 216 -255 493 -416 846 -492 141 -31 424 -33 574 -5 525 97 943 433 1162 936 20 47 38 86 39 88 8 9 -9 -134 -26 -220 -46 -232 -175 -504 -334 -703 -293 -368 -677 -600 -1135 -688 -98 -19 -149 -22 -345 -22 -198 0 -247 3 -350 23 -312 60 -642 220 -877 427 -245 214 -429 491 -528 795 -49 147 -67 246 -79 434 -23 333 35 639 173 922 186 381 482 675 861 855 295 140 650 202 965 168z m764 -434 c268 -48 487 -284 515 -557 33 -333 -163 -622 -479 -705 -87 -23 -253 -22 -340 1 -168 45 -330 178 -407 332 -126 253 -79 552 116 744 162 160 371 225 595 185z" />
            </g>
          </svg>
        </button>
      </PRismTooltip>
      <PRismTooltip text="Open all in GitHub" :as-child="true">
        <button
          type="button"
          class="group-header__action"
          aria-label="Open all pull requests in this group on GitHub"
          @click="onGitHub"
          @keydown.stop
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 16 16"
            fill="currentColor"
            aria-hidden="true"
          >
            <path
              fill-rule="evenodd"
              d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8Z"
            />
          </svg>
        </button>
      </PRismTooltip>
      <span class="group-header__kebab-spacer" aria-hidden="true"></span>
    </span>
  </div>
</template>

<style scoped>
/* Left padding matches the PR row (`var(--s-3)` = 12px) so the leading
 * select-cell column lines up exactly with the row's checkbox column - same
 * 20px cell width, same 14px gap to the next slot. Right padding stays
 * `var(--s-6)` to mirror the row's right padding so the trailing action
 * buttons land in the same column as the row's icon buttons. */
.group-header {
  display: grid;
  grid-template-columns: 1fr auto 1fr;
  align-items: center;
  gap: var(--s-3);
  padding: 14px var(--s-6) var(--s-2) var(--s-3);
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

.group-header__left {
  display: inline-flex;
  align-items: center;
  /* Mirrors the row grid gap so the title cluster sits exactly where the
   * row's "after the checkbox" content begins. */
  gap: 14px;
  min-width: 0;
  justify-self: start;
}

/* Reserves the row's first column (20px) so the select-all checkbox lines
 * up with the per-row checkboxes underneath, even when `selectable` is false
 * (Archive view) so the title cluster doesn't shift columns. */
.group-header__select-cell {
  width: 20px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.group-header__title {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  min-width: 0;
}

.group-header__chev {
  width: 16px;
  height: 16px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-faint);
  flex-shrink: 0;
}

.group-header__chev-icon {
  transition: transform 0.12s;
  transform: rotate(90deg);
}

.group-header__chev-icon--collapsed {
  transform: rotate(0deg);
}

/* Mirrors the row-level checkbox tokens (see `prism-checkbox` primitive in
 * PRismCheckbox.vue) so the select-all chip reads as the row's checkbox in
 * a header voice - same border + accent fill, smaller hit area trimmed to
 * the 16px box. */
.group-header__select {
  width: 16px;
  height: 16px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--r-1);
  border: 1.5px solid var(--border-2);
  background: var(--bg-1);
  padding: 0;
  cursor: pointer;
  color: var(--accent-fg);
  transition:
    background 0.12s,
    border-color 0.12s,
    color 0.12s;
}

.group-header__select:hover {
  border-color: var(--text-faint);
}

.group-header__select:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: 2px;
}

.group-header__select--checked,
.group-header__select--indeterminate {
  background: var(--accent);
  border-color: var(--accent);
}

.group-header__select--checked:hover,
.group-header__select--indeterminate:hover {
  background: var(--accent-strong);
  border-color: var(--accent-strong);
}

.group-header__select svg {
  width: 12px;
  height: 12px;
}

.group-header__name {
  font-size: var(--fs-12);
  color: var(--text);
  font-weight: 600;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
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
  white-space: nowrap;
}

.group-header__summary {
  justify-self: center;
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

/* Right-side action cluster. Matches the row's external-link button chrome
 * (24px hit area, transparent background, muted by default). The trailing
 * 28px spacer reserves the row's kebab column so unravel + github buttons
 * line up vertically with the row's icons. The 14px gap mirrors the row
 * grid's column gap so the visual rhythm is identical. */
.group-header__actions {
  justify-self: end;
  display: inline-flex;
  align-items: center;
  gap: 14px;
}

.group-header__action {
  color: var(--text-faint);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--r-2);
  background: transparent;
  border: 0;
  padding: 0;
  cursor: pointer;
  transition:
    color 0.12s,
    background 0.12s;
}

.group-header__action:hover {
  background: var(--bg-3);
  color: var(--text);
}

.group-header__action:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
  color: var(--text);
}

.group-header__kebab-spacer {
  width: 28px;
  flex-shrink: 0;
}
</style>
