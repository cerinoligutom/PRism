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
 * `load(prId)` is now a synchronous DB read (`load_pr_conversation`). The
 * store caches the hydrated payload across re-mounts and refreshes each
 * visible PR on every completed sync cycle so new bodies, resolved flags,
 * and threads bar bucket shifts show up without the user closing the drawer.
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
     * cache entry. Suppresses the `errors` map write so a transient failure
     * doesn't replace the visible conversation with an error overlay.
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
      const result = await invoke<HydratedConversation>("load_pr_conversation", {
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
