import { defineStore } from "pinia";
import { ref, watch } from "vue";

export type ThemeMode = "dark" | "light" | "system";
export type Density = "tight" | "comfortable" | "roomy";

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
}

const DEFAULT_STATE: PersistedState = {
  mode: "dark",
  density: "comfortable",
  accent: ACCENT_PRESETS.magenta!,
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

function prefersDark(): boolean {
  if (typeof window === "undefined") return true;
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
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

  function effectiveTheme(): "dark" | "light" {
    if (mode.value === "system") return prefersDark() ? "dark" : "light";
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
    };
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(payload));
  }

  function hydrate(): void {
    const stored = readPersisted();
    mode.value = stored.mode;
    density.value = stored.density;
    accent.value = { ...stored.accent };
    applyToDocument();

    // Always-on OS theme listener so switching to "system" mid-session also
    // picks up subsequent OS theme changes without requiring re-hydration.
    if (typeof window !== "undefined") {
      window
        .matchMedia("(prefers-color-scheme: dark)")
        .addEventListener("change", () => {
          if (mode.value === "system") applyToDocument();
        });
    }
  }

  watch([mode, density, accent], () => {
    applyToDocument();
    persist();
  }, { deep: true });

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

  return {
    mode,
    density,
    accent,
    hydrate,
    setMode,
    setDensity,
    setAccent,
    setAccentHue,
  };
});
