# 0019 - Error handling: per-store `lastError`, no toast for failures, self-healing reauth

- **Status:** Accepted
- **Date:** 2026-05-23
- **Issue:** [#287](https://github.com/cerinoligutom/PRism/issues/287)
- **Deciders:** @cerinoligutom

## Context

The codebase has grown a consistent error-handling pattern that was never written down. Two recent code reviews (one human-driven, one agent-driven) independently proposed "centralising" the pattern in ways that would have contradicted the existing intent:

- An `invokeWithToast(command, args)` wrapper that funnels every Tauri command failure into a toast.
- A `useReauthHandler` composable that invalidates dashboard / conversation caches when the worker emits `auth://reauth-required`.
- A workspace-level `CommandError` trait with `is_auth_error()` / `is_rate_limited()` / `is_network_error()` categorical predicates so cross-cutting code can route errors uniformly.

None of these match how the system actually works. Document the existing pattern so future reviewers (human or otherwise) don't re-propose the same centralisation.

## Decision drivers

- Errors should be visible to the user in the context that produced them; "did this action succeed?" is answered next to the action, not in a separate overlay.
- Toasts are an attention-getting affordance with a short lifespan. Reserving them for transient success keeps them meaningful.
- The categorisation that a cross-cutting trait would expose is not actually shared across modules; designing for it would force false uniformity.
- The reauth flow is already self-healing in the worker; adding cache invalidation on the frontend would *worsen* UX (an account expiring would suddenly hide its rows from the dashboard).

## Considered options

1. **Federated convention (current)** - each store owns `lastError`; UI panels read from the store; toasts are for success confirmations. The reauth flow self-heals via worker suspend/resume + the existing `phase: "synced"` listener.
2. **Centralised `invokeWithToast` wrapper** - every Tauri command call goes through one helper that catches errors and shows a toast.
3. **Cache invalidation on `auth://reauth-required`** - a global composable that calls `invalidate()` on dashboard / conversation stores when the event fires.
4. **Workspace-level `CommandError` trait** - every command-error enum implements `is_auth_error()` etc., so cross-cutting code can categorise without binding to specific enum types.

## Decision

We will keep **Option 1**. Explicitly reject options 2, 3, and 4.

Each store exposes `lastError: Ref<string | null>` (or a per-PR `errors: Map<id, string>` for conversation), populates it inside its actions' `catch` blocks, and surfaces it to the UI via standard ref binding. Panels render the error inline near the affected action - `RepositoriesSettings.vue:168-171` is the canonical example, complete with a comment explaining why the action doesn't fire a success toast on failure. Discriminated-union error mirrors (e.g. `ConversationCommandError` in `src/stores/conversation.ts:83-85`) match the shape of the Rust error enums one-for-one so the store can map `kind` to a user-facing message.

Toasts (`useToastStore().show(message, { variant: "success" })`) fire on confirmable wins (a repo toggled tracked, a link copied) and on local non-Tauri failures (clipboard refused). They do not fire on Tauri command failures - the store has already routed the error somewhere visible.

Reauth is handled entirely by the existing flow:

1. Worker hits 401 → suspends the per-account loop (`worker.rs:380-396`), sets `state.phase = Unauthorized`, calls `reauth.notify()` which emits `auth://reauth-required`.
2. `AccountsPanel.vue:131` listens for the event and adds the account id to its `expiredAccountIds` set, rendering a danger callout.
3. User opens `ReauthDialog`, supplies a fresh PAT, the auth command updates the keychain and nudges the worker.
4. Worker resumes the per-account loop; the next successful cycle emits `phase: "synced"`.
5. `dashboard.ts` already listens for `phase: "synced"` and reloads the list. No additional invalidation needed.

The reason we don't have a `CommandError` trait: three of the six command-error enums in `src-tauri/src/*/commands.rs` read locally from SQLite and never touch GitHub. They have only `NotFound` and `Internal` variants. A trait of categorical predicates would force these to return `false` from `is_auth_error()` / `is_rate_limited()` / `is_network_error()`, which is the "always-false boolean" smell of a premature abstraction. The categorical layer is already where it belongs: `github::GitHubError` carries `Unauthorized` / `RateLimited` / `Network` and the modules that need them convert via `impl From<GitHubError>`.

## Consequences

### Positive

- Errors are visible in the action's own context. Users can copy a message into a bug report without racing a 3-second toast.
- Success toasts stay meaningful; they're not crowded out by failures.
- New stores follow an obvious template (`lastError` + `clearError`) without inventing a private convention.
- The reauth flow handles itself; new surfaces inherit the self-heal automatically just by listening to `phase: "synced"`.

### Negative

- A future surface that legitimately wants cross-cutting error logic (e.g. structured metrics) has to bind to each error enum individually. If a real third consumer appears (per the three-uses rule), revisit and extract a trait keyed to *actual* shared variants.
- The "I need to be told the action failed" UX is implicit (panel renders the message). A user who closes the panel before reading the error loses it. Acceptable given the alternative (toast) is even more ephemeral.

### Neutral / follow-ups

- If the worker ever stops emitting `phase: "synced"` after reauth (regression risk), the dashboard would stay stale. Covered by the existing sync-worker test suite; not a separate guard.
- If a future ADR adopts ts-rs / typeshare (see the typeshare spike), generated TypeScript will mirror the per-module error enums automatically; the convention here doesn't change.

## References

- `src/stores/conversation.ts:83-85` - example of a per-store discriminated-union error mirror.
- `src/views/settings/RepositoriesSettings.vue:168-171` - inline comment documenting the "no toast on failure" rule.
- `src-tauri/src/sync/worker.rs:380-396` - reauth suspend/resume implementation.
- `src/views/settings/AccountsPanel.vue:131` - the single consumer of `auth://reauth-required`.
- ADR 0005 - PAT auth and keychain storage.
- ADR 0017 - Desktop notifications (decision 5 references the same toast policy on the notify side).
