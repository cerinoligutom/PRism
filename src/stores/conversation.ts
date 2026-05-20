import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

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
 * network round-trip. `invalidate(prId)` is provided so a future sync-cycle
 * subscriber can drop a stale entry; M3 doesn't wire that hook yet because
 * the open-once-then-close pattern is the v1 common case.
 */
export const useConversationStore = defineStore("conversation", () => {
  const cache = ref<Map<number, HydratedConversation>>(new Map());
  const loading = ref<Set<number>>(new Set());
  const errors = ref<Map<number, string>>(new Map());

  // In-flight promise per PR id so concurrent callers wait on the same fetch
  // rather than firing duplicate Tauri invocations.
  const inflight = new Map<number, Promise<HydratedConversation>>();

  async function load(pullRequestId: number): Promise<HydratedConversation> {
    const cached = cache.value.get(pullRequestId);
    if (cached !== undefined) return cached;

    const pending = inflight.get(pullRequestId);
    if (pending !== undefined) return pending;

    loading.value.add(pullRequestId);
    errors.value.delete(pullRequestId);

    const promise = invoke<HydratedConversation>("fetch_pr_conversation", {
      pullRequestId,
    })
      .then((result) => {
        cache.value.set(pullRequestId, result);
        return result;
      })
      .catch((err: unknown) => {
        errors.value.set(pullRequestId, formatError(err));
        throw err;
      })
      .finally(() => {
        loading.value.delete(pullRequestId);
        inflight.delete(pullRequestId);
      });

    inflight.set(pullRequestId, promise);
    return promise;
  }

  function invalidate(pullRequestId: number): void {
    cache.value.delete(pullRequestId);
    errors.value.delete(pullRequestId);
  }

  function clearError(pullRequestId: number): void {
    errors.value.delete(pullRequestId);
  }

  return {
    cache,
    loading,
    errors,
    load,
    invalidate,
    clearError,
  };
});

function formatError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "Couldn't load conversation.";
}
