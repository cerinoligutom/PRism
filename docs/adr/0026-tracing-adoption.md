# 0026 - Backend logging via `tracing` to stdout

- **Status:** Accepted
- **Date:** 2026-05-24
- **Issue:** [#334](https://github.com/cerinoligutom/PRism/issues/334)
- **Deciders:** @cerinoligutom

## Context

The Rust backend logged failures through ad-hoc `eprintln!` calls scattered across the sync worker, notification sink, badge refresher, auth store, update worker, and every Tauri command's `internal()` helper. The convention worked for M1-M6 development but has two structural problems past v1:

- No level filter. Every line is unconditional stderr output, so a Release build either logs everything (noisy on Windows / Linux where the binary owns its console) or has a developer reach for `cfg!(debug_assertions)` per call site.
- No structured fields. Errors are interpolated into a flat string, which means correlation IDs (account_id, pull_request_id, event name) sit next to free-form prose. A future log viewer or remote sink would have to re-parse the string to recover them.

The CLAUDE.md error-handling rule already calls out "structured logging (JSON) with correlation IDs ... ERROR, WARN (default), INFO, DEBUG" as the target. This ADR formalises the move.

## Decision drivers

- Pick the level-and-fields convention already documented in CLAUDE.md so per-call-site lift-and-shift is rote.
- Keep the dependency surface small. The Tauri runtime captures stdout in dev mode, so a rolling-file appender doesn't earn its keep at v1 scale.
- The on-disk `startup.log` (issue #239) is already covered by `startup::report_failure`. Logging persistence outside that window is a v1.x problem, not a v1.0 one.
- Don't ship a feature the issue explicitly puts out of scope (log-viewer UI surface, remote shipping).

## Considered options

1. **`tracing` + `tracing-subscriber` with stdout-only `fmt` layer** - swap each `eprintln!` for `tracing::error!` / `warn!` / `info!` / `debug!` per semantic, default filter `warn`, `RUST_LOG` override.
2. **`log` + `env_logger`** - simpler crate pair but no structured fields and no span support, so it forecloses the correlation-ID story CLAUDE.md already calls for.
3. **Continue with `eprintln!`** - no per-level gating, no fields, no future path to remote shipping.
4. **`tracing` with both stdout and a rolling file (`tracing-appender`)** - covers the "user closed the terminal" gap but introduces filesystem permission concerns we'd rather avoid pre-v1.

## Decision

We will go with **Option 1**: `tracing` + `tracing-subscriber` (`env-filter` + `fmt` features), stdout only.

The subscriber is initialised once inside `lib.rs::run_setup`, before any other backend wiring, via `try_init` so a re-entered setup in tests is a no-op rather than a panic. The filter is `EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))`, which keeps a default Release run quiet and lets a developer raise the floor with `RUST_LOG=debug` (or `RUST_LOG=prism_lib=trace,reqwest=warn` for crate-scoped detail).

Semantic level mapping (issue #334 acceptance criteria):

- `error!` - unrecoverable per-cycle / per-command failures (`Internal` returns, db poisoned, install-on-quit failed).
- `warn!` - transient and retryable (prune failed, persist outcome write failed, emit failed).
- `info!` - cycle-shaped one-shots (auto-archive sweep complete, archive retention sweep complete).
- `debug!` - per-entity hydration and noisy skips (timeline element skip, notify dispatch payload, "PR row missing" gaps).

Test-only `eprintln!` inside `#[cfg(test)]` modules stays. The replacement is a production-path concern; test fixtures keep the simpler primitive.

## Consequences

### Positive

- Per-level filter at runtime via `RUST_LOG` without rebuilding.
- Structured fields land at the call site (`account_id`, `pull_request_id`, `event`, `err`), so a future JSON formatter or remote sink (post-v1) has the data to forward without re-parsing.
- The CLAUDE.md error-handling rule now matches the codebase.

### Negative

- One more dependency pair to track. `tracing-subscriber` pulls in `regex` for `env-filter`; the build-time cost is bounded (it's already an indirect dep via `reqwest`).
- A consumer outside the Tauri dev window (production binary, no terminal attached) sees nothing without a follow-up file or remote sink ADR.

### Neutral / follow-ups

- Log-viewer UI surface is explicitly out of scope for #334; revisit when the post-v1 observability work lands.
- A future ADR can extend this with `tracing-appender` if the "no terminal attached" gap matters before remote shipping is on the table.
- `tracing::instrument` is intentionally not sprinkled across functions. Spans land where they earn their keep (sync cycle root, conversation hydrator) in follow-up work; this ADR covers the `eprintln!` replacement only.

## References

- Issue [#334](https://github.com/cerinoligutom/PRism/issues/334) - the M7 polish ticket that authorised the swap.
- ADR 0019 - the error-handling convention this ADR's call-sites lean on.
- ADR 0024 - auto-update silent-failure rule that informs the `warn` vs `error` split on the update worker's persist path.
- `CLAUDE.md` - "Error handling" section that names the level set.
- `tracing` crate docs: <https://docs.rs/tracing/0.1>
- `tracing-subscriber` crate docs: <https://docs.rs/tracing-subscriber/0.3>
