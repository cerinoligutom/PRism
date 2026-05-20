# 0012 — Threads-bar four-state redesign and outdated counted in the denominator

- **Status:** Accepted
- **Date:** 2026-05-21
- **Issue:** [#98](https://github.com/cerinoligutom/PRism/issues/98)
- **Deciders:** @cerinoligutom

## Context

M3 shipped a per-PR threads bar with two inconsistent slicings: the dashboard row split threads into three segments (`unresolved`, `involved`, `resolved`) where `involved` overlapped both states, while the conversation-surface bar treated `involved` as a slice of `unresolved`. A PR with one unresolved thread that's also yours rendered as a single blue "you're in" segment on the surface bar - no red, no alarm cue.

The same release excluded outdated threads from the bar denominator and from the resolution rate. The implicit policy was "outdated = handled" - convenient for the math but wrong about the UX. An outdated thread on a still-open PR can carry unresolved feedback that just hasn't been re-attached after a force-push; treating it as resolved makes the threads bar lie about the PR's state, and "Show N outdated" added a toggle nobody asked for to a list view that already dims outdated rows.

ADR 0010 documented the three-column rollup. ADR 0011 cancelled inline expansion. This ADR settles the bar's semantics so the row and the conversation surface read the same numbers in the same colours, and so the resolution rate stops pretending outdated threads are settled.

## Decision drivers

- **One bar, one mental model.** The row and the conversation surface should answer "what's the state of this PR's conversation?" with the same partition of threads, not two different ones.
- **Unresolved-mine deserves a colour that says "yours".** The original blue swallowed urgency. Orange separates "your action item" from "someone else's unresolved" (red) without losing the strong-red alarm for the un-claimed work.
- **Outdated isn't a synonym for resolved.** The threads list shows outdated rows dimmed; the bar should too. Counting them in the denominator makes the resolution rate read what reviewers expect from the visible list.
- **Tooltips beat legends.** A four-segment bar with hover-tooltips conveys the same information as a pip-row legend without the screen real estate.
- **Backwards compatibility doesn't cost anything yet.** M3 just landed; there are no other consumers of the v4 rollup columns, so the migration can rename / drop without shims.

## Considered options

### Bar segmentation

1. **Keep three segments, fix the surface bar to match the row.** Cheaper but still doesn't separate "yours" from "theirs" in the unresolved bucket.
2. **Four segments by `(resolved x involved)`, identical on row and surface.** Disjoint partition of the full thread set, one colour per bucket, one tooltip per segment.
3. **Two-axis stacked bar (resolved vertical band + involved horizontal stripe).** Higher information density, much higher visual complexity, doesn't fit the 90px row footprint.

### Outdated treatment

1. **Keep outdated carved out of the denominator + the "Show N outdated" toggle.** Preserves the existing math. Threads bar doesn't reflect the threads list; resolution rate over-reads when code changes invalidated previously-resolved threads.
2. **Count outdated normally in both the bar and the rate.** Outdated threads sort into one of the four buckets by their own flags. The list always renders them with a dim treatment + `OUTDATED` badge.
3. **Tri-state per thread: surface unresolved, resolved, outdated as parallel buckets in the bar.** Adds a fifth segment to track. We already have the per-row badge for the visual cue; doubling it on the bar adds noise without information.

### "You're in" naming

1. **Keep `is_you_in` / `YOU'RE IN`.** Conversational but inconsistent with the noun form used elsewhere (`involved` in PRism's "Watching" view comes from GitHub's `is:involved` query).
2. **Rename to `is_involved` / `INVOLVED`.** Matches the rest of the codebase's vocabulary; the bar's orange already says "yours" so the badge text just needs to identify the relationship, not perform it.

## Decision

**Bar segmentation: option 2.** Four segments mapped to disjoint `(resolved x involved)` buckets, identical on the dashboard row and the conversation surface. Colour mapping:

| Bucket | Colour token | Rationale |
|---|---|---|
| Unresolved AND NOT involved | `--danger` (red) | Loudest signal: open work nobody's pulled in to yet. |
| Unresolved AND involved | `--warning` (orange) | Your action item, distinct from the un-claimed unresolved set. |
| Resolved AND NOT involved | `--info` (blue) | Settled work you weren't part of. |
| Resolved AND involved | `--success` (green) | Settled work you participated in. |

Non-zero buckets render with a 5% sliver floor so single-thread categories stay visible; remaining width shares proportionally by raw count. Each segment carries a `PRismTooltip` with the bucket label + count (`"Unresolved · 3 threads"`, `"Resolved (involved) · 1 thread"`). The pip-row legend on the conversation surface retires - the tooltips carry the same information without competing for the column head.

**Outdated treatment: option 2.** Outdated threads count in `threads_total` and sort into one of the four buckets by their own `is_resolved` flag and the active account's involvement. The threads list always renders outdated rows with the existing dim treatment + `OUTDATED` badge. The `showOutdated` toggle and its local state are removed. `oldest_unresolved_at` includes outdated-unresolved threads. Resolution rate becomes `resolved / total`, bounded `[0, 1]` by construction.

**"Involved" naming: option 2.** Rename `is_you_in` → `is_involved` (Rust DTO + TypeScript mirror), `YOU'RE IN` badge → `INVOLVED`, and every variable carrying the old vocabulary (`unresolvedOnly`, `involvedSegment` and similar). The `INVOLVED` badge stays on unresolved-involved threads only - the green colour on resolved-involved threads already says "yours" without the chip.

**Schema.** Migration `0005_threads_breakdown.sql` adds the four bucket columns and drops the v4 `threads_unresolved` / `threads_involved` columns. `threads_total` stays as the bar denominator. No backward-compat shims - M3 just landed and there's no other consumer.

## Consequences

### Positive

- The dashboard row and the conversation surface bars are identical. Two surfaces, one mental model, one colour mapping.
- Unresolved-mine has its own colour (orange). PRs with only a single unresolved thread (yours) stop reading as no-urgency blue.
- Tooltips replace the pip-row legend. The conversation surface gains vertical space; the row bar gains hover affordance it didn't have.
- Outdated threads stop lying about the PR's state. The threads list and the bar agree, and the resolution rate matches the visible counts.
- The "involved" vocabulary lines up with the rest of the codebase ("Watching" view, GitHub's `is:involved` query).

### Negative

- The schema migration drops two columns. Anything outside the contract that reads `threads_unresolved` or `threads_involved` breaks - acceptable now because nothing does, but a constraint on how aggressively future migrations can drop without notice.
- Outdated-unresolved threads now hold up the `oldest_unresolved` tile. PRs where a long-abandoned thread is technically still open will read older than they "feel". Acceptable tradeoff: the alternative is the rate-overshoot class of bug from ADR 0010.
- One extra column in the dashboard row SELECT (five thread columns instead of three) plus a CTE in the worker's rollup UPDATE. Cost is negligible at v1 scale.

### Neutral / follow-ups

- The four-bucket math could be pre-aggregated for the conversation surface too (the bar currently iterates `threads` to bucket per-thread because the cached stats only carry the global counts). Not worth doing unless the per-thread iteration shows up in render profiling.
- ADR 0010's "outdated excluded from the bar denominator" stance is superseded by this ADR. The thread-ID storage and lazy-hydration decisions in 0010 remain in force.
- If multi-account ever makes the per-account `(resolved x involved)` split a hot complaint, M5 can promote the bucket columns to per-account rows rather than overwriting them per cycle.

## References

- Contract: [`docs/contracts/conversation-depth.md`](../contracts/conversation-depth.md) (updated alongside this ADR)
- Migration: [`src-tauri/migrations/0005_threads_breakdown.sql`](../../src-tauri/migrations/0005_threads_breakdown.sql)
- ADR [0010](0010-conversation-depth-storage.md) — original rollup shape; the four-bucket columns supersede the three v4 columns it described.
- ADR [0011](0011-cancel-inline-pr-detail-surface.md) — host-cancellation precedent; this ADR is the same shape (revisit a deferred-but-incomplete M3 decision once the surface is in production).
