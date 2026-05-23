# 0021 - Rust to TypeScript type bindings: stay manual through v1, revisit if drift exceeds the trigger

- **Status:** Accepted
- **Date:** 2026-05-23
- **Issue:** [#293](https://github.com/cerinoligutom/PRism/issues/293)
- **Deciders:** @cerinoligutom

## Context

The frontend mirrors a slice of the Rust DTO surface by hand. `src/types/dashboard.ts:9-10` literally instructs: _"Keep this file in lock-step with `dashboard/types.rs`"_. `src/types/conversation.ts` follows the same pattern, and per-store files (`accounts.ts`, `sync.ts`, `syncActivity.ts`, `conversation.ts`) carry inline discriminated-union mirrors of per-module `*CommandError` enums.

Concrete surface area today:

- Two dedicated mirror files: `src/types/dashboard.ts` (165 lines, 13 declarations), `src/types/conversation.ts` (212 lines, 12 declarations).
- Per-store inline mirrors: ~30 additional `export type` / `interface` declarations across `stores/*.ts` that match Rust DTO shapes.
- Total roughly 50 mirrored declarations.

Drift events in observed history (this post-M7 hardening campaign):

- Renaming `SyncRateLimitPayload.pct` to `rate_remaining_pct` (PR 4) required a coordinated touch on both sides. The frontend store had been silently remapping `e.payload.pct` to the canonical name on receive - a documented smell. `vue-tsc --noEmit` caught the mismatch once both sides were edited.
- The `dismissedFailureId` / `accountScope` / `applyStatus` renames (PR 5) were TS-only and didn't touch the wire surface, but illustrate how the mirror discipline still requires conscious effort across files.
- The DB column renames (`is_draft`, `relation_observed_at` in PR 9 / PR 10) did *not* touch wire shapes because the Rust struct field names had always been the new names; only the SQL column drifted. Codegen wouldn't have helped here.

Net drift cost over the campaign: one coordinated wire-shape change (PR 4), caught and fixed end-to-end without incident. The "Keep in lock-step" comments are a live convention, not a paper one.

## Decision drivers

- v1 launch is imminent. Adding a new build-time dependency, codegen step, or derive macro across ~50 DTOs is a meaningful surface-area expansion days before tag-cutting. Cost is paid now; payoff is amortised over future drift.
- The drift rate is low (1 incident across a 10-PR campaign that explicitly went looking for naming inconsistencies). TypeScript catches the mismatch at the type-check step the moment the consumer is touched, so drift can't merge silently.
- Tauri 2 narrows the tooling field: `specta` / `tauri-specta` for Tauri 2 ship only as `v2.0.0-rc.25` (latest 2026-05-08), still pre-release after three years of development. Stable `specta` 1.x predates Tauri 2 and isn't viable.
- The two genuine alternatives are `typeshare` (1Password, stable 1.x line, last release 2025-12-11) and `ts-rs` (active, latest v12.0.0 from 2026-01-31). Both are real codegen tools, neither is Tauri-aware - they only generate types, not command bindings.
- The architectural ADRs landing this week (0019, 0020) explicitly value the federated per-module pattern. Codegen via specta would push toward a more centralised "all bindings come from one tool" worldview that doesn't fit how the codebase is organised.

## Considered options

1. **Stay manual (current)** - hand-mirror Rust DTOs into `src/types/` and per-store files. Comments name the Rust source. `vue-tsc --noEmit` catches drift the moment a consumer is touched.
2. **Adopt `typeshare`** - add `#[typeshare]` annotations to ~50 Rust DTOs, install the CLI, wire a build step or CI check that regenerates `src/types/generated.ts`. Lightweight: just types, no command bindings.
3. **Adopt `ts-rs`** - similar to typeshare but via derive macro. Generates one TS file per type by default (or one bundled file via config). More compile-time integration; more runtime crate weight in `Cargo.toml`.
4. **Adopt `specta` + `tauri-specta`** - the Tauri-ecosystem-aligned choice. Generates both DTO types AND command call wrappers. Currently only available for Tauri 2 as `v2.0.0-rc.25`.

## Decision

We will keep **Option 1** through v1 launch.

We will revisit if any of the following triggers fire post-launch:

- Three or more drift incidents land on main (where "drift incident" = a PR whose review or CI catches a TS / Rust mismatch caused by hand-mirror lag).
- A new module ships more than ten DTOs at once (e.g. an M8 Teams expansion that adds Team / Membership / Review-request types).
- The frontend grows a second consumer surface (a CLI, a second Tauri window with isolated bindings, etc.) that would benefit from a single source of truth.

When we revisit, the default candidate is **typeshare**:

- Stable, mature 1.x line, last release within the past five months.
- Lightweight: annotations on the Rust side, CLI on the build side, no runtime crate added to `Cargo.toml`.
- No coupling to Tauri command bindings; the existing `invoke<T>(...)` call style stays.
- 1Password ships it as part of their production toolchain - the maintainership posture is right.

`specta` / `tauri-specta` is the wrong choice for PRism specifically. Three years of RCs without a stable 2.0 is too much version risk for a project that already chose Tauri 2 over Tauri 1 to avoid pre-release dependence; adopting an RC tool would re-introduce exactly the dependency posture we declined upstream.

## Consequences

### Positive

- Zero added build-time complexity this cycle. No new derive macros, no new CLI in CI, no new generated files in the repo.
- The "Mirrors X" comments stay as the convention. Future contributors see a precise pointer to the Rust source for any TS mirror.
- TS type-check at vue-tsc keeps catching drift. The PR 4 incident demonstrates the safety net works.

### Negative

- Each non-trivial wire-shape change still needs two coordinated edits. As the surface grows past 50 DTOs the manual cost rises linearly.
- Discriminated-union shapes (`ConversationCommandError`, etc.) are duplicated in TS. A new variant in Rust isn't enforced on the TS mirror until a consumer hits it.

### Neutral / follow-ups

- M8 (Teams view) will be the practical proving ground for the trigger. If it adds ten or more DTOs, this ADR gets revisited automatically.
- If a future post-mortem traces an incident to mirror drift, count it against the trigger and flag.

## References

- `src/types/dashboard.ts:9-10` - the live "lock-step" instruction.
- `src/types/conversation.ts` - second mirror file with the same pattern.
- PR #284 (`refactor(sync): rename pct -> rate_remaining_pct`) - the single observed drift event during the M7 hardening campaign.
- [typeshare on GitHub](https://github.com/1Password/typeshare) - 1.13.4 (2025-12-11).
- [ts-rs on GitHub](https://github.com/Aleph-Alpha/ts-rs) - 12.0.0 (2026-01-31).
- [specta on GitHub](https://github.com/specta-rs/specta) - 1.0.5 (stable, 2023-07-17); 2.0.0-rc.25 (2026-05-08, pre-release).
- [tauri-specta on GitHub](https://github.com/specta-rs/tauri-specta) - 1.0.2 (stable, 2023-05-18); 2.0.0-rc.25 (2026-05-08, pre-release).
- ADR 0019 - Error handling convention (federated per-module command-error enums).
- ADR 0020 - Settings persistence boundary.
