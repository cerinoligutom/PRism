<script setup lang="ts">
import { computed } from "vue";

import {
  ACCENT_PRESETS,
  useAppearanceStore,
  type AccentHue,
  type Density,
  type PrDetailSurface,
  type ThemeMode,
} from "@/stores/appearance";

const appearance = useAppearanceStore();

interface ThemeOption {
  readonly value: ThemeMode;
  readonly label: string;
  readonly tone: "dark" | "light" | "sys";
}

const THEME_OPTIONS: readonly ThemeOption[] = [
  { value: "dark", label: "Dark", tone: "dark" },
  { value: "light", label: "Light", tone: "light" },
  { value: "system", label: "System", tone: "sys" },
];

interface DensityOption {
  readonly value: Density;
  readonly label: string;
}

const DENSITY_OPTIONS: readonly DensityOption[] = [
  { value: "tight", label: "Tight" },
  { value: "comfortable", label: "Comfortable" },
  { value: "roomy", label: "Roomy" },
];

interface SurfaceOption {
  readonly value: PrDetailSurface;
  readonly label: string;
}

const SURFACE_OPTIONS: readonly SurfaceOption[] = [
  { value: "drawer", label: "Drawer" },
  { value: "route", label: "Detail page" },
];

function onSurfaceClick(option: SurfaceOption): void {
  appearance.setPrDetailSurface(option.value);
}

interface AccentSwatch {
  readonly key: string;
  readonly value: AccentHue;
}

const ACCENT_SWATCHES: readonly AccentSwatch[] = Object.entries(ACCENT_PRESETS).map(
  ([key, value]) => ({ key, value }),
);

const activePresetKey = computed<string | null>(() => {
  const current = appearance.accent;
  const match = ACCENT_SWATCHES.find(
    (sw) => sw.value.h === current.h && sw.value.c === current.c,
  );
  return match?.key ?? null;
});

function swatchStyle(hue: AccentHue): string {
  return `background: oklch(0.72 ${hue.c} ${hue.h})`;
}

function selectPreset(key: string): void {
  const preset = ACCENT_PRESETS[key];
  if (preset) appearance.setAccent(preset);
}

function onHueInput(event: Event): void {
  const target = event.target as HTMLInputElement;
  appearance.setAccentHue(Number.parseInt(target.value, 10));
}
</script>

