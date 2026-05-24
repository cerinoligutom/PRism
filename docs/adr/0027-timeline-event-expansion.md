# 0027 — Expanding the StatusTimelineTab event set

- **Status:** Accepted
- **Date:** 2026-05-25
- **Issue:** [#342](https://github.com/cerinoligutom/PRism/issues/342)
- **Deciders:** @cerinoligutom

## Context

ADR 0007 pinned the timeline tab to a closed enum of seven status-change events (`ready_for_review`, `convert_to_draft`, `review_requested`, `reviewed`, `merged`, `closed`, `reopened`). The artboard for the conversation surface hinted at a wider set; M3 shipped the narrow one. Issue #342 reopens the gap as part of pre-v1 polish.

ADR 0007's seven events serve two roles today: they drive the per-PR "latest status change" timestamp (sorted on the dashboard) **and** they are the only events rendered in the tab. Expanding the rendered set without disturbing the dashboard sort means the two roles need to be split.

## Decision drivers

- The tab should reflect the PR's lifecycle, not only the status-change subset.
- The dashboard sort on `latest_status_change_at` must keep the same semantics — a new label or assignment must not bump a PR up the queue.
- New events should carry the secondary string a reasonable user wants to read (the label name, the assignee login, the milestone title).
- Forward-compat: GitHub may add event types we haven't modelled; the renderer must degrade rather than blank out.

## Considered options

1. **Add the broader set to `QualifyingEvent`** — rejected; it changes which events count toward `latest_status_change_at`, breaking the dashboard sort contract from ADR 0007.
2. **Split into two sets: status-change events (unchanged) plus renderable-only events** — chosen.
3. **Persist every event from GitHub verbatim** — rejected; the noise events (`subscribed`, `mentioned`, `cross-referenced`, `referenced`) don't carry signal worth storing, and `commented` is already surfaced separately via the conversation tab.

## Decision

We will keep `QualifyingEvent` unchanged and introduce a parallel **renderable** set the sync writer persists into `timeline_events` purely for the tab.

**Status-change (drive `latest_status_change_at`, unchanged from ADR 0007):**

- `ready_for_review`, `convert_to_draft`, `review_requested`, `reviewed`, `merged`, `closed`, `reopened`.

**Renderable-only (new in this ADR; persisted to `timeline_events`, ignored by `latest_status_change`):**

- `assigned`, `unassigned` — actor + `assignee.login` as subject.
- `labeled`, `unlabeled` — actor + `label.name` as subject.
- `milestoned`, `demilestoned` — actor + `milestone.title` as subject.
- `head_ref_force_pushed` — actor only.
- `base_ref_changed` — actor only.
- `locked`, `unlocked` — actor only.

**Storage shape.** The existing `timeline_events.payload` JSON column gains an optional `subject` field for the secondary string (label name, assignee login, milestone title). The `event_type` column stays the GitHub wire name. `review_state` continues to live under `payload.state` for `reviewed` events.

**Frontend fallback.** Unknown event types render with a generic "Updated" label and a neutral icon. This is the forward-compat path: future GitHub event types surface in the tab without a frontend release.

**Skipped events** (carve-outs for the audit trail):

- `subscribed`, `mentioned` — interest signals, not lifecycle events. Noise.
- `commented` — surfaced on the conversation tab; duplicating it on the timeline would be misleading because issue-comment text wouldn't render there.
- `cross-referenced`, `referenced` — link signals, low value on a single-PR view.
- `renamed` — title changes are visible in the PR header; not worth a row.
- `committed` — modelled explicitly in the REST parser to document the carve-out (carries `committer.date` rather than `created_at`); the commit list is implicit in the diff.
- `head_ref_deleted`, `head_ref_restored` — branch cleanup, not user-facing.

## Consequences

### Positive

- The tab matches the artboard hint without changing the dashboard sort contract.
- The `payload.subject` convention keeps the schema flat: no new columns, no migration.
- The frontend fallback covers future GitHub additions without a release.

### Negative

- The REST parser grows by ten variants. Mitigated by table-driven tests covering every variant.
- The renderable set is a second list to keep in sync alongside `QualifyingEvent`. Mitigated by exhaustive match arms in both the REST parser and the frontend renderer — drift fails type-check.

### Neutral / follow-ups

- Per-event filtering (collapsing labels, for example) is explicit out of scope for #342.
- If a future cycle wants `committed` to surface, the `committer.date` carve-out in the REST parser already documents the work needed.

## References

- ADR 0007 (status timeline from timeline events API).
- [GitHub timeline events API](https://docs.github.com/en/rest/issues/timeline)
- Issue #342.
