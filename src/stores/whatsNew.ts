import { defineStore } from "pinia";
import { ref } from "vue";

/**
 * Manual-open signal for the in-app "What's new" dialog (ADR-0025).
 *
 * The dialog itself is hosted by `App.vue`, which gates the auto-open path on
 * `app_settings.last_seen_version` (the version cursor). After that cursor
 * advances on dismiss there is no in-app way to re-open the dialog, so the
 * About panel exposes a "View changelog" affordance that calls
 * `requestManualOpen()` here. The App-level host watches `requestCount` and
 * renders the dialog with the full bundled changelog (not the per-version
 * slice). On dismiss the host skips the cursor advance because the cursor's
 * job is "have you seen the most recent bump", not "have you ever looked at
 * the changelog".
 *
 * `requestCount` is a monotonic counter rather than a boolean so a second
 * request while the dialog is already open is still observable from the
 * host's `watch`. v1 only has one consumer (About panel), but the counter
 * shape keeps future call sites (a keybinding, a status-bar menu, etc.)
 * idempotent without coordination.
 */
export const useWhatsNewStore = defineStore("whatsNew", () => {
  const requestCount = ref(0);

  function requestManualOpen(): void {
    requestCount.value += 1;
  }

  return { requestCount, requestManualOpen };
});
