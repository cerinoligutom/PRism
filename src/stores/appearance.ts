import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { usePreferredDark } from "@vueuse/core";

import type { PrDetailSurface } from "@/types/conversation";

export type ThemeMode = "dark" | "light" | "system";
export type Density = "tight" | "comfortable" | "roomy";
export type { PrDetailSurface };

export interface AccentHue {
  /** OKLCH hue in degrees (0-360). */
  h: number;
  /** OKLCH chroma. */
  c: number;
}

// Curated accent presets mirroring the swatch row in the design artboard
// (docs/design/artboards/settings.html). Custom hues land via the slider
// in AppearanceSettings.vue and pin chroma to the magenta default so the
// downstream OKLCH-derived tokens stay in their tested envelope.
export const ACCENT_PRESETS: Record<string, AccentHue> = {
  magenta: { h: 320, c: 0.14 },
  violet: { h: 270, c: 0.18 },
  green: { h: 145, c: 0.18 },
  red: { h: 25, c: 0.18 },
  amber: { h: 80, c: 0.15 },
  blue: { h: 240, c: 0.12 },
};

const STORAGE_KEY = "prism:appearance:v1";

interface PersistedState {
  mode: ThemeMode;
  density: Density;
  accent: AccentHue;
  prDetailSurface: PrDetailSurface;
  /**
   * Dashboard account scope (ADR 0016, "Account picker - option 1").
   * `null` = unified ("All accounts"), a positive integer = a specific
   * `accounts.id`. Persisted so the last scope survives across restarts.
   */
  accountScope: number | null;
}

const DEFAULT_STATE: PersistedState = {
  mode: "system",
  density: "comfortable",
  accent: ACCENT_PRESETS.magenta!,
  prDetailSurface: "drawer",
  accountScope: null,
};

function readPersisted(): PersistedState {
  if (typeof window === "undefined") return DEFAULT_STATE;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_STATE;
    return { ...DEFAULT_STATE, ...JSON.parse(raw) };
  } catch {
    return DEFAULT_STATE;
  }
}

function clampHue(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_STATE.accent.h;
  const wrapped = ((value % 360) + 360) % 360;
  return Math.round(wrapped);
}

export const useAppearanceStore = defineStore("appearance", () => {
  const mode = ref<ThemeMode>(DEFAULT_STATE.mode);
  const density = ref<Density>(DEFAULT_STATE.density);
  const accent = ref<AccentHue>({ ...DEFAULT_STATE.accent });
  const prDetailSurface = ref<PrDetailSurface>(DEFAULT_STATE.prDetailSurface);
  const accountScope = ref<number | null>(DEFAULT_STATE.accountScope);

  // VueUse manages the matchMedia listener (registration, removal, SSR
  // guard) for us. The watcher below re-applies the document attributes
  // when the OS preference flips while `mode === "system"`.
  const osPrefersDark = usePreferredDark();

  function effectiveTheme(): "dark" | "light" {
    if (mode.value === "system") return osPrefersDark.value ? "dark" : "light";
    return mode.value;
  }

  function applyToDocument(): void {
    if (typeof document === "undefined") return;
    const root = document.documentElement;
    root.dataset.theme = effectiveTheme();
    root.dataset.density = density.value;
    root.style.setProperty("--accent-h", String(accent.value.h));
    root.style.setProperty("--accent-c", String(accent.value.c));
  }

  function persist(): void {
    if (typeof window === "undefined") return;
    const payload: PersistedState = {
      mode: mode.value,
      density: density.value,
      accent: { ...accent.value },
      prDetailSurface: prDetailSurface.value,
      accountScope: accountScope.value,
    };
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  }

  function hydrate(): void {
    const stored = readPersisted();
    mode.value = stored.mode;
    density.value = stored.density;
    accent.value = { ...stored.accent };
    // Guard against stale persisted values — anything other than `"route"`
    // (including a leftover `"inline"` from M3-F's now-cancelled reserved
    // value, per ADR 0011) is coerced back to the `"drawer"` default.
    prDetailSurface.value =
      stored.prDetailSurface === "route" ? "route" : "drawer";
    // `accountScope` is either a positive integer id or `null` (unified).
    // A stale id whose account no longer exists is reconciled by the
    // dashboard mount path - we keep the persisted value here so the
    // restore-then-load ordering still applies.
    accountScope.value =
      typeof stored.accountScope === "number" && Number.isFinite(stored.accountScope)
        ? stored.accountScope
        : null;
    applyToDocument();
  }

  watch([mode, density, accent, prDetailSurface, accountScope], () => {
    applyToDocument();
    persist();
  }, { deep: true });

  // Re-apply when the OS preference flips so "system" mode tracks live.
  watch(osPrefersDark, () => {
    if (mode.value === "system") applyToDocument();
  });

  function setMode(next: ThemeMode): void {
    mode.value = next;
  }
  function setDensity(next: Density): void {
    density.value = next;
  }
  function setAccent(next: AccentHue): void {
    accent.value = { ...next };
  }
  function setAccentHue(hue: number): void {
    accent.value = { ...accent.value, h: clampHue(hue) };
  }
  function setPrDetailSurface(next: PrDetailSurface): void {
    prDetailSurface.value = next;
  }
  function setAccountScope(next: number | null): void {
    accountScope.value = next;
  }

  return {
    mode,
    density,
    accent,
    prDetailSurface,
    accountScope,
    hydrate,
    setMode,
    setDensity,
    setAccent,
    setAccentHue,
    setPrDetailSurface,
    setAccountScope,
  };
});
