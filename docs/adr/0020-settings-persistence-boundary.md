# 0020 - Settings persistence: SQLite for worker-visible state, localStorage for device-local UI prefs

- **Status:** Accepted
- **Date:** 2026-05-23
- **Issue:** [#287](https://github.com/cerinoligutom/PRism/issues/287)
- **Deciders:** @cerinoligutom

## Context

User-affecting settings live in two places:

- `app_settings` singleton row in SQLite (`src-tauri/src/settings/types.rs`): notification preferences, sync interval, OS notification permission state.
- `prism:appearance:v1` JSON blob in `localStorage` (`src/stores/appearance.ts`): theme mode, density, accent hue, PR detail surface, dashboard account scope.

The split has been consistent since M2 but was never documented. A recent architectural review questioned why `accountScope` (logically "where the user wants the dashboard pointed") sits in localStorage rather than the database. Without a written rule, future contributors will keep relitigating the boundary.

## Decision drivers

- The background sync worker (Rust) needs to read some settings from inside Tauri commands or from its own polling loop. Those have to live where the worker can reach them - the database.
- UI preferences are read on every render; routing them through Tauri commands adds IPC churn for state that has no Rust-side consumer.
- localStorage survives across app restarts on the same device but is wiped by "Clear application data" and is not backed up. Acceptable for prefs that can be re-derived from defaults; not acceptable for state the user expects to migrate (e.g. they exported a backup and restored it on a new machine).
- The notifications subsystem (ADR 0017) already pinned its preferences in SQLite for the same worker-visibility reason. The pattern was set; this ADR generalises it.

## Considered options

1. **All settings in SQLite** - one canonical store. Every UI pref read becomes a Tauri command.
2. **All settings in localStorage** - cheap reads everywhere. Rust reads have to take a roundtrip via a frontend-supplied value (or duplicate state in the database).
3. **Split by reader (current)** - SQLite for anything the worker / Rust commands consume; localStorage for UI-only prefs.

## Decision

We will keep **Option 3**. The boundary is reader-driven:

**SQLite (`app_settings` row, Rust-owned)**
- `notifications_enabled`, `notify_on_needs_attention`, `notify_on_mention` - the notification sink reads these inside `dispatch_or_skip` on every potential dispatch.
- `notification_permission_state` - the sink checks this before any OS call.
- `sync_interval_seconds` - the scheduler reads this on every tick.
- Anything future that the worker / commands need server-side.

**localStorage (`prism:appearance:v1`, Vue-owned)**
- `mode` (`"dark" | "light" | "system"`), `density`, `accent`, `prDetailSurface` - read on every render; no Rust consumer.
- `accountScope` - the dashboard's currently-pinned scope. Read every dashboard load on the frontend; Rust receives it as a parameter to `list_dashboard_pull_requests`, not as ambient state.

Rule of thumb when adding a new setting: if the worker, the notification sink, or any `#[tauri::command]` needs to read it without the frontend supplying it as an argument, the setting lives in SQLite. Otherwise localStorage.

## Consequences

### Positive

- The worker doesn't have to invoke the frontend to discover its own polling cadence or notification gates.
- UI preference reads stay cheap (no IPC), and writes don't have to cross the Tauri boundary.
- Each storage layer has obvious ownership: anything in `localStorage` is a Vue concern, anything in `app_settings` is a Rust concern.

### Negative

- `accountScope` doesn't survive a backup-and-restore to a new device. The user re-selects their preferred view once. Acceptable.
- A future setting that's read by *both* the worker and the UI has to choose a side. By convention, choose SQLite and let the frontend `invoke` to read it - the IPC cost on the UI side is the lesser evil compared to duplicating writeable state across both layers.
- A clean wipe ("Clear application data" on macOS, equivalent on Windows / Linux) flushes only the localStorage half. Users may not realise their theme reverted to default while their notification prefs survived. Mention this in the settings panel copy if it ever surfaces a complaint.

### Neutral / follow-ups

- If we add device-roaming preferences (e.g. signed-in users syncing their settings to a server), revisit. Both layers become local mirrors and the canonical store moves elsewhere.
- A clean-wipe affordance in the app would touch both layers; out of scope for v1.

## References

- ADR 0003 - Local storage: SQLite for app state.
- ADR 0017 - Desktop notifications (decision 5 stores permission state in `app_settings`).
- ADR 0016 - Unified multi-account dashboard (defines `accountScope` semantics).
- `src-tauri/src/settings/types.rs:43-79` - the `AppSettings` struct and loader.
- `src/stores/appearance.ts:30-138` - the appearance store and its `PersistedState` shape.
