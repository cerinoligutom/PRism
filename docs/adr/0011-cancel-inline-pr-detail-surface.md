# 0011 — Cancel inline expansion as a third PR detail surface

- **Status:** Accepted
- **Date:** 2026-05-21
- **Issue:** [#88](https://github.com/cerinoligutom/PRism/issues/88)
- **Deciders:** @cerinoligutom

## Context

M3 (conversation depth) shipped two detail surfaces for the PR conversation content — a right-anchored drawer (default) and a dedicated detail route. A third surface, inline-expansion-within-the-list (per `docs/design/artboards/dashboard-expanded.html`), was scoped out of M3 and reserved as a `'inline'` value on the `prDetailSurface` settings selector. The contract (`docs/contracts/conversation-depth.md`) called for a "post-M3 follow-up host" that would activate the reserved value once implemented.

Post-launch, with drawer + route in production, the inline-expansion plan was reviewed and cancelled. This ADR records the cancellation so future agents reading the M3 contract don't treat the reserved value as in-flight work.

## Decision drivers

- **Cost vs demand.** Inline expansion was estimated by the M3 scoping discussion as roughly half the M3 frontend complexity — neighbour-row collapse, focus management across compressed siblings, list-virtualisation interaction, sync-during-expanded behaviour — for a UX that wasn't the default and had no user signal driving it.
- **The drawer + route cover the read-only PR detail need.** The drawer keeps list context (sidebar visible, dim overlay over the dashboard) and is ~80% of the content area after PR #87; the route gives a deep-linkable full-bleed view. There's no read-only scenario the inline surface enables that drawer / route don't.
- **Dead surface in the settings selector.** The reserved-but-disabled option, the setter guard rejecting `'inline'`, and the hydrate coercion combined to give every reader (UI, store, type) one more state to reason about — for a value that doesn't function.
- **A reserved option is not a free option.** Keeping `'inline'` on the type union spreads its weight across every switch / map / exhaustive match that ever needs to handle a `PrDetailSurface`, plus the documentation that has to keep explaining what it means.

## Considered options

1. **Keep `'inline'` reserved as documented.** Leave the disabled option, ship the follow-up later.
2. **Cancel `'inline'` and remove it from the type.** Two surfaces, no dead state.
3. **Ship a minimal inline host now.** Compress to a single-PR effort, accept the technical debt to honour the reservation.

## Decision

Go with **option 2**. Cancel the inline-expansion plan and remove `'inline'` from `PrDetailSurface`, the settings selector, the store guards, and the contract's "reserved" mentions.

If user feedback later asks for inline expansion, it can be re-introduced via a fresh ADR with its own design discussion. The drawer's host-agnostic content component (`PullRequestConversation.vue`) means a future inline host would still mount it without component rewrites — the architectural option is preserved even though the runtime option isn't.

## Consequences

### Positive

- One less state on the `PrDetailSurface` type union; one less option in the settings selector; one less guard in the appearance store; one less row in the contract's deferred-surfaces table.
- The disabled-state machinery in `AppearanceSettings.vue` (the `disabled` / `hint` fields on the surface-option type, the `:disabled` / `:aria-disabled` bindings, the `seg__btn--disabled` CSS) is removed because no current option needs it.
- The conversation-depth contract reads cleanly as "drawer + route" rather than "drawer + route + reserved-for-later inline".

### Negative

- Anyone who manually edited their localStorage to set `prDetailSurface = "inline"` will see it coerced back to `"drawer"` on next hydrate. The hydrate coercion in `appearance.ts` already handles this for the broader "any unknown value" case, so the migration is silent.
- If demand for inline does materialise, re-introducing it requires the design + storage + focus-management work the M3 scoping discussion already framed — not a free lunch.

### Neutral / follow-ups

- The `dashboard-expanded.html` artboard still ships in `docs/design/artboards/` as a historical reference for what was considered. No need to delete it.
- The `_format.ts` and StatusTimelineTab deferrals from `project_m3_done.md` are unrelated and remain valid M4-prep items.

## References

- Contract: [`docs/contracts/conversation-depth.md`](../contracts/conversation-depth.md) (updated alongside this ADR)
- M3-F issue [#74](https://github.com/cerinoligutom/PRism/issues/74) — where the reserved `'inline'` option was first wired into the settings selector
- ADR [0010](0010-conversation-depth-storage.md) — storage decisions that remain unchanged by this cancellation
- Artboard: [`docs/design/artboards/dashboard-expanded.html`](../design/artboards/dashboard-expanded.html) — the originally-considered inline UX
