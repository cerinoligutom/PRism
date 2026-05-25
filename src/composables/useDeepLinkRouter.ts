import { invoke } from "@tauri-apps/api/core";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { openUrl } from "@tauri-apps/plugin-opener";
import { onBeforeUnmount, onMounted } from "vue";
import { useRouter } from "vue-router";

import {
  githubPrUrl,
  parsePrismDeepLink,
  type DeepLinkTarget,
  type PrCoordinates,
} from "@/lib/deepLinks";
import { useDashboardStore } from "@/stores/dashboard";

/**
 * Frontend half of the `prism://` deep-link surface (issue #339).
 *
 * The Tauri plugin emits a `deep-link://new-url` event each time a URL is
 * routed into the running app (macOS native, Linux + Windows via the
 * single-instance plugin's forwarding hook). It also queues the URL that
 * triggered a cold launch so `getCurrent()` returns it on first read; we
 * drain that on mount so the launch-from-link path lands the same way as
 * an `onOpenUrl` arrival.
 *
 * Routing:
 *  - Parse via `parsePrismDeepLink`. Unsupported URLs are dropped silently.
 *  - Resolve `(host, owner, repo, number)` against the local cache via the
 *    `pr_lookup_by_coordinates` Tauri command.
 *  - Hit: open the PR through the active detail surface (drawer or route,
 *    per the appearance setting), aligning the dashboard view + account
 *    scope first so the back-navigation lands on the right list.
 *  - Miss: open the canonical GitHub URL through `tauri-plugin-opener` so
 *    an external link to a not-yet-tracked PR still lands somewhere useful.
 *
 * Lifecycle: bound from `App.vue`'s top-level setup so it survives every
 * route change. The unlisten handle is torn down on unmount; cold-start
 * drain runs once per mount.
 */

interface PrCoordinatesMatch {
  readonly account_id: number;
  readonly pull_request_id: number;
  readonly number: number;
  readonly owner: string;
  readonly name: string;
  readonly view: "authored" | "assigned" | "watching" | "archive";
}

interface PrLookupErrorPayload {
  readonly kind: "not_found" | "internal";
}

export function useDeepLinkRouter(): void {
  const router = useRouter();
  const dashboard = useDashboardStore();
  let unlisten: (() => void) | null = null;

  onMounted(async () => {
    try {
      unlisten = await onOpenUrl((urls) => {
        for (const url of urls) {
          void handleRawUrl(url);
        }
      });
    } catch (err) {
      console.warn("prism://: failed to attach onOpenUrl listener", err);
    }

    try {
      const queued = await getCurrent();
      if (queued !== null) {
        for (const url of queued) {
          void handleRawUrl(url);
        }
      }
    } catch (err) {
      console.warn("prism://: getCurrent drain failed", err);
    }
  });

  onBeforeUnmount(() => {
    if (unlisten !== null) {
      unlisten();
      unlisten = null;
    }
  });

  async function handleRawUrl(raw: string): Promise<void> {
    const target = parsePrismDeepLink(raw);
    if (target === null) {
      // Unsupported URL shape (e.g. `prism://search?q=...` which is out of
      // scope for v1 per issue #339). Drop silently so external callers
      // sending malformed URLs don't surface a UI error.
      return;
    }
    await routeTarget(target);
  }

  async function routeTarget(target: DeepLinkTarget): Promise<void> {
    if (target.kind !== "pr") return;
    const coords = target.coords;

    const match = await lookupPr(coords);
    if (match === null) {
      await fallbackToBrowser(coords);
      return;
    }

    // Route through the shared dashboard store action so the active detail
    // surface (drawer vs route, per the appearance setting) decides the
    // target. The helper sets account scope and routes to the matching
    // dashboard host for the drawer branch.
    await dashboard.openPrFromExternal(
      {
        pullRequestId: match.pull_request_id,
        accountId: match.account_id,
        view: match.view,
      },
      router,
    );
  }

  async function lookupPr(coords: PrCoordinates): Promise<PrCoordinatesMatch | null> {
    try {
      return await invoke<PrCoordinatesMatch>("pr_lookup_by_coordinates", {
        host: coords.host,
        owner: coords.owner,
        repo: coords.repo,
        number: coords.number,
      });
    } catch (err) {
      if (isNotFound(err)) return null;
      console.warn(
        `prism://: lookup failed for ${coords.host}/${coords.owner}/${coords.repo}#${coords.number}`,
        err,
      );
      return null;
    }
  }

  async function fallbackToBrowser(coords: PrCoordinates): Promise<void> {
    const url = githubPrUrl(coords);
    try {
      await openUrl(url);
    } catch (err) {
      console.warn(`prism://: opener fallback failed for ${url}`, err);
    }
  }
}

function isNotFound(err: unknown): boolean {
  if (typeof err !== "object" || err === null) return false;
  const payload = err as Partial<PrLookupErrorPayload>;
  return payload.kind === "not_found";
}
