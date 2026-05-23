// `navigator.platform` is deprecated but is the only API Tauri's embedded
// webview reports consistently across macOS, Windows, and Linux.
export type ModifierGlyph = "⌘" | "Ctrl";

export function usePlatformModifier(): ModifierGlyph {
  return /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl";
}
