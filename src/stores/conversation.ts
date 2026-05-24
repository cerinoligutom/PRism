import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { useTauriListener } from "@/composables/useTauriListener";
import type { HydratedConversation } from "@/types/conversation";

/**
 * Per-PR conversation cache for the drawer / route / future inline expansion.
 *
 * Mirrors the contract in `docs/contracts/conversation-depth.md` (section
 * "Pinia store"). `load(prId)` invokes the `fetch_pr_conversation` Tauri
 * command, caches the hydrated result, and de-duplicates concurrent mounts
 * (e.g. drawer + status-bar prefetch racing on the same PR).
 *
 * Re-opens within the same session re-render from the cache without a new
 * network round-trip. `invalidate(prId)` drops a stale entry; `bind()`
 * subscribes to `sync://status` so a completed cycle invalidates non-visible
 * entries and refreshes the visible one inline (issue #337).
 */

interface SyncStatusEvent {
  readonly account_id: number;
  readonly phase: string;
}

const SYNC_STATUS_EVENT = "sync://status";

export const useConversationStore = defineStore("conversation", () => {
  const cache = ref<Map<number, HydratedConversation>>(new Map());
  const loading = ref<Set<number>>(new Set());
  const errors = ref<Map<number, string>>(new Map());

  // Per-PR fetch promise so concurrent callers wait on the same Tauri
  // invocation. Persists for the session's lifetime, keyed by PR id.
  const pendingLoads = new Map<number, Promise<HydratedConversation>>();

  // Ref-counted set of PR ids the UI is currently rendering. A `synced`
  // sync-status event refreshes any visible PR inline (overwrite-on-arrival,
  // no cache eviction) so the drawer / route surface doesn't flicker through
  // a loading state. Non-visible cached PRs simply drop on the same signal.
  const visibleRefCounts = new Map<number, number>();

  const listener = useTauriListener();

  async function load(pullRequestId: number): Promise<HydratedConversation> {
    const cached = cache.value.get(pullRequestId);
    if (cached !== undefined) return cached;

    const existing = pendingLoads.get(pullRequestId);
    if (existing !== undefined) return existing;

    return dispatchFetch(pullRequestId, { background: false });
  }

  interface DispatchOptions {
    /**
     * `true` when the fetch is a sync-cycle refresh of an already-rendered
     * cache entry. Suppresses the `errors` map write so a transient fetch
     * failure doesn't replace the visible conversation with an error overlay;
     * the stale-but-rendered payload survives until the next cycle succeeds.
     * Loading-set and pending-promise book-keeping still applies so concurrent
     * user-initiated `load()` calls coalesce onto the in-flight promise.
     */
    readonly background: boolean;
  }

  function dispatchFetch(
    pullRequestId: number,
    options: DispatchOptions,
  ): Promise<HydratedConversation> {
    loading.value.add(pullRequestId);
    if (!options.background) {
      errors.value.delete(pullRequestId);
    }

    const promise = invoke<HydratedConversation>("fetch_pr_conversation", {
      pullRequestId,
    })
      .then((result) => {
        cache.value.set(pullRequestId, result);
        return result;
      })
      .catch((err: unknown) => {
        if (!options.background) {
          errors.value.set(pullRequestId, formatError(err));
        }
        throw err;
      })
      .finally(() => {
        loading.value.delete(pullRequestId);
        pendingLoads.delete(pullRequestId);
      });

    pendingLoads.set(pullRequestId, promise);
    return promise;
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
   * unmount. A non-zero refcount means a completed sync cycle re-hydrates the
   * cache entry inline instead of evicting it, so the drawer / route surface
   * never flips through a loading state when fresh data lands.
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
   * Handle a completed sync cycle. The `sync://status` payload doesn't list
   * which PRs the cycle touched, so we conservatively re-hydrate every cached
   * entry: visible PRs refresh inline (overwrite on arrival, no flicker);
   * non-visible cached PRs drop so the next open re-fetches lazily.
   *
   * The pending-load guard skips PRs currently mid-fetch - the in-flight
   * promise already lands the freshest data.
   */
  function handleSyncedCycle(): void {
    const cachedIds = Array.from(cache.value.keys());
    for (const id of cachedIds) {
      if (pendingLoads.has(id)) continue;
      if (visibleRefCounts.has(id)) {
        void dispatchFetch(id, { background: true }).catch(() => {
          // Background refresh failures are swallowed: the previous payload
          // is still in `cache`, so the UI keeps rendering it. The next cycle
          // gets another attempt; the user-driven kebab Retry path stays as
          // the escape hatch for a persistent failure.
        });
      } else {
        invalidate(id);
      }
    }
  }

  async function bind(): Promise<void> {
    await listener.bind(() =>
      Promise.all([
        listen<SyncStatusEvent>(SYNC_STATUS_EVENT, (event) => {
          if (event.payload.phase === "synced") {
            handleSyncedCycle();
          }
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
 * `src-tauri/src/conversation/commands.rs`. The shape comes from
 * `#[serde(tag = "kind", rename_all = "snake_case")]`.
 */
type ConversationCommandError =
  | { kind: "not_found" }
  | { kind: "internal" };

/**
 * Translates the structured Rust error into a single user-facing message.
 * Mirrors `formatAuthError` in `src/stores/accounts.ts`. Falls back to the
 * generic conversation message when the payload isn't one of the kinds we
 * know about so a future variant doesn't render the raw object.
 */
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
