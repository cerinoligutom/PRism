import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { useTauriListener } from "@/composables/useTauriListener";
import type { HydratedConversation } from "@/types/conversation";

/**
 * Per-PR conversation cache for the drawer / route / future inline expansion.
 *
 * ADR 0029: sync owns `review_comments` / `issue_comments` persistence, so
 * `load(prId)` is now a synchronous DB read. The store caches the hydrated
 * payload across re-mounts and refreshes each visible PR on every completed
 * sync cycle so new bodies, resolved flags, and threads bar bucket shifts show
 * up without the user closing the drawer.
 *
 * ADR 0033 single-seam: a FOREGROUND open goes through `load_pr_conversation`,
 * which advances the read watermark and emits `dashboard://refresh`. A
 * BACKGROUND re-read (the `handleSyncedCycle` reaction to that very event) goes
 * through the non-mutating `read_pr_conversation` instead - if it called the
 * emitting `load_pr_conversation`, the emit would re-trigger `handleSyncedCycle`
 * and spin an infinite refresh loop.
 */

const DASHBOARD_REFRESH_EVENT = "dashboard://refresh";

export const useConversationStore = defineStore("conversation", () => {
  const cache = ref<Map<number, HydratedConversation>>(new Map());
  const loading = ref<Set<number>>(new Set());
  const errors = ref<Map<number, string>>(new Map());

  // PRs the drawer / route is rendering right now. A completed sync cycle
  // refreshes these inline; non-visible cached entries drop so the next open
  // re-reads from a fresh transaction. Ref-counted because the same PR can
  // be mounted twice (e.g. drawer + route during a navigation).
  const visibleRefCounts = new Map<number, number>();

  const listener = useTauriListener();

  async function load(pullRequestId: number): Promise<HydratedConversation> {
    const cached = cache.value.get(pullRequestId);
    if (cached !== undefined) return cached;
    return dispatchLoad(pullRequestId, { background: false });
  }

  interface DispatchOptions {
    /**
     * `true` when the call is a sync-cycle refresh of an already-rendered
     * cache entry. Two effects: suppresses the `errors` map write so a
     * transient failure doesn't replace the visible conversation with an error
     * overlay, AND routes to the non-mutating `read_pr_conversation` reader so
     * the refresh can't re-enter the emitting `load_pr_conversation` open path
     * (ADR 0033 single-seam loop break).
     */
    readonly background: boolean;
  }

  async function dispatchLoad(
    pullRequestId: number,
    options: DispatchOptions,
  ): Promise<HydratedConversation> {
    loading.value.add(pullRequestId);
    if (!options.background) {
      errors.value.delete(pullRequestId);
    }
    try {
      // Foreground open marks + emits `dashboard://refresh`; a background
      // re-read (driven by that event) must NOT re-emit, so it uses the
      // pure cache reader. See the store doc comment for the loop break.
      const command = options.background
        ? "read_pr_conversation"
        : "load_pr_conversation";
      const result = await invoke<HydratedConversation>(command, {
        pullRequestId,
      });
      cache.value.set(pullRequestId, result);
      return result;
    } catch (err) {
      if (!options.background) {
        errors.value.set(pullRequestId, formatError(err));
      }
      throw err;
    } finally {
      loading.value.delete(pullRequestId);
    }
  }

  function invalidate(pullRequestId: number): void {
    cache.value.delete(pullRequestId);
    errors.value.delete(pullRequestId);
  }

  function clearError(pullRequestId: number): void {
    errors.value.delete(pullRequestId);
  }

  /**
   * Advance the per-unit "seen" watermark for one review thread (ADR 0031),
   * fanned across every relation owner the viewer holds for the PR so a
   * unified-mode row settles uniformly - mirroring `auto_mark_units_seen`'s
   * fan-out on the Rust side.
   *
   * `mark_thread_seen` now emits `dashboard://refresh` on commit (#449), so the
   * canonical reconcile rides the shared listeners: this store's own
   * `handleSyncedCycle` re-reads the visible conversation, the dashboard store
   * reloads its rows, and the notifications store refreshes the inbox / chip.
   * This action only owns the optimistic local flip of `thread.unread` for
   * snappiness, plus a re-read when every invoke failed (no event fires then,
   * so nothing else rolls the optimistic flip back).
   */
  async function markThreadSeen(
    pullRequestId: number,
    accountIds: readonly number[],
    threadNodeId: string,
  ): Promise<void> {
    if (accountIds.length === 0) return;
    flipThreadUnreadOptimistically(pullRequestId, threadNodeId);
    await settleSeenInvokes(
      pullRequestId,
      accountIds.map((accountId) =>
        invoke("mark_thread_seen", { pullRequestId, accountId, threadNodeId }),
      ),
    );
  }

  /**
   * Advance the general-comment-stream "seen" watermark for the PR (ADR 0031),
   * fanned across relation owners. Companion to [`markThreadSeen`] for the
   * stream unit; same emit-driven reconcile via the shared listeners.
   */
  async function markGeneralStreamSeen(
    pullRequestId: number,
    accountIds: readonly number[],
  ): Promise<void> {
    if (accountIds.length === 0) return;
    await settleSeenInvokes(
      pullRequestId,
      accountIds.map((accountId) =>
        invoke("mark_general_stream_seen", { pullRequestId, accountId }),
      ),
    );
  }

  /**
   * Advance the reviews-stream "seen" watermark for the PR (ADR 0033), fanned
   * across relation owners. Companion to [`markGeneralStreamSeen`] for the
   * reviews unit (a formal review whose body @-mentions the viewer); same
   * emit-driven reconcile via the shared listeners. Lands unwired to UI in this
   * slice - the Reviews-tab "Mark all seen" affordance arrives in phase 4.
   */
  async function markReviewsSeen(
    pullRequestId: number,
    accountIds: readonly number[],
  ): Promise<void> {
    if (accountIds.length === 0) return;
    await settleSeenInvokes(
      pullRequestId,
      accountIds.map((accountId) =>
        invoke("mark_reviews_seen", { pullRequestId, accountId }),
      ),
    );
  }

  /**
   * Optimistically clear a single thread's `unread` cue in the cached
   * conversation so the card settles in the same paint as the click. The
   * `reconcileAfterSeen` re-read replaces it with canonical state.
   */
  function flipThreadUnreadOptimistically(
    pullRequestId: number,
    threadNodeId: string,
  ): void {
    const cached = cache.value.get(pullRequestId);
    if (cached === undefined) return;
    let touched = false;
    const threads = cached.threads.map((t) => {
      if (t.node_id !== threadNodeId || !t.unread) return t;
      touched = true;
      return { ...t, unread: false };
    });
    if (!touched) return;
    cache.value.set(pullRequestId, { ...cached, threads });
  }

  /**
   * Await the per-account seen invokes. On any success the backend emits
   * `dashboard://refresh`, and the shared listeners (conversation, dashboard,
   * notifications) reconcile every surface - so this only re-reads the
   * conversation when every invoke failed, rolling the optimistic flip back to
   * canonical state since no event fires on a total failure.
   */
  async function settleSeenInvokes(
    pullRequestId: number,
    invokes: readonly Promise<unknown>[],
  ): Promise<void> {
    const outcomes = await Promise.allSettled(invokes);
    const allFailed = outcomes.every((o) => o.status === "rejected");
    if (allFailed) {
      await dispatchLoad(pullRequestId, { background: true }).catch(() => {
        // Swallow: the cached payload (with the optimistic flip) keeps
        // rendering; the next sync cycle or a user retry reconciles it.
      });
    }
  }

  /**
   * Mark `pullRequestId` as visible in the UI. Pairs with `release(prId)` on
   * unmount. A non-zero refcount means a completed sync cycle re-reads the
   * cache entry in place instead of evicting it, so the drawer / route
   * surface never flips through a loading state when fresh data lands.
   */
  function acquire(pullRequestId: number): void {
    visibleRefCounts.set(pullRequestId, (visibleRefCounts.get(pullRequestId) ?? 0) + 1);
  }

  function release(pullRequestId: number): void {
    const next = (visibleRefCounts.get(pullRequestId) ?? 0) - 1;
    if (next <= 0) {
      visibleRefCounts.delete(pullRequestId);
    } else {
      visibleRefCounts.set(pullRequestId, next);
    }
  }

  /**
   * Re-read every cached entry after a sync cycle. Visible PRs refresh in
   * place; non-visible cached PRs drop so the next open hits a clean DB read.
   */
  function handleSyncedCycle(): void {
    for (const id of Array.from(cache.value.keys())) {
      if (visibleRefCounts.has(id)) {
        void dispatchLoad(id, { background: true }).catch(() => {
          // Swallow: the previous payload is still cached so the UI keeps
          // rendering it. The next cycle gets another attempt; the user-
          // driven kebab Retry path stays as the escape hatch.
        });
      } else {
        invalidate(id);
      }
    }
  }

  async function bind(): Promise<void> {
    await listener.bind(() =>
      Promise.all([
        // ADR 0029: one refresh signal across surfaces. Sync emits this at
        // the end of each successful cycle; triage commands emit it on commit.
        listen(DASHBOARD_REFRESH_EVENT, () => {
          handleSyncedCycle();
        }),
      ]),
    );
  }

  function unbind(): void {
    listener.unbind();
  }

  return {
    cache,
    loading,
    errors,
    load,
    invalidate,
    clearError,
    markThreadSeen,
    markGeneralStreamSeen,
    markReviewsSeen,
    acquire,
    release,
    bind,
    unbind,
  };
});

/**
 * Discriminated union mirroring `ConversationCommandError` in
 * `src-tauri/src/conversation/commands.rs`.
 */
type ConversationCommandError =
  | { kind: "not_found" }
  | { kind: "internal" };

function formatError(err: unknown): string {
  if (typeof err === "object" && err !== null && "kind" in err) {
    const tagged = err as ConversationCommandError;
    switch (tagged.kind) {
      case "not_found":
        return "This pull request is no longer available.";
      case "internal":
        return "Couldn't load conversation. Check the application logs.";
    }
  }
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "Couldn't load conversation.";
}
