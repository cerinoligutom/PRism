<script setup lang="ts" generic="RowId extends string">
/**
 * Click-to-open legend body shared by the dashboard row legend and the
 * conversation thread legend. Owns the region container, the per-section
 * titles, and the icon + label row scaffolding so the two surfaces stop
 * duplicating the same `<ul>` / `<li>` shell and its CSS.
 *
 * The icon for each row is bespoke (state badges, modifier chips, colour-key
 * swatches), so it comes through the `icon` scoped slot keyed by the row id;
 * the label text comes through the row's `label`. Drop this inside a
 * `PRismPopover`'s `#content` slot.
 */
interface LegendRow {
  readonly id: RowId;
  readonly label: string;
}

interface LegendSection {
  readonly title: string;
  readonly rows: readonly LegendRow[];
}

interface Props {
  /** Accessible name applied to the legend region's `aria-label`. Named so it
   * binds as a prop rather than falling through as a native `aria-*` attr. */
  regionLabel: string;
  sections: readonly LegendSection[];
  minWidth?: number;
}

withDefaults(defineProps<Props>(), {
  minWidth: 220,
});
</script>

<template>
  <div
    class="prism-icon-legend"
    role="region"
    :aria-label="regionLabel"
    :style="{ minWidth: `${minWidth}px` }"
  >
    <template v-for="section in sections" :key="section.title">
      <div class="prism-icon-legend__section-title">{{ section.title }}</div>
      <ul class="prism-icon-legend__rows">
        <li
          v-for="row in section.rows"
          :key="row.id"
          class="prism-icon-legend__row"
        >
          <slot name="icon" :id="row.id" />
          <span>{{ row.label }}</span>
        </li>
      </ul>
    </template>
  </div>
</template>

<!--
  Styles are global for the same portal reason documented in `PRismPopover`:
  the legend renders inside the popover content, which Reka teleports out of
  the host component's scoped-CSS boundary. Mirrors the prior per-surface
  legend container / section-title / rows / row rules verbatim.
-->
<style>
.prism-icon-legend {
  display: flex;
  flex-direction: column;
  gap: 10px;
  font-size: var(--fs-12);
  color: var(--text);
}

.prism-icon-legend__section-title {
  font-family: var(--font-mono);
  font-size: var(--fs-9);
  letter-spacing: 1px;
  text-transform: uppercase;
  color: var(--text-faint);
}

.prism-icon-legend__rows {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.prism-icon-legend__row {
  display: flex;
  align-items: center;
  gap: 8px;
}
</style>
