# 0022 - Versioning scheme and build-metadata pipeline

- **Status:** Accepted
- **Date:** 2026-05-23
- **Issue:** [#295](https://github.com/cerinoligutom/PRism/issues/295)
- **Deciders:** @cerinoligutom

## Context

The `0.1.0` string lives in three files independently, each consumed by a different toolchain:

- `package.json` - pnpm + the frontend bundle.
- `src-tauri/Cargo.toml` - cargo (and via `CARGO_PKG_VERSION`, the Rust runtime).
- `src-tauri/tauri.conf.json` - Tauri's bundler and `@tauri-apps/api/app.getVersion()` at runtime.

Missing one out of a release-bump leaves the bundle's reported version stale. The release pipeline (#303, #304) needs a deterministic version to tag and stamp into the GitHub Release; the in-app About panel + StatusBar pill need a single source of truth at runtime.

Separately, the About panel needs the build commit and timestamp so bug reports carry reproducible identifiers, and dev StatusBar pills need the SHA inline so console screenshots aren't ambiguous about which commit they came from.

## Decision drivers

- One canonical version per release; drift between the three files is a release-blocking class of bug.
- Every shipped binary must be traceable back to a specific commit without parsing build artefact metadata.
- The release flow is fully manual-trigger (ADR-0023). A bump script the maintainer runs locally is fine; auto-bumping on every PR is not.
- Frontend already consumes Rust types via manual snake_case bindings (ADR-0021). Build metadata follows the same convention.
- Build-script overhead matters. `git rev-parse` is cheap; deeper version parsing or vendored crates aren't worth it for a 6-char SHA.

## Considered options

1. **Three files, sync script + CI drift check** - SemVer-validated bump script writes all three; CI fails on disagreement.
2. **One canonical file, generate the others at build** - e.g. read `tauri.conf.json` and template the rest. Requires teaching cargo + pnpm to defer to a non-canonical file each, which neither supports cleanly.
3. **Drop one file's version** - leave cargo / pnpm at `0.0.0` permanently and only stamp `tauri.conf.json`. Breaks `CARGO_PKG_VERSION` that the Rust `app_metadata` module reads, and trips npm tooling that warns on `0.0.0`.
4. **CalVer** - date-shaped versions instead of SemVer. Loses the semantic upgrade signal (breaking vs feature vs patch) on a versioned desktop app with a long roadmap.

## Decision

We will go with **Option 1**: SemVer across all three files, kept in sync by `scripts/bump-version.ts`, with a CI step that fails the build when the three files disagree.

SemVer (`MAJOR.MINOR.PATCH`). Pre-release stays at `0.x.y` until Platform-QA passes; the first stable cut is `v1.0.0`. Pre-release / build-metadata suffixes (`1.0.0-rc.1`) stay out of scope until a real release candidate forces the conversation.

Three additional consts are baked into the Rust binary at compile time via `src-tauri/build.rs`:

- `PRISM_GIT_SHA` - first 6 chars of `git rev-parse HEAD`, or `"unknown"` when git is unavailable (source tarball, no `.git/`).
- `PRISM_BUILD_DATE` - UTC RFC 3339 timestamp.
- `PRISM_PROFILE` - cargo's `PROFILE` env var (`"release"` or `"debug"`).

A `get_app_metadata` Tauri command returns `{ version, commit_sha, build_date, profile, os, arch }` to the frontend. `version` reads from `CARGO_PKG_VERSION`, which Cargo populates from `Cargo.toml` directly, so the runtime cannot disagree with the build that produced it. `os` and `arch` come from `std::env::consts`.

Display format:

- StatusBar pill: `v0.1.0` on release builds, `v0.1.0 · abc123` on dev / debug builds.
- About panel: full breakdown with `Build abc123` as its own row, plus a "Copy diagnostics" button that emits the same fields as plain text for pasting into bug reports.

## Consequences

### Positive

- One bump command writes all three files. The CI drift check makes drift impossible to merge.
- The runtime version cannot diverge from the build that produced it (it reads `CARGO_PKG_VERSION`).
- Every bug report copy-pasted from the About panel carries the commit + build date, removing the "what version are you on" round-trip.
- Dev builds visibly carry their SHA in the StatusBar, so screenshots in PR discussions self-identify.

### Negative

- Bumping the version is a deliberate maintainer step (`pnpm bump-version <x>`), not automated. Also a positive given the manual-trigger release flow (ADR-0023).
- The bump script edits files via anchored regex rather than parsed JSON / TOML; keeps file formatting byte-stable but is mildly fragile if `Cargo.toml` is reformatted to put `version` outside the `[package]` block at the top. CI drift check catches mismatches but not malformed inputs.
- `time` is added as a `[build-dependencies]` entry. Tiny - it's already a main dep - but explicitly noted.

### Neutral / follow-ups

- Release pipeline (ADR-0023) consumes the bump script + drift check.
- "What's new" dialog (ADR-0025) compares `app_metadata.version` against persisted `last_seen_version` to decide whether to show the dialog after an update.
- If we ever ship pre-release tags (`1.0.0-rc.1`), revise `SEMVER_RE` in `scripts/bump-version.ts` and the StatusBar pill copy.

## References

- Issue [#295](https://github.com/cerinoligutom/PRism/issues/295)
- [ADR-0021](0021-rust-to-typescript-type-bindings.md) - snake_case TS bindings convention this ADR follows
- [`scripts/bump-version.ts`](../../scripts/bump-version.ts)
- [`src-tauri/build.rs`](../../src-tauri/build.rs)
- [Semantic Versioning 2.0.0](https://semver.org/)
