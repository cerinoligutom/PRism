import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { onBeforeUnmount, onMounted } from "vue";
import { useRouter } from "vue-router";

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
 * that event and pushes onto the existing `pr-detail` route.
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
    // Scope the dashboard to the originating account before the route push
    // so the back-navigation lands on a list that contains the deep-linked
    // row. `setAccountFilter` is a no-op when the filter already matches and
    // triggers a single `load()` otherwise; the load can race the route
    // push, which is fine - the detail view runs its own onMounted load.
    dashboard.setAccountFilter(payload.account_id);

    const meta = await resolveMetadata(payload);
    if (meta === null) return;

    void router.push({
      name: "pr-detail",
      params: { view: meta.view, id: meta.pull_request_id },
    });
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