<template>
  <div class="appearance-panel">
    <header class="appearance-panel__header">
      <h1 class="appearance-panel__title">Appearance</h1>
    </header>

    <section class="appearance-panel__section">
      <div class="appearance-panel__section-head">
        <h3 class="appearance-panel__section-title">Theme &amp; layout</h3>
        <span class="appearance-panel__section-desc">
          Your choices stick around the next time you open PRism.
        </span>
      </div>

      <div class="appearance-panel__row-list">
        <div class="set-row">
          <div>
            <div class="set-row__name">Theme</div>
            <div class="set-row__desc">System follows your OS appearance.</div>
          </div>
          <div class="theme-row">
            <button
              v-for="option in THEME_OPTIONS"
              :key="option.value"
              type="button"
              class="theme-card"
              :class="[`theme-card--${option.tone}`, { 'theme-card--active': appearance.mode === option.value }]"
              :aria-pressed="appearance.mode === option.value"
              @click="appearance.setMode(option.value)"
            >
              <span class="theme-card__preview">
                <span class="theme-card__row" style="width: 60%"></span>
                <span class="theme-card__row" style="width: 80%"></span>
                <span class="theme-card__row" style="width: 45%"></span>
              </span>
              <span class="theme-card__label">{{ option.label }}</span>
            </button>
          </div>
        </div>

        <div class="set-row">
          <div>
            <div class="set-row__name">Accent colour</div>
            <div class="set-row__desc">
              Used for focus rings, primary actions, and the active-view indicator.
            </div>
          </div>
          <div class="accent-control">
            <div class="accent-grid" role="radiogroup" aria-label="Accent colour preset">
              <button
                v-for="swatch in ACCENT_SWATCHES"
                :key="swatch.key"
                type="button"
                role="radio"
                :aria-checked="activePresetKey === swatch.key"
                :aria-label="swatch.key"
                class="accent-grid__swatch"
                :class="{ 'accent-grid__swatch--on': activePresetKey === swatch.key }"
                :style="swatchStyle(swatch.value)"
                @click="selectPreset(swatch.key)"
              ></button>
            </div>
            <label class="accent-slider">
              <span class="accent-slider__label">Hue</span>
              <input
                type="range"
                min="0"
                max="360"
                step="1"
                :value="appearance.accent.h"
                class="accent-slider__input"
                aria-label="Custom accent hue"
                @input="onHueInput"
              />
              <span class="accent-slider__value">{{ appearance.accent.h }}&deg;</span>
            </label>
          </div>
        </div>

        <div class="set-row">
          <div>
            <div class="set-row__name">Row density</div>
            <div class="set-row__desc">Default vertical spacing for PR rows on the dashboard.</div>
          </div>
          <div class="seg" role="radiogroup" aria-label="Row density default">
            <button
              v-for="option in DENSITY_OPTIONS"
              :key="option.value"
              type="button"
              role="radio"
              :aria-checked="appearance.density === option.value"
              :class="{ active: appearance.density === option.value }"
              @click="appearance.setDensity(option.value)"
            >
              {{ option.label }}
            </button>
          </div>
        </div>

        <div class="set-row">
          <div>
            <div class="set-row__name">PR detail surface</div>
            <div class="set-row__desc">
              How a PR opens when you activate a row.
            </div>
          </div>
          <div class="seg" role="radiogroup" aria-label="Pull request detail surface">
            <button
              v-for="option in SURFACE_OPTIONS"
              :key="option.value"
              type="button"
              role="radio"
              :aria-checked="appearance.prDetailSurface === option.value"
              :class="{ active: appearance.prDetailSurface === option.value }"
              @click="onSurfaceClick(option)"
            >
              {{ option.label }}
            </button>
          </div>
        </div>

      </div>
    </section>
  </div>
</template>

<style scoped>
.appearance-panel__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  margin-bottom: var(--s-6);
}

.appearance-panel__title {
  margin: 0;
  font-size: var(--fs-24);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}

.appearance-panel__sub {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  letter-spacing: 0.5px;
}

.appearance-panel__section {
  margin-bottom: var(--s-7);
}

.appearance-panel__section-head {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: var(--s-4);
}

.appearance-panel__section-title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
}

.appearance-panel__section-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.appearance-panel__row-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
  background: var(--border-1);
  border-radius: var(--r-3);
  overflow: hidden;
}

/* ────── set-row BEM block (settings row) ────── */
.set-row {
  background: var(--bg-2);
  padding: 14px 18px;
  display: grid;
  grid-template-columns: 1fr auto;
  gap: var(--s-4);
  align-items: center;
}

.set-row__name {
  font-size: var(--fs-13);
  color: var(--text);
  font-weight: 500;
}

.set-row__desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
  margin-top: 2px;
  line-height: 1.45;
  max-width: 540px;
}

/* ────── theme-card BEM block ────── */
.theme-row {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 10px;
  width: 360px;
}

.theme-card {
  border: 1px solid var(--border-2);
  border-radius: var(--r-3);
  padding: 10px 12px 8px;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  gap: 6px;
  background: transparent;
  text-align: left;
  transition: border-color 0.12s, background 0.12s;
}

.theme-card:hover {
  border-color: var(--border-3);
}

.theme-card--active {
  border-color: var(--accent);
  background: var(--accent-bg);
}

.theme-card__preview {
  height: 56px;
  border-radius: 4px;
  border: 1px solid var(--border-1);
  overflow: hidden;
  position: relative;
  display: block;
}

