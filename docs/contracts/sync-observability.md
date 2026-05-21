# Sync observability contract

Reference for the diagnostic activity feed that surfaces sync-cycle progress and failures in the status bar (issue #122, milestone _Sync observability_).

The feed is **additive** alongside the existing `sync://status` / `sync://error` / `sync://rate-limit-warning` events. The phase dot in the status bar keeps its current driver (`SyncPhase`); the activity feed adds a live ticker label and a per-cycle event log.

## Event channel

A single channel: `sync://activity`. Every push to the in-memory ring buffer emits one event with the [`ActivityEvent`](#activityevent) payload.

```rust
pub const SYNC_ACTIVITY_EVENT: &str = "sync://activity";
```

## ActivityEvent

```jsonc
// Discriminated by `kind`; variant-specific fields sit alongside the wrapper.
{
  "id": 17,                         // monotonically increasing per process
  "timestamp_ms": 1731234567890,    // unix milliseconds
  "level": "info" | "warn" | "error",
  "account_id": 1 | null,           // null for global events (none today)
  "message": "Cycle started for cerinoligutom",
  "kind": "cycle_started"           // discriminator, see below
  // ...variant-specific fields
}
```

### Levels

| Level   | Use                                                                    |
| ------- | ---------------------------------------------------------------------- |
| `info`  | Cycle / phase boundaries, per-PR fetches, normal completion summaries. |
| `warn`  | Rate-limit pauses; recoverable conditions that delay a cycle.          |
| `error` | Cycle failures (`cycle_failed` only).                                  |

### Variants

| `kind`              | Extra fields                                                   | Level | Notes                                                                 |
| ------------------- | -------------------------------------------------------------- | ----- | --------------------------------------------------------------------- |
| `cycle_started`     | -                                                              | info  | One per cycle entry, after the rate-budget check.                     |
| `phase_started`     | `phase: "discovery" \| "enrichment" \| "pruning"`              | info  | Fired on entry to each sub-phase.                                     |
| `phase_progress`    | `phase`, `current: u32`, `total: u32`                          | info  | Enrichment only today. Fires once per PR; `total` is the cycle total. |
| `pr_fetched`        | `number`, `owner`, `name`, `url`                               | info  | Surfaces a deep-link button in the panel.                             |
| `phase_completed`   | `phase`, `summary: string`                                     | info  | `summary` is a short, human-readable wrap-up of the phase.            |
| `cycle_completed`   | `prs_visited: u32`, `summary: string`                          | info  | Terminal success event for one cycle.                                 |
| `cycle_failed`      | `error_message: string`, `error_kind: string`                  | error | Terminal failure. `error_kind` is `discovery` / `enrichment` / `pruning` / `client_build`. `error_message` is the truncated underlying error. |
| `rate_limit_pause`  | `reset_in_seconds: u64`                                        | warn  | Emitted when the rate-budget guard skips a cycle.                     |

The pre-rendered `message` field is what the panel renders in each row; the structured `kind` payload is what filters and deep-links read from.

## Tauri command

```rust
#[tauri::command]
pub fn list_recent_activity(
    input: Option<ListRecentActivityInput>,
    buffer: State<'_, ActivityBuffer>,
) -> Vec<ActivityEvent>;

pub struct ListRecentActivityInput {
    pub limit: Option<usize>,        // default 100, capped at BUFFER_CAP (200)
    pub account_id: Option<AccountId>,
}
```

Returns the most-recent-first slice of the buffer, filtered by `account_id` when set, capped at `limit`. Used by the frontend store to hydrate on app start so the panel shows recent history before the next `sync://activity` event lands.

## Buffer behaviour

- **In-memory only.** `Arc<Mutex<VecDeque<ActivityEvent>>>` managed on Tauri app state (`app.manage(buffer.clone())`).
- **Cap:** `BUFFER_CAP = 200`. Oldest evicted FIFO once the cap is hit.
- **Per-account semantics.** Each event carries `account_id`. The frontend store filters / aggregates client-side; the buffer itself is global.
- **No persistence.** Buffer is lost on restart. SQLite persistence is out of scope (see [Out of scope](#out-of-scope)).
- **ID monotonicity.** A process-wide `AtomicU64` mints IDs, so the frontend can dedupe (during reconnection windows) and so future "jump to event" affordances have a stable key.

## Live ticker (status-bar chip text)

While a cycle is in flight for any account, the chip's label text reads from the activity store's `currentPhaseLabel`. When idle / completed / errored, the existing phase-derived summary (`Live` / `Sync failed` / etc.) takes over.

### Throttling rules

The ticker derives its label from the most recent activity event. To prevent strobing on fast cycles:

- **Each frame holds for at least 500ms.** Once a value is committed, the next value is held until the 500ms mark.
- **No more than one update per 250ms.** Trailing values that arrive faster are batched into a single trailing-edge commit.

The throttler is built in `src/stores/syncActivity.ts`. It's a small synchronous state machine over `setTimeout`; no animation frames, no batched microtasks. Lockstep with the backend's per-PR cadence keeps it predictable.

## Panel UX

### Open / close

| Trigger                                       | Behaviour                                  |
| --------------------------------------------- | ------------------------------------------ |
| Click chip                                    | Toggle.                                    |
| Click outside panel (and not on chip)         | Close.                                     |
| Press Esc                                     | Close.                                     |
| Sync transitions to `error` and a new failure has landed | Hover over chip auto-opens once. Subsequent hovers do nothing until the next failure. |

The auto-open-on-hover behaviour is gated by an "acknowledged failure id" — opening the panel marks the current failure as seen, so the user isn't peppered with the same popup.

### Filters

- **Account picker.** Dropdown over visible accounts. Default "All accounts". Hidden when only one account is configured.
- **Level toggles.** Three pressable chips — `info`, `warn`, `error`. All on by default.

### Per-row affordances

- Relative timestamp (e.g. "2s ago", "1m ago"), level dot, message.
- For `pr_fetched` events: an "↗" button that opens the PR URL via Tauri's opener.
- The most recent `cycle_failed` event has its row highlighted with a soft red background until the user opens / closes the panel.

### Positioning

`position: fixed`, anchored above the chip via the chip's `getBoundingClientRect()`. A `ResizeObserver` re-anchors when the chip's geometry changes (rare; e.g. account-count label flipping `1 account` → `2 accounts`).

## File ownership

```
src-tauri/src/sync/
  activity.rs                # ActivityBuffer + ActivityEvent + record()
  events.rs                  # SYNC_ACTIVITY_EVENT constant
  worker.rs                  # cycle / phase / PR instrumentation
  commands.rs                # list_recent_activity

src-tauri/src/lib.rs         # app.manage(buffer) + command registration

src/stores/
  syncActivity.ts            # Pinia store + throttling

src/components/
  StatusBar.vue              # chip wrap + ticker + panel mount + auto-open
  StatusBar/
    SyncActivityPanel.vue    # the dropdown panel
```

## Out of scope

- **SQLite-backed persistence.** Deferred until user demand. The session-scoped buffer suffices for the diagnostic UX the issue calls out; cross-session retention would need a schema and migration that isn't justified yet.
- **Structured logging integration.** The activity events aren't wired into a structured log emitter; the buffer is purely the UI-facing surface today.
- **Per-event PR-drawer opening.** The panel only ships the external-link affordance (`↗` on `pr_fetched` rows). Surfacing a row that opens the conversation drawer in-app is a follow-up.
- **Per-cycle telemetry rollup.** A "last N cycles summary" view isn't part of this feed; the panel is a flat event log.
