import { ref } from "vue";

/**
 * Best-effort deep-link target for the conversation surface (ADR 0031, issue
 * #437). When a notification that carries a `unit_ref` (a review thread
 * `node_id`) is opened - via an in-app inbox row click or the toast
 * `notification://open-pr` replay - the open path records the target here. The
 * conversation surface, after its threads load, scrolls to / briefly
 * highlights the matching thread card and then clears the target.
 *
 * Module-level singleton ref rather than a Pinia store: the state is a single
 * transient pointer with no actions worth a store, and the producer (the
 * notification open paths) and the consumer (`PullRequestConversation`) live in
 * different component trees that don't share a prop seam.
 *
 * If the thread isn't present when the surface consumes the target (pruned /
 * closed / legacy row without a node_id), the consumer clears it and the open
 * degrades to just showing the PR - no error.
 */
const pendingThreadNodeId = ref<string | null>(null);

/**
 * Stable DOM id for a thread card, keyed on its GraphQL `node_id`. The list
 * renders this as the card's `:id` and the conversation surface resolves it
 * via `document.getElementById` to scroll. The node_id is base64-ish and may
 * carry `=`; CSS.escape isn't needed because we go through `getElementById`
 * (an exact-string lookup), not a selector.
 */
export function threadAnchorId(nodeId: string): string {
  return `thread-${nodeId}`;
}

export interface ThreadDeepLink {
  /** Record a thread `node_id` to scroll to on the next conversation load. */
  setPendingThread(nodeId: string | null): void;
  /** Read and clear the pending target. Returns `null` when none is set. */
  takePendingThread(): string | null;
}

export function useThreadDeepLink(): ThreadDeepLink {
  function setPendingThread(nodeId: string | null): void {
    pendingThreadNodeId.value =
      nodeId !== null && nodeId.length > 0 ? nodeId : null;
  }

  function takePendingThread(): string | null {
    const target = pendingThreadNodeId.value;
    pendingThreadNodeId.value = null;
    return target;
  }

  return { setPendingThread, takePendingThread };
}
