import { useEventListener } from "@vueuse/core";
import { useRoute } from "vue-router";
import { useSyncStore } from "@/stores/sync";
import { useDashboardStore } from "@/stores/dashboard";

/**
 * Global keyboard shortcuts mounted once at the app root.
 *
 * - `Cmd/Ctrl+R` triggers a manual refresh. The Tauri webview reloads the
 *   whole window on the default `Cmd+R`, dropping in-memory state, so we
 *   have to `preventDefault` before invoking the sync.
 * - `ArrowUp` / `ArrowDown` move focus through the dashboard rows; the
 *   highlight is the cue the row-targeted shortcuts use to pick a target.
 * - `E` archives the focused PR row from a default view, or unarchives it
 *   from the Archive view. No-op when no row is focused or while a sync
 *   cycle is mid-flight (mirrors the `Cmd+R` suppression so concurrent
 *   writes don't pile on the worker).
 *
 * Match the modifier strictly per-platform: `metaKey` on macOS (Cmd) and
 * `ctrlKey` everywhere else. `metaKey` on Windows is the Windows key, which
 * we don't want firing the shortcut. The pattern mirrors `DashboardSearch.vue`.
 */
export function useKeyboardShortcuts(): void {
  const sync = useSyncStore();
  const dashboard = useDashboardStore();
  const route = useRoute();

  useEventListener(window, "keydown", (event: KeyboardEvent) => {
    if (event.key === "r" || event.key === "R") {
      const isMacCombo = event.metaKey && !event.ctrlKey;
      const isNonMacCombo = event.ctrlKey && !event.metaKey;
      if (!isMacCombo && !isNonMacCombo) return;
      if (event.altKey || event.shiftKey) return;
      event.preventDefault();
      if (sync.aggregate === "syncing") return;
      void sync.refreshNow(null);
      return;
    }

    // Row-targeted shortcuts only fire on a dashboard route. Reading the
    // route meta keeps us from intercepting `E` / arrow keys on Settings
    // or onboarding screens, where they have no target.
    if (typeof route.meta?.dashboardView !== "string") return;
    // While the PR drawer is open the user is reading a single PR, not
    // navigating the list - hijacking arrow keys / E underneath the modal
    // would be jarring. Drawer dismissal is Esc (Reka default), after which
    // the shortcuts resume.
    if (dashboard.expandedPullRequestId !== null) return;
    // Bare-key shortcuts must not hijack input typing - the modifier shortcut
    // above is exempt because `Cmd+R` is a chord users always mean as a
    // refresh, regardless of focus.
    if (isEditableTarget(event.target)) return;
    if (hasModifier(event)) return;

    if (event.key === "ArrowDown") {
      event.preventDefault();
      dashboard.moveFocus(1);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      dashboard.moveFocus(-1);
      return;
    }

    if (event.key !== "e" && event.key !== "E") return;
    event.preventDefault();
    if (sync.aggregate === "syncing") return;
    const focusedId = dashboard.focusedPullRequestId;
    if (focusedId === null) return;
    const target = dashboard.filteredPullRequests.find((pr) => pr.id === focusedId);
    if (target === undefined) return;
    if (target.account_ids.length === 0) return;
    // Advance focus to the next visible row before the optimistic flip drops
    // the current one, so repeated `E` presses walk down the list instead of
    // stalling on a phantom id. Falls back to the previous row at the list
    // tail, and clears focus entirely if this was the only row.
    const order = dashboard.visibleRowIds;
    const idx = order.indexOf(focusedId);
    const nextId =
      idx === -1 || order.length <= 1
        ? null
        : idx + 1 < order.length
          ? (order[idx + 1] ?? null)
          : (order[idx - 1] ?? null);
    dashboard.setFocusedPullRequest(nextId);
    if (route.meta.dashboardView === "archive") {
      void dashboard.unarchive(target.id, target.account_ids);
    } else {
      void dashboard.archive(target.id, target.account_ids);
    }
  });
}

/**
 * Don't hijack keystrokes the user is aiming at a text input. Covers the
 * dashboard search, settings forms, and any future contenteditable surfaces.
 */
function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return target.isContentEditable;
}

function hasModifier(event: KeyboardEvent): boolean {
  return event.metaKey || event.ctrlKey || event.altKey;
}
