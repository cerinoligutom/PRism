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
 *
 * `read_at` is nullable; NULL means unread (ADR 0028 decision 3).
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
  /** Unix seconds the row was marked read; `null` while unread. */
  readonly read_at: number | null;
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
 *
 * Read/unread state (issue #379): `markRead(id)` and `markAllRead()` stamp
 * `read_at` server-side and mutate the local list in place. `unreadCount`
 * is computed from the list when the inbox view is mounted; the sidebar
 * chip reads it via `loadUnreadCount()` so it stays accurate without
 * having to load the full list.
 */
export const useNotificationsStore = defineStore("notifications", () => {
  const list = ref<readonly Notification[]>([]);
  const loading = ref(false);
  const lastError = ref<string | null>(null);
  const unreadCount = ref(0);

  const count = computed<number>(() => list.value.length);
  const isEmpty = computed<boolean>(() => list.value.length === 0);

  async function load(): Promise<void> {
    loading.value = true;
    lastError.value = null;
    try {
      const rows = await invoke<Notification[]>("list_notifications", {
        limit: null,
        beforeId: null,
      });
      list.value = rows;
      unreadCount.value = rows.reduce(
        (acc, n) => (n.read_at === null ? acc + 1 : acc),
        0,
      );
    } catch (err) {
      lastError.value = formatError(err);
    } finally {
      loading.value = false;
    }
  }

  async function loadUnreadCount(): Promise<void> {
    try {
      unreadCount.value = await invoke<number>("unread_notification_count");
    } catch (err) {
      // Counts are advisory - on failure the chip drops to zero rather
      // than holding a stale signal. The lastError surface stays empty so
      // the inbox view itself doesn't flag a banner over a sidebar miss.
      console.warn("notifications.loadUnreadCount failed", err);
      unreadCount.value = 0;
    }
  }

  async function deleteOne(id: number): Promise<void> {
    lastError.value = null;
    try {
      await invoke<void>("delete_notification", { id });
      const removed = list.value.find((n) => n.id === id);
      list.value = list.value.filter((n) => n.id !== id);
      if (removed !== undefined && removed.read_at === null) {
        unreadCount.value = Math.max(0, unreadCount.value - 1);
      }
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
      unreadCount.value = 0;
    } catch (err) {
      lastError.value = formatError(err);
      throw new Error(lastError.value);
    }
  }

  async function markRead(id: number): Promise<void> {
    const target = list.value.find((n) => n.id === id);
    if (target === undefined || target.read_at !== null) return;
    // Optimistically mark locally so the row state updates the moment the
    // user clicks. The backend write is idempotent so a duplicate land
    // is a no-op; a failure logs but doesn't roll back - the next sync
    // cycle's `loadUnreadCount` reconciles.
    const now = Math.floor(Date.now() / 1000);
    list.value = list.value.map((n) =>
      n.id === id ? { ...n, read_at: now } : n,
    );
    unreadCount.value = Math.max(0, unreadCount.value - 1);
    try {
      await invoke<void>("mark_notification_read", { id });
    } catch (err) {
      console.warn("notifications.markRead failed", err);
    }
  }

  async function markAllRead(): Promise<void> {
    if (unreadCount.value === 0) return;
    const now = Math.floor(Date.now() / 1000);
    list.value = list.value.map((n) =>
      n.read_at === null ? { ...n, read_at: now } : n,
    );
    unreadCount.value = 0;
    try {
      await invoke<number>("mark_all_notifications_read");
    } catch (err) {
      lastError.value = formatError(err);
      console.warn("notifications.markAllRead failed", err);
    }
  }

  function clearError(): void {
    lastError.value = null;
  }

  return {
    list,
    loading,
    lastError,
    unreadCount,
    count,
    isEmpty,
    load,
    loadUnreadCount,
    deleteOne,
    clearAll,
    markRead,
    markAllRead,
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
