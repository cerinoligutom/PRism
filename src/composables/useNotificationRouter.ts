import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { onBeforeUnmount, onMounted } from "vue";
import { useRouter } from "vue-router";

import { useThreadDeepLink } from "@/composables/useThreadDeepLink";
import { useDashboardStore } from "@/stores/dashboard";

/**
 * Frontend half of the notification click-to-open contract (ADR 0017
 * decision 4, issue #201).
 *
 * The Rust sink enqueues each dispatched notification's payload before the
 * toast fires. On every desktop OS, clicking a toast activates the source
 * app, which surfaces as a `WindowEvent::Focused(true)` event on the main
 * window. The Tauri side drains the pending queue on that focus event and
 * emits `notification://open-pr` per payload. This composable listens on
 * that event and routes through the shared `dashboard.openPrFromExternal`
 * action, which honours the active detail surface (drawer or route) from
 * the appearance store (issue #410).
 *
 * The plugin's desktop v2.3.3 surface ships no per-notification or global
 * click callback, so the focus-driven replay is the contract-faithful path.
 * The Rust queue applies a TTL so a focus event past the window doesn't
 * replay a stale payload.
 *
 * Cold start: queued events fire after the listener mounts because Tauri's
 * event channel buffers per-source. Mounting in `App.vue:onMounted` is
 * early enough for the payload that triggered the cold launch to land here.
 */
const NOTIFICATION_OPEN_PR_EVENT = "notification://open-pr";

interface NotificationOpenPrPayload {
  readonly account_id: number;
  readonly pull_request_id: number;
  // ADR 0031/0033: the toast threads the conversation unit or role obligation
  // it points at. A `'thread'` unit's `unit_ref` (the review thread `node_id`)
  // drives the deep-link scroll on the conversation surface; routing + open
  // clears the unit's watermark via the conversation auto-mark-seen. The
  // `'general'` / `'review'` units and the role kinds (`'review_request'` |
  // `'changes_requested'`) carry no anchor and just open the PR.
  readonly unit_kind?:
    | "thread"
    | "general"
    | "review"
    | "review_request"
    | "changes_requested"
    | null;
  readonly unit_ref?: string | null;
  readonly deep_link_url?: string | null;
}

interface PrRouteMetadata {
  readonly pull_request_id: number;
  readonly number: number;
  readonly owner: string;
  readonly name: string;
  readonly view: "authored" | "assigned" | "watching" | "archive";
}

export function useNotificationRouter(): void {
  const router = useRouter();
  const dashboard = useDashboardStore();
  const threadDeepLink = useThreadDeepLink();
  let unlisten: UnlistenFn | null = null;

  onMounted(async () => {
    try {
      unlisten = await listen<NotificationOpenPrPayload>(
        NOTIFICATION_OPEN_PR_EVENT,
        (event) => {
          void handleOpenPr(event.payload);
        },
      );
    } catch (err) {
      console.warn(
        `${NOTIFICATION_OPEN_PR_EVENT}: failed to attach listener`,
        err,
      );
    }
  });

  onBeforeUnmount(() => {
    if (unlisten !== null) {
      unlisten();
      unlisten = null;
    }
  });

  async function handleOpenPr(
    payload: NotificationOpenPrPayload,
  ): Promise<void> {
    const meta = await resolveMetadata(payload);
    if (meta === null) return;

    // Record the thread deep-link target (ADR 0031) before routing so the
    // conversation surface scrolls to the exact thread once its threads load.
    // Only a `'thread'` unit carries a scrollable anchor.
    threadDeepLink.setPendingThread(
      payload.unit_kind === "thread" ? payload.unit_ref ?? null : null,
    );

    // Route through the shared dashboard store action so the active detail
    // surface (drawer vs route, per the appearance setting) decides the
    // target. The helper sets account scope before navigating so the
    // back-navigation lands on a list that contains the row.
    await dashboard.openPrFromExternal(
      {
        pullRequestId: meta.pull_request_id,
        accountId: payload.account_id,
        view: meta.view,
      },
      router,
    );
  }

  async function resolveMetadata(
    payload: NotificationOpenPrPayload,
  ): Promise<PrRouteMetadata | null> {
    // Cache hit: the dashboard store already carries the row (the active
    // view contains it). In the cache-hit path we still need the view name
    // for the route - pick from the relation flags client-side would
    // duplicate the Rust resolver. Cheaper to round-trip the resolver, which
    // also handles the cache-miss case uniformly.
    try {
      return await invoke<PrRouteMetadata>("get_pr_route_metadata", {
        accountId: payload.account_id,
        pullRequestId: payload.pull_request_id,
      });
    } catch (err) {
      // The notification arrived for a PR that no longer has a relation row
      // visible to this account (relation pruned, account removed). The
      // toast can't usefully route; log and drop. The in-app badge keeps
      // doing its job either way.
      console.warn(
        `notification://open-pr: resolve failed for pr=${payload.pull_request_id} account=${payload.account_id}`,
        err,
      );
      return null;
    }
  }
}