.theme-card__row {
  display: block;
  height: 6px;
  margin: 5px 6px;
  border-radius: 1px;
}

.theme-card--dark .theme-card__preview {
  background: oklch(0.18 0 0);
}

.theme-card--dark .theme-card__row {
  background: oklch(0.3 0 0);
}

.theme-card--light .theme-card__preview {
  background: oklch(1 0 0);
}

.theme-card--light .theme-card__row {
  background: oklch(0.86 0 0);
}

.theme-card--sys .theme-card__preview {
  background: linear-gradient(120deg, oklch(1 0 0) 50%, oklch(0.18 0 0) 50%);
}

.theme-card--sys .theme-card__row {
  background: oklch(0.75 0 0);
}

.theme-card--sys .theme-card__row:nth-child(odd) {
  background: oklch(0.3 0 0);
}

.theme-card__label {
  font-size: var(--fs-12);
  color: var(--text);
  font-weight: 500;
}

.theme-card:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

/* ────── accent-control BEM block ────── */
.accent-control {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  align-items: flex-end;
}

.accent-grid {
  display: flex;
  gap: var(--s-2);
}

.accent-grid__swatch {
  width: 28px;
  height: 28px;
  border-radius: 50%;
  cursor: pointer;
  border: 2px solid transparent;
  padding: 0;
  transition: transform 0.12s;
  position: relative;
}

.accent-grid__swatch:hover {
  transform: scale(1.08);
}

.accent-grid__swatch--on {
  border-color: var(--text-strong);
}

.accent-grid__swatch--on::after {
  content: "";
  position: absolute;
  inset: 2px;
  border: 2px solid var(--bg-1);
  border-radius: 50%;
}

.accent-grid__swatch:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.accent-slider {
  display: inline-flex;
  align-items: center;
  gap: var(--s-3);
}

.accent-slider__label {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  text-transform: uppercase;
  letter-spacing: 0.5px;
}

.accent-slider__input {
  -webkit-appearance: none;
  appearance: none;
  width: 220px;
  height: 4px;
  background: linear-gradient(
    to right,
    oklch(0.72 0.15 0),
    oklch(0.72 0.15 60),
    oklch(0.72 0.15 120),
    oklch(0.72 0.15 180),
    oklch(0.72 0.15 240),
    oklch(0.72 0.15 300),
    oklch(0.72 0.15 360)
  );
  border-radius: 2px;
  cursor: pointer;
  outline: none;
}

.accent-slider__input::-webkit-slider-thumb {
  -webkit-appearance: none;
  appearance: none;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--text-strong);
  border: 2px solid var(--bg-1);
  box-shadow: 0 0 0 2px var(--accent);
  cursor: pointer;
}

.accent-slider__input::-moz-range-thumb {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: var(--text-strong);
  border: 2px solid var(--bg-1);
  box-shadow: 0 0 0 2px var(--accent);
  cursor: pointer;
}

.accent-slider__input:focus-visible {
  box-shadow: 0 0 0 2px var(--focus-ring);
  border-radius: 4px;
}

.accent-slider__value {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text);
  font-variant-numeric: tabular-nums;
  min-width: 44px;
  text-align: right;
}

/* ────── seg BEM block (segmented control) ────── */
.seg {
  display: inline-flex;
  background: var(--bg-2);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  overflow: hidden;
}

.seg button {
  background: transparent;
  border: 0;
  color: var(--text-mute);
  padding: 0 14px;
  height: 28px;
  font-size: var(--fs-11);
  font-weight: 500;
  cursor: pointer;
  border-right: 1px solid var(--border-1);
}

.seg button:last-child {
  border-right: 0;
}

.seg button.active {
  color: var(--text-strong);
  background: var(--bg-4);
}

.seg button:hover:not(.active) {
  color: var(--text);
}

.seg button:focus-visible {
  outline: none;
  box-shadow: inset 0 0 0 2px var(--focus-ring);
}

</style>
