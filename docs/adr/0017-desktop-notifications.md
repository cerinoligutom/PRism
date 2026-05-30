# 0017 - Desktop notifications: triggers, app-wide preferences, macOS-only dock badge, deferred permission prompt

- **Status:** Accepted; superseded in part by [ADR 0031](0031-conversation-unit-attention-and-rearm-dispatch.md) (2026-05-31)
- **Date:** 2026-05-22
- **Issue:** [#188](https://github.com/cerinoligutom/PRism/issues/188)
- **Deciders:** @cerinoligutom

> **Superseded in part by [ADR 0031](0031-conversation-unit-attention-and-rearm-dispatch.md) (2026-05-31).** The dock badge counts the conversation-unit roll-up (not `needs_attention` as originally framed, and not the interim unread predicate the M6 cut moved to). The two trigger toggles collapse to a single `notify_on_needs_attention` that is now actually read by `decide_dispatch` (it was dead). Dispatch is edge-with-re-arm rather than pure edge. The original decisions below stand as the historical record.

## Context

M6 turns the in-app attention signals (M4 sidebar dots, the `needs_attention` composite from ADR 0015, the `mentioned_count_unread` counter) into OS-level notifications and finishes the badge story. The pieces already in place:

- `recompute_needs_attention` writes `pull_request_viewer_relations.needs_attention` and `mentioned_count_unread` on every sync cycle and on mark-read transitions.
- `SidebarNav.vue:43-99` aggregates `needs_attention` counts per view and renders attention dots on the four sidebar entries.
- The sync worker emits `sync://status` per phase change; nothing else is emitted PR-by-PR.

What is missing:

- No `tauri-plugin-notification` dependency, capability grant, or `notify` command wrapper.
- No event in the sync worker that says "PR X newly crossed into the attention bucket" or "a new unresolved mention landed". Toasts need that diff.
- No preferences store; no UI to opt in or out.
- No OS dock / taskbar badge on any platform.
- The Settings nav placeholder for Notifications is mislabelled "Lands in M5".

Five sub-decisions need pinning before the parallel implementation issues fan out:

1. **Trigger taxonomy.** Which events fire a desktop toast?
2. **Pref storage shape.** Where do preferences live - new table, JSON blob on a settings row, or columns on an existing row?
3. **OS badge platform scope.** Which OSes get a dock / taskbar badge in v1?
4. **Click-to-open contract.** How does a toast click land the user on the right PR?
5. **Permission lifecycle.** When do we ask the OS for notification permission?

## Decision drivers

- **Signal-to-noise.** A toast every sync cycle would be quickly muted by the user. M6 has to bias toward under-firing rather than over-firing; the in-app badge is the always-on signal, the toast is the spike-detector.
- **Single source of truth for "what changed".** The recompute already runs in one place; firing notifications from there avoids reproducing the formula in a second consumer.
- **Cross-platform parity gaps are real.** Tauri's `set_badge_count` exists on macOS; Windows taskbar overlay icons need a custom image per state; Linux varies by desktop environment. Promising a feature everywhere when only one platform delivers it cleanly is worse than naming the gap up front.
- **Minimal preference surface.** Per-account, per-view, per-trigger fine-grained toggles would let users mute everything by accident. A master switch plus the two trigger toggles covers v1's needs.
- **Permission UX.** Asking for OS notification permission on first launch (before the user has seen any PR) is the worst time - they have no context for what the toasts will be about. Deferring the ask to the first firing event is more legible.
- **Reuse ADR 0015's pattern.** The recompute helper already owns the attention / mention transition signals; the notification trigger is the natural consumer.

## Considered options

### Trigger taxonomy

1. **`needs_attention` 0->1 flip + `mentioned_count_unread` increment.** Two signals, fired from inside `recompute_needs_attention` after a transition is observed. Lines up with the in-app badge logic.
2. **Every sync cycle that touched data.** "10 PRs updated" toast per cycle. Noisy; rapidly muted.
3. **Every PR state change (any field).** Closest to GitHub's own notifications. Way too noisy without ML-grade filtering.
4. **Sync failed / reauth required only.** Conservative; misses the actual M6 brief.

### Pref storage shape

1. **New `app_settings` singleton row** with one BOOLEAN per pref. `CHECK (id = 1)` enforces singleton. Cheap migration, cheap reads, no JSON parsing.
2. **JSON TEXT blob on the same singleton row.** More forward-compatible (add prefs without migrations) but every read parses JSON; structured queries can't filter on prefs.
3. **Per-account `notification_prefs` columns on `accounts`.** Per-account control. Settings UI becomes a matrix. Over-engineered for v1.
4. **In-memory + Tauri config file.** Survives restart via the file, but two persistence stores split state.

### OS badge platform scope

1. **macOS only via `WebviewWindow::set_badge_count`.** Native, numeric. Windows and Linux: documented gap, post-v1 follow-up.
2. **macOS + Windows taskbar overlay icons.** Windows `SetOverlayIcon` takes a custom image, not a number - requires a sprite per state (1, 2, 3, ..., 9+). Real work, low payoff at v1 scale.
3. **macOS + Windows + Linux Unity launcher.** Linux Unity launcher is desktop-environment-specific (Unity, KDE Plasma, GNOME extension variations). Brittle.
4. **Skip OS badge entirely.** Sidebar dots already cover attention surfacing in-app. Skipping is defensible but loses the at-a-glance "PRism wants me" cue when the app isn't focused.

### Click-to-open contract

1. **Custom Tauri event from the notification action handler.** Frontend listens on a `notification://open-pr` channel, payload is `{ account_id, pr_node_id }`. Router pushes onto the detail surface. No URL scheme registration needed.
2. **Custom URL scheme `prism://pr/<account>/<pr_id>`.** Cross-platform deep-link via Tauri's `tauri-plugin-deep-link`. More machinery, opens the door to OS-level URL handlers (useful for "Open in PRism" extensions, post-v1).
3. **Window focus only; no specific PR routing.** Toast brings the app forward; user finds the PR themselves. Cheap; misses the natural affordance.

### Permission lifecycle

1. **Deferred-ask on first triggering event.** Don't prompt at launch. The first time a notification would fire, request permission. Persist the result and avoid re-asking.
2. **Ask on first launch (after the onboarding flow).** Predictable; users may say no before seeing any value.
3. **Ask when the Settings -> Notifications panel is first opened.** Tied to intent (they came to configure notifications). Best UX. Adds a button + state in the panel.
4. **Never ask; rely on the OS to prompt on first `notify()` call.** Lets the platform handle it. macOS does this automatically on first notification; Windows requires explicit permission acquisition through the plugin.

## Decision

### Trigger taxonomy - option 1

Fire on `needs_attention` 0->1 flip and on `mentioned_count_unread` increment, both detected inside `recompute_needs_attention` after the UPDATE runs. The recompute helper takes a `before` snapshot, runs its existing logic, then compares to the `after` row. If a transition fires the trigger, the helper hands a structured `NotificationTrigger { account_id, pull_request_id, kind }` to a new `notification_sink` trait (mirroring `ReauthSink` in `sync/worker.rs:135`), which the Tauri runtime implementation maps to a native toast respecting user prefs.

Rationale: the in-app badge is the always-on signal, so the toast carries spike-detection duty. The two triggers cover the legitimate "PRism wants my attention right now" cases without dragging in every PR field change. Sync failures and reauth surface in the status bar (`StatusBar.vue:81-103`) - no OS toast for those in v1.

### Pref storage shape - option 1 (new `app_settings` singleton row)

```sql
CREATE TABLE app_settings (
    id                              INTEGER PRIMARY KEY CHECK (id = 1),
    notifications_enabled           INTEGER NOT NULL DEFAULT 1,
    notify_on_needs_attention       INTEGER NOT NULL DEFAULT 1,
    notify_on_mention               INTEGER NOT NULL DEFAULT 1,
    notification_permission_state   TEXT    NOT NULL DEFAULT 'unprompted',
    updated_at                      INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
INSERT INTO app_settings (id) VALUES (1);
```

App-wide (not per-account). The master switch (`notifications_enabled`) defaults to ON alongside the two trigger toggles — the assumption is that a user who's added an account wants to know when PRs need them. The OS permission prompt fires the first time PRism actually dispatches a toast; `notification_permission_state` (`unprompted` / `granted` / `denied`) records the outcome so the UI can show the right call-to-action without re-asking the OS every time. A user who genuinely doesn't want notifications turns the master OFF in Settings; the deny path is one click away on first launch.

The original ADR shipped the master defaulting to OFF; the bump to ON happened pre-v1 launch after the in-app sidebar dots proved more than sufficient as a "noticed it without a toast" signal and the master-OFF default left first-time users wondering why nothing happened.

Rationale: the v1 surface is two boolean toggles plus a master switch. A JSON blob would buy forward-compat at the cost of typed reads from Rust; a new column is one migration. Per-account adds a settings matrix that we have no reason to ask v1 users to navigate.

### OS badge platform scope - option 1 (macOS only)

Use `tauri::WebviewWindow::set_badge_count` after each sync cycle. Count = total `pull_request_viewer_relations.needs_attention = 1` across in-scope accounts (the `accountFilter` from the dashboard store). Update is cheap: one query, one Tauri API call.

Windows and Linux: badge is not implemented in v1. The Settings -> Notifications panel notes the platform limitation in a small footer line. A post-v1 follow-up issue tracks Windows taskbar overlay icons and Linux Unity-launcher / KDE plasmoid integration. Implementation guards with `#[cfg(target_os = "macos")]` so the non-mac builds don't carry dead syscalls.

Rationale: shipping a numeric macOS badge cleanly is a one-day job; shipping correct cross-platform badges is a week of asset generation and platform-specific quirk-handling. The roadmap doesn't promise badge parity, and skipping it on macOS would drop the most natural at-a-glance affordance for the largest segment of the user base.

### Click-to-open contract - option 1 (custom Tauri event)

The notification handler in Rust emits `notification://open-pr` with `{ account_id: i64, pull_request_id: i64 }`. The frontend's `App.vue` (or a small `useNotificationRouter` composable) listens, focuses the main window via `appWindow.setFocus()`, and pushes onto the PR detail route. No new dependency, no URL-scheme registration, no OS handler registration.

Rationale: option 2's `prism://` scheme opens the door to OS-level "Open in PRism" use cases but requires `tauri-plugin-deep-link`, scheme registration in `tauri.conf.json`, and platform-specific quirks (Windows registry entries, macOS `LSHandlers` plist). Not worth it for the v1 in-app round-trip - the route already exists; the toast just needs to push onto it.

### Permission lifecycle - option 1 (deferred-ask on first triggering event)

The notification sink checks `notification_permission_state`:

- `unprompted`: call the plugin's `requestPermission()`, persist the result, then emit the notification if granted.
- `granted`: emit.
- `denied`: skip the OS toast, increment a `notification_skipped_count` counter for diagnostics, leave the in-app badge to do its job.

The Settings -> Notifications panel renders a "Notifications blocked" callout when the state is `denied`, with a one-liner pointing at the OS permission settings (mac: System Settings -> Notifications -> PRism; Windows: Settings -> System -> Notifications; Linux: notification daemon-specific). The callout doesn't re-prompt - re-prompting after denial is browser-grade dark-pattern territory.

Rationale: asking on launch before any sync has happened gives the user nothing to base a yes/no on. Asking when the panel opens (option 3) is also good and may move here in a follow-up if the deferred ask feels surprising; the deferred ask wins on minimal UI surface in v1.

## Consequences

### Positive

- One source of truth for triggers: `recompute_needs_attention` owns both the in-app badge state and the notification firing decision. The formula doesn't get reimplemented in a second consumer.
- App-wide prefs keep the Settings panel small (three toggles + a permission-state callout).
- macOS badge is cheap to ship and matches the most common dev platform; documented gap on other platforms is honest rather than aspirational.
- Deferred permission ask sidesteps the worst onboarding moment.

### Negative

- Per-account notification muting is not available in v1. Users with multiple noisy GHE accounts and one quiet personal account have to use the master switch. Post-v1 escalation path: add a `per_account_notify` column on `accounts` and a per-account override row in the panel.
- Quiet hours are not in this ADR (descoped to post-v1). A user who works late and gets pinged on weekends has to flip the master switch manually.
- macOS-only badge means Windows / Linux users still get sidebar dots but lose the dock-bouncing cue.
- The `notification://open-pr` event is internal-only - third parties can't deep-link into PRism. Acceptable for v1.

### Open follow-ups (not blocking M6)

- Post-v1: Windows taskbar overlay icons (sprite-per-state).
- Post-v1: Linux Unity launcher / KDE / GNOME variants.
- Post-v1: Quiet hours (TZ-aware schedule).
- Post-v1: Per-account notification overrides.
- Post-v1: `prism://` URL scheme if external integrations become a thing.

## Wiki sync

The Architecture page's [Notifications section](../wiki/Architecture.md#notifications) currently reads:

> Both desktop (native OS toasts) and in-app (badges). In-app badges are the default; toasts are per-event opt-in. Quiet hours suppress toasts.

The PR that implements ADR 0017 updates this section to:

- Replace "per-event opt-in" with "two opt-in toggles (needs attention, mentions) plus master switch". The per-event framing implies more granularity than v1 ships.
- Drop the "Quiet hours suppress toasts" sentence; quiet hours are post-v1.
- Add a one-liner noting macOS-only dock badge in v1.

The Wiki sync block in CONTRIBUTING.md flags this for republishing.
