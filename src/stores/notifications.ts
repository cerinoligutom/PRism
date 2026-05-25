import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

/**
 * Mirror of `crate::notifications::types::Notification`. Keep this in
 * lock-step with the Rust struct (ADR 0021 - bindings stay manual through
 * v1).
 *
 * The snapshot fields (`owner`, `repo`, `pr_number`, `pr_node_id`,
 * `pr_title`) duplicate state already in `pull_requests` at insert time so
 * the row stays meaningful after a PR prune. `pull_request_id` is the soft
 * link the State-A click path uses to open the local detail surface; it
 * goes `null` when the source PR row is deleted.
 */
export interface Notification {
  readonly id: number;
  /** `needs_attention` or `mention`, serialised snake_case. */
  readonly kind: string;
  readonly account_id: number;
  readonly pull_request_id: number | null;
  readonly owner: string;
  readonly repo: string;
  readonly pr_number: number;
  readonly pr_node_id: string | null;
  readonly pr_title: string;
  readonly title: string;
  readonly body: string | null;
  /** Unix seconds. Newest first in the list. */
  readonly created_at: number;
}

/**
 * Inbox store backing `/dashboard/notifications`. Reads from
 * `list_notifications`, deletes per-row via `delete_notification`, and
 * wipes the whole list via `clear_all_notifications`. The OS toast pipeline
 * (ADR 0017) writes inbox rows from Rust, so this store is read-mostly
 * from the frontend's side.
 *
 * Local mutation on delete: once a delete command resolves successfully,
 * the deleted row is dropped from the in-memory list directly rather than
 * round-tripping the full list. The DB and the in-memory list move
 * together, and the next `load()` reconciles either way.
 */
export const useNotificationsStore = defineStore("notifications", () => {
  const list = ref<readonly Notification[]>([]);
  const loading = ref(false);
  const lastError = ref<string | null>(null);

  const count = computed<number>(() => list.value.length);
  const isEmpty = computed<boolean>(() => list.value.length === 0);

  async function load(): Promise<void> {
    loading.value = true;
    lastError.value = null;
    try {
      list.value = await invoke<Notification[]>("list_notifications", {
        limit: null,
        beforeId: null,
      });
    } catch (err) {
      lastError.value = formatError(err);
    } finally {
      loading.value = false;
    }
  }

  async function deleteOne(id: number): Promise<void> {
    lastError.value = null;
    try {
      await invoke<void>("delete_notification", { id });
      list.value = list.value.filter((n) => n.id !== id);
    } catch (err) {
      lastError.value = formatError(err);
      throw new Error(lastError.value);
    }
  }

  async function clearAll(): Promise<void> {
    if (list.value.length === 0) return;
    lastError.value = null;
    try {
      await invoke<void>("clear_all_notifications");
      list.value = [];
    } catch (err) {
      lastError.value = formatError(err);
      throw new Error(lastError.value);
    }
  }

  function clearError(): void {
    lastError.value = null;
  }

  return {
    list,
    loading,
    lastError,
    count,
    isEmpty,
    load,
    deleteOne,
    clearAll,
    clearError,
  };
});

function formatError(raw: unknown): string {
  if (typeof raw === "string") return raw;
  if (raw instanceof Error) return raw.message;
  if (typeof raw === "object" && raw !== null) {
    const maybe = raw as { kind?: string };
    if (typeof maybe.kind === "string") return `notifications.${maybe.kind}`;
  }
  return "Couldn't reach the notifications backend.";
}
