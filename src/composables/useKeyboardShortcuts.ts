import { useEventListener } from "@vueuse/core";
import { useSyncStore } from "@/stores/sync";

/**
 * Global keyboard shortcuts mounted once at the app root.
 *
 * Currently handles Cmd/Ctrl+R for manual refresh. The Tauri webview reloads
 * the whole window on the default Cmd+R, dropping in-memory state, so we have
 * to `preventDefault` before invoking the sync.
 *
 * Match the modifier strictly per-platform: `metaKey` on macOS (Cmd) and
 * `ctrlKey` everywhere else. `metaKey` on Windows is the Windows key, which
 * we don't want firing the shortcut. The pattern mirrors `DashboardSearch.vue`.
 */
export function useKeyboardShortcuts(): void {
  const sync = useSyncStore();

  useEventListener(window, "keydown", (event: KeyboardEvent) => {
    if (event.key !== "r" && event.key !== "R") return;
    const isMacCombo = event.metaKey && !event.ctrlKey;
    const isNonMacCombo = event.ctrlKey && !event.metaKey;
    if (!isMacCombo && !isNonMacCombo) return;
    if (event.altKey || event.shiftKey) return;
    event.preventDefault();
    if (sync.aggregate === "syncing") return;
    void sync.refreshNow(null);
  });
}
