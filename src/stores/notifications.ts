import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { useTauriListener } from "@/composables/useTauriListener";

const DASHBOARD_REFRESH_EVENT = "dashboard://refresh";

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
 * ADR 0031 narrows `read_at` to the orphan-row fallback (a live row's unread
 * state is derived per-row against its own unit watermark) and adds the
 * derived per-row `unread` flag plus the conversation-unit reference. Trust
 * `unread`, not `read_at === null`, when rendering the read/unread cue and
 * counting the chip.
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
  /** Unix seconds the row was marked read. ADR 0031: meaningful only for an
   * orphan row (`pull_request_id === null`); a live row's unread state is
   * derived into `unread`. */
  readonly read_at: number | null;
  /** Conversation unit this row points at (ADR 0031): `'thread'` |
   * `'general'` | `null` (legacy / PR-level row). */
  readonly unit_kind: "thread" | "general" | null;
  /** Review thread `node_id` when `unit_kind === 'thread'`, else `null`. */
  readonly unit_ref: string | null;
  /** Deep link to the exact unit (thread url or PR conversation url). */
  readonly deep_link_url: string | null;
  /** Derived per-row unread flag (ADR 0031). A live row is unread iff its own
   * unit still needs the viewer; an orphan row is unread iff `read_at` is
   * null. Computed server-side by `notifications::store::list`. */
  readonly unread: boolean;
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
 * Read/unread state (ADR 0031): a row's unread state is the backend-derived
 * `unread` flag, not `read_at === null`. `load()` sets `unreadCount` by
 * summing that flag (the same value `unread_notification_count` returns), so
 * the chip and the dock badge agree by construction. `markRead(id)` /
 * `markAllRead()` flip the flag locally for snappiness; the next refresh
 * trusts the backend.
 *
 * Live refresh (issue #437): the dispatch hook writes inbox rows during a
 * sync cycle and the per-unit mark-seen path advances watermarks, both of
 * which change derived read-state. `bind()` subscribes to `dashboard://refresh`
 * so a mounted inbox + the sidebar chip live-update mid-session instead of
 * only on mount.
 */
export const useNotificationsStore = defineStore("notifications", () => {
  const list = ref<readonly Notification[]>([]);
  const loading = ref(false);
  const lastError = ref<string | null>(null);
  const unreadCount = ref(0);

  const listener = useTauriListener();

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
      // ADR 0031: the chip mirrors the backend-derived count. Summing the
      // per-row `unread` flag equals `unread_notification_count`, so we get
      // it from the rows already in hand without a second round-trip.
      unreadCount.value = rows.reduce((acc, n) => (n.unread ? acc + 1 : acc), 0);
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
      if (removed !== undefined && removed.unread) {
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
    if (target === undefined || !target.unread) return;
    // Optimistically flip the derived `unread` cue locally so the row state
    // updates the moment the user clicks. ADR 0031: a LIVE row's unread state
    // is derived from its unit watermark, so the next `dashboard://refresh`
    // re-lights it unless the click also advanced the watermark (the open
    // path's `auto_mark_units_seen` does). The `mark_notification_read`
    // command only stamps `read_at` for orphan rows; it's a no-op for live
    // ones. We trust the backend on the next refresh either way.
    const now = Math.floor(Date.now() / 1000);
    list.value = list.value.map((n) =>
      n.id === id ? { ...n, unread: false, read_at: n.read_at ?? now } : n,
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
      n.unread ? { ...n, unread: false, read_at: n.read_at ?? now } : n,
    );
    unreadCount.value = 0;
    try {
      await invoke<number>("mark_all_notifications_read");
    } catch (err) {
      lastError.value = formatError(err);
      console.warn("notifications.markAllRead failed", err);
    }
  }

  /**
   * Subscribe to `dashboard://refresh` so a mounted inbox + the sidebar chip
   * live-update mid-session. The sync worker emits this at the end of each
   * cycle (after writing new inbox rows and recomputing watermarks); triage
   * commands emit it on commit. Reloads the full list when the inbox view is
   * mounted (rows present), otherwise refreshes only the chip count.
   */
  async function bind(): Promise<void> {
    await listener.bind(() =>
      Promise.all([
        listen(DASHBOARD_REFRESH_EVENT, () => {
          if (list.value.length > 0) {
            void load();
          } else {
            void loadUnreadCount();
          }
        }),
      ]),
    );
  }

  function unbind(): void {
    listener.unbind();
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
    bind,
    unbind,
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
