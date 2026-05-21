# Triage UX interface contract

This document is the shared interface contract for **M4: sorting, filter chips, search, unread/mention highlighting, and the "needs my attention" composite signal**. It pins the schema additions, sync-cycle changes, Tauri command surface, DTO extensions, frontend component interfaces, store changes, and the file-ownership map for the six Wave-2 / Wave-3 implementation issues that fan out from this PR.

If you're implementing any M4 issue, **read this end-to-end before writing code**. Anything ambiguous is a spec bug - open a PR or comment on the issue to refine the contract rather than silently diverging.

Wave / issue map:

- **M4-0** [#144](https://github.com/cerinoligutom/PRism/issues/144) - this PR (contract + ADR + migration + module shell + DTO extensions).
- **M4-A** [#145](https://github.com/cerinoligutom/PRism/issues/145) - `mark_pr_read` + `mark_pr_unread` Tauri commands and the lazy-on-open auto-mark wiring.
- **M4-B** [#146](https://github.com/cerinoligutom/PRism/issues/146) - sync-cycle mention scanner + `needs_attention` recompute.
- **M4-C** [#147](https://github.com/cerinoligutom/PRism/issues/147) - dashboard query: extend `DashboardPullRequest` SELECT to read the triage columns; sidebar attention-count badge.
- **M4-D** [#148](https://github.com/cerinoligutom/PRism/issues/148) - `list_filter_chip_counts` Tauri command + the `DashboardSort::Stale` / `DashboardSort::NeedsMe` ORDER BYs.
- **M4-E** [#149](https://github.com/cerinoligutom/PRism/issues/149) - frontend filter chips bar + sort selector + search input components.
- **M4-F** [#150](https://github.com/cerinoligutom/PRism/issues/150) - dashboard store extension (active chips, sort, search, in-memory filter pipeline) + row unread/attention tints + view-change reset.

## Why this exists

M2's [`dashboard-data.md`](dashboard-data.md) demonstrated the pattern. M3's [`conversation-depth.md`](conversation-depth.md) repeated it. One contract PR lands the shared scaffolding (DTO extensions, file boundaries, schema, command shells, ADR) so parallel agents implement against the contract without conflicting on `Cargo.toml`, `lib.rs`, migrations, or the dashboard projection.

M4 has six implementation issues fanning out from this contract (three back-end, three front-end). The decisions encoded here were agreed in the scoping discussion before this doc was drafted:

- **Read-trigger is implicit.** Opening the drawer / route auto-marks the PR read - same pattern as M3's lazy-hydration trigger. An explicit "mark all read" power-user action is deferred to a polish PR; the per-PR "Mark unread" menu action (M4-F) is the only manual flip.
- **Read-state lives on the existing `pull_request_viewer_relations` table.** The row is already keyed `(account_id, pull_request_id)` and the discovery / sync cycle already maintains it. No new table.
- **"Needs my attention" is precomputed.** A composite boolean column on the relations row, written by the sync worker after every cycle and by `mark_pr_read` / `mark_pr_unread` for the mention-driven flip. Dashboard reads a single column.
- **Mentions are detected by substring scan on the persisted `body_text`** (review comments + issue comments), gated by a per-(account, PR) `mention_scan_watermark_at` so repeated cycles are idempotent.
- **Filter chips compose as AND across chips, OR within a chip.** Turning on more chips narrows results. Per-chip counts are independent - each shows what would match if you toggled that chip _alone_.
- **Search is in-memory.** The dataset is bounded (a few hundred PRs typical). FTS5 is deferred indefinitely.

## Scope

### In M4

- Per-account read-state persisted (`read_at`, `read_pr_updated_at`).
- Mention counter persisted per (account, PR) and reset on read.
- Precomputed `needs_attention` boolean per (account, PR).
- `DashboardSort::Stale` and `DashboardSort::NeedsMe` ORDER BYs on the dashboard query.
- Filter chips bar (Needs my attention, Unresolved threads, CI failing, Stale, Drafts) with live counts.
- Search input that filters the loaded view in-memory across title / repo / author.
- Sort selector (segmented buttons) next to the existing GROUP selector.
- Dashboard row unread dot + `.pr.attention` row tint matching the artboard.
- Sidebar "needs me" count badge on each view.

### Deferred (do not implement)

- **Explicit "Mark all read" power-user action.** Polish-PR follow-up; menu placement undecided.
- **Mention chip on the row.** Optional polish in M4-F; the artboard shows the unread dot + tint as the v1 signal.
- **FTS5 search backend.** The bounded dataset doesn't justify the dependency. If multi-hundred-PR accounts grow into thousands, revisit via a fresh ADR.
- **cmd+K palette UI.** v1 only focuses the existing search input on `cmd+K`. A command palette is a post-v1 surface.
- **Per-thread mention attribution.** The unread mention count is a single counter, not a thread-by-thread index. Drilling down lives on the conversation surface itself.
- **`@team-handle` resolution.** Only `@<viewer-login>` matches in v1; team mentions are out of scope until accounts grow GitHub-org membership data.
- **Mention false-positive filtering.** Mentions inside fenced code blocks and inside blockquoted historical quotes are counted alongside real mentions. Acceptable v1 noise (see "Implementation notes" below).

## Module layout

```
src-tauri/
  migrations/
    0010_triage_state.sql         # Wave 1 - owned by the contract PR
  src/
    triage/                       # Wave 1 creates module shell; Wave 2 implements
      mod.rs                      # public surface + Tauri command registration re-exports
      types.rs                    # FilterChipCounts DTO
      commands.rs                 # Wave 2-A + 2-D - Tauri command bodies
      query.rs                    # Wave 2-D - SQL composition for filter-chip counts
    sync/
      worker.rs                   # Wave 2-B extends write_pr_updates with mention scan + attention recompute
    dashboard/
      types.rs                    # Wave 1 extends DashboardPullRequest (unread, needs_attention, mentioned_count_unread)
                                  # Wave 1 widens DashboardSort enum (Stale, NeedsMe)
      query.rs                    # Wave 2-C extends SELECT + projection; Wave 2-D implements the new ORDER BYs

src/
  components/
    dashboard/
      FilterChipsBar.vue          # Wave 3-E NEW - chip row + counts
      SortSelector.vue            # Wave 3-E NEW - segmented sort buttons
      DashboardSearch.vue         # Wave 3-E NEW - search input + cmd+K focus
      PullRequestRow.vue          # Wave 3-F extends with unread dot + .pr.attention tint
  stores/
    dashboard.ts                  # Wave 3-F extends with activeChips / activeSort / searchQuery + in-memory pipeline
  types/
    dashboard.ts                  # Wave 1 mirrors the Rust DTO + DashboardSort extensions
```

## Schema additions

The contract PR lands the full migration. Wave-2 agents must not edit this file - additional columns post-M4 go in `0011+`.

```sql
-- src-tauri/migrations/0010_triage_state.sql

-- ----------------------------------------------------------------
-- pull_request_viewer_relations: per-account triage state. Keyed
-- (account_id, pull_request_id); already cascade-deleted with accounts.
-- ----------------------------------------------------------------

-- Unix seconds when the viewer last opened this PR's detail surface. NULL =
-- never opened on this account.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN read_at INTEGER;

-- Snapshot of pull_requests.updated_at at the moment read_at was set. The
-- frontend derives `unread` as `read_at IS NULL OR
-- pull_requests.updated_at > read_pr_updated_at` so the row flips back to
-- unread when sync bumps `updated_at` after the last open.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN read_pr_updated_at INTEGER;

-- Running count of @<viewer-login> matches the sync cycle has seen since
-- the last read. Reset to zero by `mark_pr_read`. Idempotent across cycles
-- via the watermark below.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN mentioned_count_unread INTEGER NOT NULL DEFAULT 0;

-- Unix seconds of the latest comment.created_at the mention scanner has
-- already counted. The next sync only scans comments newer than this. NEW
-- in this migration so the first scan picks up every comment newer than
-- the epoch (the default 0); `mark_pr_read` pushes this forward to the
-- read timestamp even when no mentions are present.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN mention_scan_watermark_at INTEGER NOT NULL DEFAULT 0;

-- Precomputed "needs my attention" composite (see ADR 0015 for the
-- formula). Written by the sync worker after every cycle and by
-- `mark_pr_read` / `mark_pr_unread` for the mention-driven flip.
ALTER TABLE pull_request_viewer_relations
    ADD COLUMN needs_attention INTEGER NOT NULL DEFAULT 0;

-- Partial index sized for the sidebar count-badge query
-- (`SELECT COUNT(*) ... WHERE account_id = ? AND needs_attention = 1`).
CREATE INDEX idx_pr_viewer_relations_attention
    ON pull_request_viewer_relations (account_id)
    WHERE needs_attention = 1;
```

### Rationale for the schema choices

- **Extends the existing relations table.** The row is already keyed `(account_id, pull_request_id)` and the discovery / sync cycle already maintains its lifecycle. A new `pr_read_state` table would duplicate the key + ON DELETE CASCADE pattern for zero structural benefit.
- **`read_*` prefix not `last_seen_at`.** `pull_request_viewer_relations.last_seen_at` already exists - it's a cycle-bookkeeping timestamp used by the discovery pruning phase. Naming the read watermark `read_at` keeps the two semantically distinct fields visibly distinct.
- **Two columns for read-state (`read_at` + `read_pr_updated_at`).** A single column can't distinguish "you read the PR after its last upstream update" from "you read the PR but it's been bumped since". The pair turns the unread derivation into a deterministic comparison the query can run at read time.
- **`mention_scan_watermark_at` separate from `read_at`.** The scanner needs to advance its idempotency cursor every cycle even when no mentions are present; `read_at` advances only on user opens. Conflating them would either double-count (scanner re-runs on every cycle) or miss mentions written between cycles (scanner never advances).
- **Boolean `needs_attention` column instead of a CTE.** The composite is read on every dashboard render and on every sidebar badge update. Pre-aggregating once per sync cycle (and on read/unread flips) is far cheaper than running the four-condition CTE per query.
- **Partial index for the badge query.** The full relations table is small but the predicate `WHERE needs_attention = 1` keeps the index footprint to just the rows the sidebar count cares about. The badge query becomes `SELECT COUNT(*) FROM idx_pr_viewer_relations_attention WHERE account_id = ?`.
- **No new table.** The `pr_read_state(account_id, pull_request_id, read_at, mentioned_count_unread)` alternative was considered and rejected for the reasons in ADR 0015 ("Considered options" -> "Read-state storage").

## Sync cycle changes

M2 + M3 cycles run Discovery -> Team -> Enrichment -> Pruning (per account). M4 modifies only the **Enrichment** phase. Discovery, Team, and Pruning are unchanged. The cycle's rate budget envelope is unchanged - the new work is pure post-fetch SQL.

Enrichment additions per PR, after the existing thread / review / rollup writes commit:

1. **Mention scan** runs against `review_comments` + `issue_comments` rows whose `created_at > pull_request_viewer_relations.mention_scan_watermark_at` and whose `author_login != accounts.login` (viewer's own comments don't count). The scanner increments `pull_request_viewer_relations.mentioned_count_unread` by the count of matches and pushes `mention_scan_watermark_at` to `MAX(created_at)` from the scanned set. Re-runs within a cycle (the worker writes per PR, not per cycle) stay idempotent because the watermark advances atomically with the count update inside the same transaction.
2. **Attention recompute** flips `pull_request_viewer_relations.needs_attention` to 1 if ANY of the four ADR-0015 conditions hold for the (account, PR) pair, 0 otherwise. Single UPDATE per PR per account.

Pseudocode (the actual SQL lives in `sync::worker::write_pr_updates`, Wave 2-B):

```sql
-- Per (account, PR) inside the cycle's per-PR transaction.

-- 1. Mention scan.
WITH new_mentions AS (
    SELECT c.created_at, c.body_text, c.author_login
      FROM review_comments c
      JOIN review_threads t ON t.id = c.review_thread_id
     WHERE t.pull_request_id = ?pr_id
       AND c.created_at > ?watermark
       AND c.author_login != ?viewer_login
    UNION ALL
    SELECT ic.created_at, ic.body_text, ic.author_login
      FROM issue_comments ic
     WHERE ic.pull_request_id = ?pr_id
       AND ic.created_at > ?watermark
       AND ic.author_login != ?viewer_login
),
matches AS (
    -- Word-boundary substring match against `@<viewer_login>`.
    -- See "Implementation notes" below for the regex shape.
    SELECT COUNT(*) AS n, MAX(created_at) AS max_at
      FROM new_mentions
     WHERE body_text REGEXP ?mention_pattern
)
UPDATE pull_request_viewer_relations
   SET mentioned_count_unread = mentioned_count_unread + (SELECT n FROM matches),
       mention_scan_watermark_at = COALESCE(
           (SELECT max_at FROM matches),
           mention_scan_watermark_at
       )
 WHERE account_id = ?account_id
   AND pull_request_id = ?pr_id;

-- 2. Attention recompute.
UPDATE pull_request_viewer_relations rel
   SET needs_attention = (
       SELECT CASE WHEN
           -- C1: viewer authored, someone left an unresolved thread for them.
           (EXISTS (
               SELECT 1 FROM pull_requests pr
                JOIN accounts a ON a.id = rel.account_id
                WHERE pr.id = rel.pull_request_id
                  AND pr.author_login = a.login
                  AND pr.threads_unresolved_involved > 0
           ))
           -- C2: viewer is a pending requested reviewer.
           OR (EXISTS (
               SELECT 1 FROM requested_reviewers rr
                JOIN accounts a ON a.id = rel.account_id
                WHERE rr.pull_request_id = rel.pull_request_id
                  AND rr.login = a.login
           ))
           -- C3: unread mentions outstanding.
           OR (rel.mentioned_count_unread > 0)
           -- C4: changes requested on a PR the viewer authored.
           OR (EXISTS (
               SELECT 1 FROM pull_requests pr
                JOIN accounts a ON a.id = rel.account_id
                WHERE pr.id = rel.pull_request_id
                  AND pr.author_login = a.login
                  AND pr.review_decision = 'CHANGES_REQUESTED'
           ))
       THEN 1 ELSE 0 END
   )
 WHERE rel.account_id = ?account_id
   AND rel.pull_request_id = ?pr_id;
```

`mark_pr_read` runs the same UPDATE block after resetting `mentioned_count_unread = 0` so the flip propagates without waiting for the next sync.

### Rate budget impact

Zero new network round-trips. The mention scan + attention recompute are pure SQL against rows the cycle has already written. The mention scanner reads from already-persisted `body_text` columns; if a PR's comments haven't been hydrated by `fetch_pr_conversation` yet, the scan sees only the head-comment snapshot for each thread, which is acceptable - the lazy hydrator will surface the full set on the next drawer open and the mention counter will catch up on the following cycle.

## Tauri command surface

```rust
// src-tauri/src/triage/types.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterChipCounts {
    pub needs_attention: i64,
    pub unresolved_threads: i64,
    pub ci_failing: i64,
    pub stale: i64,
    pub drafts: i64,
}
```

```rust
// src-tauri/src/triage/commands.rs

/// Mark a PR as read for the given account. Sets `read_at` to now, captures
/// `pull_requests.updated_at` into `read_pr_updated_at`, resets
/// `mentioned_count_unread` to zero, pushes `mention_scan_watermark_at`
/// forward to the read timestamp, and recomputes `needs_attention` inside
/// the same transaction.
#[tauri::command]
pub fn mark_pr_read(
    pull_request_id: i64,
    account_id: i64,
    db: State<'_, DbHandle>,
) -> Result<(), String>;

/// Flip a PR back to unread for the given account. Clears `read_at` and
/// `read_pr_updated_at`; `mentioned_count_unread` is preserved. The next
/// sync cycle re-evaluates `needs_attention` against the new state; the
/// command itself runs the recompute synchronously so the dashboard reflects
/// the flip without waiting.
#[tauri::command]
pub fn mark_pr_unread(
    pull_request_id: i64,
    account_id: i64,
    db: State<'_, DbHandle>,
) -> Result<(), String>;

/// Count how many PRs in the current view would match each filter chip
/// independently of the other chips. Per-chip counts so the UI shows
/// what would match if a single chip were toggled alone.
///
/// `account_id = Some(id)` runs the per-account count. `account_id = None`
/// (the ADR 0016 unified default) fans the count across every tracked
/// account and dedupes by PR id so a PR matched via two accounts contributes
/// one to each chip it triggers - matching the dashboard query's union-mode
/// `GROUP BY pr.id` row shape.
#[tauri::command]
pub fn list_filter_chip_counts(
    view: DashboardView,
    account_id: Option<i64>,
    db: State<'_, DbHandle>,
) -> Result<FilterChipCounts, String>;
```

The commands are synchronous DB writes / reads - no `async` because there's no network round-trip. `mark_pr_read` / `mark_pr_unread` take `account_id: Option<i64>` (ADR 0016 fan-out: `None` writes across every relation owner). `list_filter_chip_counts` takes `account_id: Option<i64>` too: `Some(id)` keeps the per-account behaviour byte-identical to before ADR 0016, `None` reads the union scope and dedupes by PR id.

## DTO extensions

`DashboardPullRequest` (Rust + TS mirror) grows three fields:

```rust
pub struct DashboardPullRequest {
    // ... existing fields
    pub unread: bool,
    pub needs_attention: bool,
    pub mentioned_count_unread: i64,
}
```

```ts
export interface DashboardPullRequest {
  // ... existing fields
  readonly unread: boolean;
  readonly needs_attention: boolean;
  readonly mentioned_count_unread: number;
}
```

`DashboardSort` (Rust + TS) widens from one variant to three:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DashboardSort {
    Updated,
    Stale,
    NeedsMe,
}
```

```ts
export type DashboardSort = "updated" | "stale" | "needs-me";
```

Wave-1 widens the enum + struct so Wave-2/3 can implement against the typed interface without churning the wire shape; the contract PR projects the new fields to `false` / `0` and routes the new sort variants through the `Updated` ORDER BY so existing behaviour is preserved until Wave 2-C / 2-D land their queries.

## Read-state derivation

Computed at query time inside `dashboard::query::project_pr_row`:

```rust
let unread = read_at.is_none()
    || pull_request_updated_at > read_pr_updated_at.unwrap_or(0);
```

Equivalent SQL projection (Wave 2-C):

```sql
SELECT
    -- ... existing columns
    CASE
        WHEN rel.read_at IS NULL THEN 1
        WHEN pr.updated_at > rel.read_pr_updated_at THEN 1
        ELSE 0
    END AS unread,
    COALESCE(rel.needs_attention, 0)        AS needs_attention,
    COALESCE(rel.mentioned_count_unread, 0) AS mentioned_count_unread
FROM pull_requests pr
LEFT JOIN pull_request_viewer_relations rel
       ON rel.pull_request_id = pr.id
      AND rel.account_id = ?account_id
-- ...
```

The `LEFT JOIN` means Team-view PRs that the active account has no relation row for still surface (with `unread = 1` and `needs_attention = 0`). The four relation-backed views (Authored / Assigned / Watching) already gate on `rel.<flag> = 1` so the row is guaranteed present and the COALESCE defaults never trip.

## Sort ORDER BY (Wave 2-D)

```rust
fn build_sql(from_and_where: &str, sort: DashboardSort) -> String {
    let order_by = match sort {
        DashboardSort::Updated => {
            "ORDER BY COALESCE(pr.latest_status_change_at, pr.updated_at) DESC, pr.id DESC"
        }
        DashboardSort::Stale => {
            // Oldest activity first. `updated_at` is the upstream-truth field;
            // `latest_status_change_at` is a derived signal we don't surface
            // in the Stale ordering because the chip semantics are
            // "old upstream activity", not "stale derived state".
            "ORDER BY pr.updated_at ASC, pr.id DESC"
        }
        DashboardSort::NeedsMe => {
            "ORDER BY rel.needs_attention DESC, \
                      COALESCE(pr.latest_status_change_at, pr.updated_at) DESC, \
                      pr.id DESC"
        }
    };
    // ...
}
```

The `NeedsMe` ORDER BY references `rel.needs_attention`, which requires the relation join to be present. The Team view already lacks `rel`; Wave 2-D either keeps the column NULL-safe via `COALESCE(rel.needs_attention, 0)` or shadows the column from a LEFT JOIN against relations for the Team-view query path.

## Filter chip semantics

Five chips, each with an independent count and an active state. Multi-select.

| Chip | Active predicate | Count source |
|---|---|---|
| Needs my attention | `rel.needs_attention = 1` | `pull_request_viewer_relations.needs_attention` |
| Unresolved threads | `pr.threads_unresolved_involved + pr.threads_unresolved_uninvolved > 0` | `pull_requests.threads_*` rollup |
| CI failing | `pr.ci_state IN ('FAILURE', 'ERROR')` | `pull_requests.ci_state` |
| Stale | `(strftime('%s','now') - pr.updated_at) > 604800` (7 days) | `pull_requests.updated_at` |
| Drafts | `pr.is_draft = 1` (column `draft`) | `pull_requests.draft` |

**Composition rule.** Active chips compose as AND. The dashboard's effective WHERE clause is `view AND chip_1 AND chip_2 AND ...`. Turning on more chips only ever narrows.

**Counts rule.** Each chip count is independent of the active chip set - the count shows how many PRs would match if that chip were toggled alone (within the active view + account scope). The user always sees "if I toggle this on, how many show up?" rather than "how many of the current results match this chip?".

The chip-count Tauri command (`list_filter_chip_counts`) runs five SELECTs over the same view-scoped FROM clause and returns them in one `FilterChipCounts` payload. The frontend invalidates the counts whenever the active view or account filter changes. In unified scope (`account_id = None`) each SELECT wraps in `COUNT(DISTINCT pr.id)` over a LEFT JOIN to relations without an account predicate, mirroring the dashboard query's union-mode `GROUP BY pr.id` so the chip count agrees row-for-row with the chip-filtered list (ADR 0016, issue #171).

### Tooltip explainers

Every chip carries a `PRismTooltip` explainer on hover. The copy is pinned here so it can't drift from the predicate above. Use `:text` for single-line tooltips and the `#content` slot for the multi-row "Needs my attention" breakdown, mirroring the `ThreadsBar` and `ReviewerStack` patterns.

| Chip | Rendering | Copy |
|---|---|---|
| Needs my attention | `#content` slot, multi-row | Header: `PRs match if any of:` <br/> Rows (rendered as a tooltip-list, one per line): <br/> - You authored, unresolved thread involves you <br/> - You're a requested reviewer (pending) <br/> - You have unread @mentions <br/> - Changes requested on your PR |
| Unresolved threads | `:text` | `PRs with at least one unresolved review thread.` |
| CI failing | `:text` | `PRs whose latest commit's CI rollup is FAILURE or ERROR.` |
| Stale | `:text` | `PRs with no activity in the last 7 days.` |
| Drafts | `:text` | `Draft PRs.` |

The multi-row tooltip body lives in an unscoped `<style>` block on `FilterChipsBar.vue` because Reka's `TooltipPortal` teleports the content outside the component's `data-v-*` scope boundary (same pattern as `ReviewerStack.vue`'s overflow tooltip rows and `ThreadsBar.vue`'s breakdown).

## Search semantics

In-memory filter over the loaded view's rows. **No backend query change.**

- Case-insensitive match.
- Fields searched: `title`, `repo.owner/name` (formatted as `owner/name`), `author_login`.
- Debounce: 150ms.
- `cmd+K` focuses the input (existing artboard kbd hint).
- No regex, no fuzzy matching. Plain `.toLowerCase().includes(query)`.

**Application order:**

1. Backend returns view-scoped rows (sort + chip filter applied server-side).
2. Frontend store filters the resulting array by the search query (in-memory).
3. Grouping (Repo / Org / None) applies to the filtered set.
4. Render.

The chip filter is on the server because the chips drive the visible count and the count needs to reflect the database. The search is on the client because the dataset fits in memory and round-tripping every keystroke is wasteful.

### Counter semantics

The view header "12 open · 3 need you" reads `pullRequests.length` (post-chip, post-search) for the first segment and the live `needs_attention` count for the second. Both update as the user types in the search input.

## Frontend component interfaces

### `FilterChipsBar.vue`

```ts
type ChipKey =
  | "needs-attention"
  | "unresolved-threads"
  | "ci-failing"
  | "stale"
  | "drafts";

defineProps<{
  /** Live counts; the bar disables count display when null (loading). */
  counts: FilterChipCounts | null;
  /** Currently active chip keys. */
  active: ReadonlySet<ChipKey>;
}>();

defineEmits<{
  /** Toggle a single chip; the parent updates the Set. */
  toggle: [key: ChipKey];
  /** "Clear all" affordance from the filtered-empty state. */
  clear: [];
}>();
```

Layout: matches the artboard `.top-chips` row. Each chip is a `<button class="chip" :class="{ active: ... }">` with the count as a trailing `<span class="count">`. The bar sits between the title row (h1 + count) and the existing GROUP / SORT segmented buttons on the same `.dashboard-header__chips` flex row in `DashboardView.vue`.

### `SortSelector.vue`

```ts
defineProps<{
  modelValue: DashboardSort;
}>();

defineEmits<{
  "update:modelValue": [value: DashboardSort];
}>();
```

Three segmented buttons: Updated / Stale / Needs me. Sits to the right of the GROUP selector in the existing header chips row; the artboard already shows the position.

### `DashboardSearch.vue`

```ts
defineProps<{
  modelValue: string;
}>();

defineEmits<{
  "update:modelValue": [value: string];
}>();
```

Wraps an `<input>` with the search icon + `cmd+K` kbd hint chip. Listens for `cmd+K` globally (via VueUse `useEventListener('keydown', ...)`) and `inputRef.value?.focus()` on match. Debounces the `update:modelValue` emit by 150ms.

## Dashboard store extension

```ts
// src/stores/dashboard.ts (additions)

import type { FilterChipCounts } from "@/types/triage";

type ChipKey =
  | "needs-attention"
  | "unresolved-threads"
  | "ci-failing"
  | "stale"
  | "drafts";

const activeChips = ref<Set<ChipKey>>(new Set());
const activeSort = ref<DashboardSort>("updated");
const searchQuery = ref<string>("");
const chipCounts = ref<FilterChipCounts | null>(null);

// View change resets chips + search but preserves sort.
async function setView(next: DashboardView): Promise<void> {
  if (view.value === next) return;
  view.value = next;
  activeChips.value = new Set();
  searchQuery.value = "";
  await load();
}

function setSort(next: DashboardSort): void {
  if (activeSort.value === next) return;
  activeSort.value = next;
  void load();  // backend re-sorts
}

function toggleChip(key: ChipKey): void {
  const next = new Set(activeChips.value);
  if (next.has(key)) next.delete(key); else next.add(key);
  activeChips.value = next;
  void load();  // chip filters change which rows the backend returns
}

function clearChips(): void {
  if (activeChips.value.size === 0) return;
  activeChips.value = new Set();
  void load();
}

function setSearch(next: string): void {
  searchQuery.value = next;
  // No load() - search is purely in-memory.
}

// Filtered list - applied AFTER groups via a new derived getter so the
// existing `groups` computed reads from the filtered set rather than the
// raw `pullRequests`.
const filteredPullRequests = computed<DashboardPullRequest[]>(() => {
  const q = searchQuery.value.toLowerCase().trim();
  if (!q) return pullRequests.value;
  return pullRequests.value.filter((pr) => {
    const repoSlug = `${pr.repo.owner}/${pr.repo.name}`.toLowerCase();
    return (
      pr.title.toLowerCase().includes(q) ||
      repoSlug.includes(q) ||
      pr.author_login.toLowerCase().includes(q)
    );
  });
});
```

The existing `groups` computed reads from `filteredPullRequests.value` instead of `pullRequests.value`. The view-change reset (`activeChips` + `searchQuery`) keeps the filter state per-view; sort persists across views because the user usually wants the same ordering everywhere.

### `fetchView` extension

`fetchView` already calls `invoke('list_dashboard_pull_requests', { view, sort, accountId })`. Wave 3-F extends the arguments to include the active chip set so the backend can apply the chip filters server-side. Until Wave 2-D pins the exact arg shape, the parameter is appended:

```ts
await invoke<DashboardPullRequest[]>("list_dashboard_pull_requests", {
  view: target,
  sort: activeSort.value,
  accountId: accountFilter.value,
  activeChips: Array.from(activeChips.value),
});
```

The Rust command signature stays backwards-compatible by accepting `active_chips: Option<Vec<String>>` with `None` meaning "no chip filter applied".

## File ownership map

### Wave 1 (contract PR - M4-0) - owns everything in this section

- `docs/contracts/triage-ux.md` (this file)
- `docs/adr/0015-triage-state-model.md`
- `src-tauri/migrations/0010_triage_state.sql`
- `src-tauri/src/triage/mod.rs` (module shell + re-exports)
- `src-tauri/src/triage/types.rs` (`FilterChipCounts` struct)
- `src-tauri/src/triage/commands.rs` (three Tauri commands with `unimplemented!()` bodies)
- `src-tauri/src/triage/query.rs` (empty - Wave 2-D fills)
- `src-tauri/src/dashboard/types.rs` (extend `DashboardPullRequest`, widen `DashboardSort`)
- `src-tauri/src/dashboard/query.rs` (default-project the three new fields; route new sort variants through the `Updated` ORDER BY temporarily)
- `src-tauri/src/lib.rs` (mount `triage` module + register the three commands)
- `src/types/dashboard.ts` (mirror DTO + sort extensions)
- `src/stores/dashboard.ts` (mirror DTO + sort extensions inside the duplicated interface; store extensions land in Wave 3-F)
- `docs/adr/README.md` (append ADR 0015 to the index)

### Wave 2 (parallel, after M4-0 merges)

| Issue | Owns | Touches but doesn't own | Don't touch |
|---|---|---|---|
| **A** Read commands | `src/triage/commands.rs::mark_pr_read` + `mark_pr_unread` bodies; the attention-recompute helper that's also called from the sync worker (extract into `src/triage/recompute.rs` if useful). Wire the auto-mark-on-open from the conversation hydrator (`fetch_pr_conversation` calls into the same recompute path after persistence). | `Cargo.toml` | `dashboard::query`, sync worker write path, frontend |
| **B** Sync extension | `src/sync/worker.rs::write_pr_updates` - mention scan + `needs_attention` recompute UPDATE block, gated on the active account's `accounts.login` (Australian English: viewer, not user). Shares the recompute helper with M4-A. | `Cargo.toml` | `triage::commands`, dashboard query, frontend |
| **C** Dashboard SELECT | `src/dashboard/query.rs` - extend `PR_PROJECTION_COLUMNS` with the LEFT JOIN against `pull_request_viewer_relations` (account-scoped) and the unread + needs_attention + mentioned_count_unread projections; flip the defaults from `false`/`0` in `project_pr_row` to read from the row. Also add the sidebar attention-count badge query helper that the sidebar nav consumes. | `Cargo.toml` | sync worker, triage commands, frontend |
| **D** Chip counts + sort | `src/triage/commands.rs::list_filter_chip_counts` body; `src/triage/query.rs` - per-chip count SQL; `src/dashboard/query.rs::build_sql` - replace the temporary all-Updated mapping with the real `Stale` + `NeedsMe` ORDER BYs (incl. the relation LEFT JOIN required for NeedsMe in the Team view path). | `Cargo.toml` | sync worker, triage read commands, frontend |

**Merge order: A and B can land in parallel.** A owns `mark_pr_read` writes; B owns the per-cycle scanner + recompute. They both write to the same columns but on disjoint trigger paths. C reads the persisted state; D builds the chip + sort queries. C and D rebase on A+B.

### Wave 3 (parallel, after Wave 2 lands)

| Issue | Owns | Don't touch |
|---|---|---|
| **E** Components | `src/components/dashboard/FilterChipsBar.vue`, `SortSelector.vue`, `DashboardSearch.vue`. Additions to `src/assets/styles/primitives.css` if a new chip primitive variant is needed; reuse `.chip`/`.seg` first. | `src/stores/dashboard.ts` (M4-F), `PullRequestRow.vue` (M4-F), `DashboardView.vue` host wiring (M4-F) |
| **F** Store + row | `src/stores/dashboard.ts` - `activeChips` / `activeSort` / `searchQuery` / `chipCounts` refs + actions + `filteredPullRequests` computed + view-change reset. `src/views/DashboardView.vue` - mount the three new components and wire the v-model bindings. `src/components/dashboard/PullRequestRow.vue` - unread dot + `.pr.attention` row tint + (optional polish) mention chip. | `src/components/dashboard/FilterChipsBar.vue` + siblings (M4-E) |

E lands first; F rebases and imports E's components. F can also stub the imports against the typed defineProps until E merges.

## Out of scope (deferred)

| Surface | Lands in | Why |
|---|---|---|
| Explicit "Mark all read" power-user action | Polish PR (post-M4) | Menu placement undecided; the auto-mark-on-open handles 95% of the flow. |
| Mention chip on the row | Optional polish in M4-F | Artboard's signal is the unread dot + tint; an explicit mention chip is duplicate. Revisit if user feedback flags ambiguity. |
| FTS5 search backend | post-M4, only if dataset grows | Current v1 dataset is bounded; the dependency cost isn't justified yet. |
| `cmd+K` palette UI | post-v1 | v1 only focuses the existing search input. A command palette is its own design surface. |
| Per-thread mention attribution | Conversation surface (M3+) | Drilling into individual mentions belongs on the conversation surface, not the dashboard row. |
| `@team-handle` resolution | post-v1 | Needs GitHub-org membership data we don't ingest yet. |
| Mention false-positive filtering (code blocks, blockquotes) | Acceptable v1 noise | Documented in "Implementation notes" below; revisit if noise complaints arrive. |

## Implementation notes that aren't part of the interface

These belong here so Wave-2 / Wave-3 agents don't reinvent them, but they don't constrain the public types above.

- **Mention regex.** SQLite's built-in `LIKE` is too loose (matches `@aliceandbob`); the scanner uses a word-boundary regex against `body_text`. With the `regexp` SQLite function (registered via rusqlite's `functions` feature) the pattern is:

  ```text
  (?i)@<viewer-login>(?=[\s.,;:!?)\]'"]|$)
  ```

  Anchored on lookahead so a trailing `@alice.` doesn't bleed past the login, and case-insensitive because GitHub logins normalise that way. The escape set covers ASCII punctuation that commonly trails a mention; non-ASCII trailing characters fall through the lookahead and don't match (acceptable v1 conservatism - a stray Unicode-trailing mention is missed, never over-counted).

- **Word-boundary alternative.** If registering a `REGEXP` function is undesirable, an equivalent in pure SQL works via two `LIKE` patterns plus an `INSTR` check: `body_text LIKE '%@<login> %' OR body_text LIKE '%@<login>.%' OR ... OR body_text LIKE '@<login>%'`. Slower but no native function dependency. Wave 2-B picks one; the existing M3 contract already mandates rusqlite features the worker depends on.

- **Code-block false positives.** A `@alice` inside `` `@alice` `` or a fenced code block still counts. v1 acceptance: the cost of HTML / markdown-aware parsing inside the SQL scanner outweighs the false-positive cost. If real noise emerges, Wave 2-B's helper extracts the scan loop into Rust and pre-strips code blocks before regex matching.

- **Blockquote false positives.** A quoted historical mention (`> @alice please review`) also counts. Same acceptance: the scanner doesn't parse markdown structure. The `mention_scan_watermark_at` cursor prevents the same quoted mention from being counted on every cycle, so a single quoted mention is a one-shot false positive.

- **Mention idempotency under re-fetch.** The lazy hydrator (`fetch_pr_conversation`) re-writes `review_comments` and `issue_comments` rows by `node_id`. The mention scanner reads `created_at`, which is stable across re-writes, so the watermark continues to gate correctly even after a hydrator-driven UPSERT.

- **`needs_attention` recompute scope.** Wave 2-A's helper and Wave 2-B's worker call the same SQL block for a (account, PR) pair. Keeping it as a single function in `triage::recompute` (or wherever Wave 2-A lands it) means the formula only lives in one place; the sync worker and the read/unread commands call into it.

- **View-change reset preserves sort.** The user's "I always sort by Stale" preference shouldn't reset every time they switch from Authored to Watching. Sort persists; chips and search reset.

- **`pnpm tauri dev` validation steps for Wave-2 / Wave-3 implementers.**
  1. Open Authored view, confirm rows render (no regression from contract PR).
  2. Add a PR via the GitHub web UI (or a fixture); verify it appears `unread`.
  3. Open the drawer; close it; confirm the unread dot disappears.
  4. Have a colleague leave a comment containing your login; verify next sync sets the row's `needs_attention` and the sidebar badge increments.
  5. Toggle "CI failing" chip; verify the row count matches the chip's displayed count when the chip is the only one active.
  6. Type a substring into the search input; verify rows filter without a network round-trip and the count updates.

## ADR cross-references

- ADR [0003](../adr/0003-local-storage-sqlite.md) - local SQLite storage that the triage state extends.
- ADR [0004](../adr/0004-sync-polling-with-etag.md) - polling cadence and rate budget; M4 adds zero round-trips.
- ADR [0006](../adr/0006-graphql-first-rest-fallback.md) - GraphQL-first stance; M4 reuses already-persisted comment bodies.
- ADR [0010](../adr/0010-conversation-depth-storage.md) - thread storage that the mention scan reads from.
- ADR [0012](../adr/0012-threads-bar-four-state-and-outdated-counted.md) - threads-bar buckets that feed the "Unresolved threads" chip predicate.
- ADR [0013](../adr/0013-user-avatars-cache.md) - the `users` table layered alongside this contract.
- ADR [0015](../adr/0015-triage-state-model.md) - this contract's accompanying decision record.
