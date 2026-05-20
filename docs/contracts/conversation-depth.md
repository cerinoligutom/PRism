# Conversation depth interface contract

This document is the shared interface contract for **M3: per-thread state, comment-type breakdown, conversation stats, per-thread previews**. It pins the schema additions, GraphQL query updates, sync-cycle changes, Tauri command shape, conversation-stats math, frontend component interfaces, and the file-ownership map for the three Wave-2 back-end issues and three Wave-3 front-end issues that fan out from it.

If you're implementing any M3 issue, **read this end-to-end before writing code**. Anything ambiguous is a spec bug — open a PR or comment on the issue to refine the contract rather than silently diverging.

## Why this exists

M2's [`dashboard-data.md`](dashboard-data.md) demonstrated the pattern: one contract PR lands the shared scaffolding (types, file boundaries, schema), then parallel agents implement against the contract without conflicting on `Cargo.toml`, `lib.rs`, migrations, or GraphQL query constants. M3 has six implementation issues fanning out from this contract (three back-end, three front-end) plus a couple of touch points on the existing sync worker and dashboard query — the contract carries the spec so each implementer can work without coordination round-trips.

The decisions encoded here were agreed in the scoping discussion before this doc was drafted:

- **Detail-surface UX is configurable.** Drawer (default) + dedicated route ship in M3. Inline expansion is the reserved third option for a post-M3 follow-up via the same `prDetailSurface` settings selector.
- **Comment-fetch strategy is capped + lazy.** Sync cycle pulls thread headers + head comment + counts. Full comment bodies hydrated by a new `fetch_pr_conversation` Tauri command when the user opens the drawer / route.
- **Row threads bar reads from pre-aggregated rollup columns** on `pull_requests`, mirroring M2's CI rollup. Same write path, same query layer.
- **Reviews tab ships in M3.** Review bodies are needed anyway for the comment-type "summary" tile, so the UI cost is one component on top of free data.
- **Outdated threads are counted but hidden by default** in the threads list, surfaced via a toggle.

## Scope

### In M3

- Per-thread state persisted (`is_resolved`, `is_outdated`, head-comment snapshot, line + start_line, timestamps, reply count).
- Conversation stats: oldest unresolved, avg time-to-response, resolution rate, comment-type breakdown (review / issue / summary).
- Per-thread previews on the dashboard row (segmented threads bar driven by pre-aggregated rollup).
- Drawer host + dedicated route for the conversation surface; settings selector to switch between them.
- Reviews tab on the conversation surface with state + body + timestamp per submitted review.
- Status timeline tab backed by the M2 `status_timeline.rs` derivation (visual only — no new backend).

### Deferred (do not implement)

- **Inline expansion host** — initially reserved as the `'inline'` value on the `prDetailSurface` settings selector; **cancelled before launch** per ADR 0011. Drawer + route cover the v1 detail-surface need; if demand surfaces post-launch, inline is re-introduced via a fresh ADR rather than inheriting the v1 reservation.
- **Files tab / inline diff viewer** — post-v1 per the wiki roadmap.
- **Comment composer / write actions** — out of scope for v1 (read-only).
- **Per-check Checks expansion** — the dashboard-expanded artboard shows per-check rows; M3 keeps the existing `CiBadge` rollup. Lands in M6 polish.
- **Read-state tracking / unread dots** — M4 (needs a separate spec).
- **"Needs my attention" composite signal** — M4.

The conversation content component (`PullRequestConversation.vue`) is host-agnostic so any future host (e.g. a revived inline expansion) wires it in without component rewrites.

## Module layout

```
src-tauri/
  migrations/
    0004_conversation_depth.sql      # Wave 1 — owned by the contract PR
  src/
    conversation/                    # Wave 1 creates module shell; Wave 2-B implements
      mod.rs                         # public surface + Tauri command registration re-exports
      types.rs                       # PullRequestThread / ConversationStats / Review DTOs
      query.rs                       # Wave 2-B — read-side SQL composition + stats math
      commands.rs                    # Wave 2-B — Tauri command bodies (incl. lazy hydrator)
    sync/
      worker.rs                      # Wave 2-A extends write_pr_updates; Wave 2-C extends rollup writes
    github/
      graphql/
        queries.rs                   # Wave 2-A extends PR_DETAIL_QUERY; Wave 2-B adds PR_COMMENTS_QUERY
    dashboard/
      types.rs                       # Wave 2-C extends DashboardPullRequest with `threads` field
      query.rs                       # Wave 2-C extends SELECT + hydration

src/
  components/
    dashboard/
      PullRequestRow.vue             # Wave 3-D extends with threads column
      ThreadsBar.vue                 # Wave 3-D NEW
    conversation/                    # Wave 3-E + 3-F NEW
      PullRequestConversation.vue    # Wave 3-E — host-agnostic content
      ThreadsList.vue                # Wave 3-E
      ConversationStats.vue          # Wave 3-E
      ReviewsTab.vue                 # Wave 3-E
      StatusTimelineTab.vue          # Wave 3-E
      PullRequestDrawer.vue          # Wave 3-F — drawer host
  views/
    PullRequestDetailView.vue        # Wave 3-F — route host
    DashboardView.vue                # Wave 3-F wires `@open` to active surface
    settings/
      AppearanceSettings.vue         # Wave 3-F adds prDetailSurface selector
  stores/
    conversation.ts                  # Wave 3-E NEW
    dashboard.ts                     # Wave 3-F extends with expandedPullRequestId
    appearance.ts                    # Wave 3-F extends with prDetailSurface
  router/
    index.ts                         # Wave 3-F adds /dashboard/:view/pr/:id route
  types/
    dashboard.ts                     # Wave 2-C extends DTO type
    conversation.ts                  # Wave 1 NEW — mirrors the Rust DTOs
```

## Schema additions

The contract PR lands the full migration. Wave-2 agents must not edit this file — additional columns post-M3 go in `0005+`.

```sql
-- src-tauri/migrations/0004_conversation_depth.sql

-- ----------------------------------------------------------------
-- review_threads: per-thread state needed by the threads list and
-- the conversation-stats math.
-- ----------------------------------------------------------------

-- GraphQL node id — required for upserts (ReviewThread has no databaseId).
ALTER TABLE review_threads ADD COLUMN node_id              TEXT;

-- Outdated state — surfaced in the threads list; counted in total but not
-- in unresolved.
ALTER TABLE review_threads ADD COLUMN is_outdated          INTEGER NOT NULL DEFAULT 0;

-- Timestamps (unix seconds) needed by the conversation stats.
ALTER TABLE review_threads ADD COLUMN created_at           INTEGER;
ALTER TABLE review_threads ADD COLUMN resolved_at          INTEGER;
ALTER TABLE review_threads ADD COLUMN last_reply_at        INTEGER;

-- Reply count — denormalised from review_comments so the list query
-- doesn't need a sub-aggregation.
ALTER TABLE review_threads ADD COLUMN reply_count          INTEGER NOT NULL DEFAULT 0;

-- Head-comment snapshot — first comment in the thread, surfaced as the
-- preview snippet on the threads list. Populated from the cycle's
-- `comments(first:1)` head; full bodies live in review_comments after
-- lazy hydration.
ALTER TABLE review_threads ADD COLUMN head_comment_author_login   TEXT;
ALTER TABLE review_threads ADD COLUMN head_comment_body_text      TEXT;
ALTER TABLE review_threads ADD COLUMN head_comment_created_at     INTEGER;

-- Line range (single line or multi-line block comment).
ALTER TABLE review_threads ADD COLUMN line                  INTEGER;
ALTER TABLE review_threads ADD COLUMN start_line            INTEGER;
-- `original_line` already exists from 0001_init.sql.

CREATE UNIQUE INDEX idx_review_threads_node_id
    ON review_threads (node_id)
    WHERE node_id IS NOT NULL;

-- Threads list queries filter by PR + (resolved OR outdated) — partial index
-- on the unresolved-and-active set keeps the threads list fast.
CREATE INDEX idx_review_threads_pr_active
    ON review_threads (pull_request_id)
    WHERE is_resolved = 0 AND is_outdated = 0;

-- ----------------------------------------------------------------
-- review_comments: lazy-hydrated per-thread comment bodies.
-- ----------------------------------------------------------------

-- GraphQL node id + REST databaseId — either form may upsert depending
-- on which lazy-fetch path produced the row.
ALTER TABLE review_comments ADD COLUMN node_id              TEXT;
ALTER TABLE review_comments ADD COLUMN database_id          INTEGER;

-- Line + side (LEFT / RIGHT) for inline rendering. Mostly informational
-- in M3 (no diff viewer); persisted so M4+ can use them without backfill.
ALTER TABLE review_comments ADD COLUMN line                 INTEGER;
ALTER TABLE review_comments ADD COLUMN side                 TEXT;

CREATE UNIQUE INDEX idx_review_comments_node_id
    ON review_comments (node_id)
    WHERE node_id IS NOT NULL;

CREATE INDEX idx_review_comments_thread
    ON review_comments (review_thread_id, created_at);

-- ----------------------------------------------------------------
-- issue_comments: lazy-hydrated PR-level comment bodies.
-- ----------------------------------------------------------------

ALTER TABLE issue_comments ADD COLUMN node_id              TEXT;
ALTER TABLE issue_comments ADD COLUMN database_id          INTEGER;

CREATE UNIQUE INDEX idx_issue_comments_node_id
    ON issue_comments (node_id)
    WHERE node_id IS NOT NULL;

CREATE INDEX idx_issue_comments_pr
    ON issue_comments (pull_request_id, created_at);

-- ----------------------------------------------------------------
-- reviews: each submitted PullRequestReview (state + body).
-- ----------------------------------------------------------------

ALTER TABLE reviews ADD COLUMN node_id                     TEXT;

CREATE UNIQUE INDEX idx_reviews_node_id
    ON reviews (node_id)
    WHERE node_id IS NOT NULL;

CREATE INDEX idx_reviews_pr_submitted_at
    ON reviews (pull_request_id, submitted_at);

-- ----------------------------------------------------------------
-- pull_requests: rollup columns for the dashboard row (cheap to
-- aggregate at write time; mirrors M2 ci_total / ci_passing).
-- ----------------------------------------------------------------

ALTER TABLE pull_requests ADD COLUMN threads_total         INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests ADD COLUMN threads_unresolved    INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests ADD COLUMN threads_involved      INTEGER NOT NULL DEFAULT 0;

-- Cycle-time counter for the issue_comments contribution to the
-- comment-type breakdown. Bodies are hydrated lazily but the count
-- is read every cycle from `totalCount`.
ALTER TABLE pull_requests ADD COLUMN issue_comments_count  INTEGER NOT NULL DEFAULT 0;
```

### Rationale for the schema choices

- **`node_id TEXT` as the upsert key.** GitHub's GraphQL `ReviewThread` exposes only the global node ID (a string); there's no `databaseId`. Keeping the existing `INTEGER PRIMARY KEY` for cheap foreign keys and adding `node_id TEXT UNIQUE` for upserts is cleaner than rewriting the PK. The same pattern extends to `review_comments`, `issue_comments`, and `reviews` for consistency.
- **Partial unique index on `node_id`.** Migrating existing rows (`0001_init.sql` rows seeded by M1 tests, etc.) means `node_id` is NULL initially. A partial unique index lets us enforce uniqueness for populated rows without rejecting the NULLs.
- **`is_outdated` separate from `is_resolved`.** GraphQL exposes them as orthogonal booleans; the threads-list UX hides outdated threads independently of resolution state. Counting math (below) treats outdated as non-active rather than as a resolution state.
- **Head-comment snapshot on the thread row.** The dashboard row needs a one-line preview without joining `review_comments`. Snapshot columns on the thread save the join and survive the lazy-hydration cycle (the snapshot persists even when the full comment array hasn't been hydrated yet).
- **`reply_count` denormalised.** The threads list shows reply counts; pre-aggregating saves a sub-query per thread on every list render.
- **Rollup columns on `pull_requests`.** Same pattern M2 established with `ci_total` / `ci_passing` — the dashboard row needs the counts without a sub-aggregation, and the worker already touches the PR row on every cycle.
- **No separate `thread_viewer_relations` table.** "You're in" is computed at query time via `accounts.login = review_comments.author_login` for the active account. With one or two accounts (the common v1 case) the join is cheap; a relations table is overkill until multi-account scaling proves it isn't.

## GraphQL queries

### Extension: `PR_DETAIL_QUERY` additions

Wave 2-A extends `PR_DETAIL_QUERY` in `src-tauri/src/github/graphql/queries.rs`. New fields layered into the existing query:

```graphql
reviewThreads(first: 100) {
  pageInfo { hasNextPage endCursor }
  nodes {
    id
    isResolved
    isOutdated
    path
    line
    startLine
    originalLine
    comments(first: 1) {
      totalCount
      nodes {
        id
        author { login }
        bodyText
        createdAt
      }
    }
  }
}

reviews(first: 30) {
  nodes {
    id
    state
    body
    submittedAt
    author { login }
  }
}

issueComments(first: 50) {
  totalCount
}
```

- `reviewThreads` already exists; M3-A extends the inner selection.
- `reviews` is new — M2 only read the aggregate `reviewDecision`.
- `issueComments` is new; M3-A reads `totalCount` and writes it to `pull_requests.issue_comments_count`. No per-comment node persistence in the cycle.

Wave 2-A also extends the deserialiser structs in `queries.rs` for the new fields.

### New: `PR_COMMENTS_QUERY`

Wave 2-B adds a new query string for the lazy hydrator. Called once per `fetch_pr_conversation` invocation, paginated if needed.

```graphql
query PrComments($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          comments(first: 100) {
            pageInfo { hasNextPage endCursor }
            nodes {
              id
              databaseId
              author { login }
              body
              bodyText
              createdAt
              path
              line
              originalLine
              side
            }
          }
        }
      }
      issueComments(first: 100) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          databaseId
          author { login }
          body
          bodyText
          createdAt
        }
      }
    }
  }
}
```

Pagination notes: in v1 the lazy hydrator pages once if needed and caps at 200 comments per thread / 200 issue comments per PR. PRs beyond these limits log a warning; the cap is the practical upper bound the threads-list UI is sized for.

## Sync cycle changes

The M2 cycle (per account) runs Discovery → Team → Enrichment → Pruning. M3 modifies only the **Enrichment** phase. Discovery, Team, and Pruning are unchanged.

Enrichment additions per PR:

1. Extended `PR_DETAIL_QUERY` returns thread / review / issue-comment-count data alongside the existing M2 fields.
2. `write_pr_updates` upserts `review_threads` rows by `node_id`, populating timestamps + head-comment snapshot + reply count from `comments.totalCount`.
3. `write_pr_updates` upserts `reviews` rows by `node_id`.
4. `write_pr_updates` writes `pull_requests.issue_comments_count` from `issueComments.totalCount`.
5. After all per-PR writes for a cycle, the worker recomputes `pull_requests.threads_total / threads_unresolved / threads_involved` from the just-written rows for that account. The recompute is a single SQL aggregation per PR (see "Dashboard rollup" below).
6. Threads / reviews removed on GitHub are pruned: any `review_threads` / `reviews` row whose `node_id` doesn't appear in the latest fetch is deleted. Comments cascade.

### Rate budget impact

- `PR_DETAIL_QUERY` grows but stays one round-trip per PR per cycle. Empirically the additional fields add ~20–40% to response size on active PRs; rate-budget arithmetic is unchanged (still bounded by the existing 100-PRs-per-cycle cap).
- `fetch_pr_conversation` is an off-cycle command, not counted in the sync envelope. One round-trip per drawer / route open. With the 200-comment / 200-issue-comment caps, the worst case is two paginated requests; typical case is one.

## Tauri command surface

```rust
// src-tauri/src/conversation/types.rs

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThreadState {
    Unresolved,
    Resolved,
    Outdated,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PullRequestThread {
    pub id: i64,
    pub node_id: String,
    pub pull_request_id: i64,
    pub state: ThreadState,
    pub path: Option<String>,
    pub line: Option<i64>,
    pub start_line: Option<i64>,
    pub original_line: Option<i64>,
    pub reply_count: i64,
    pub head_comment: Option<ThreadHeadComment>,
    pub created_at: Option<i64>,
    pub resolved_at: Option<i64>,
    pub last_reply_at: Option<i64>,
    /// True when the active account's login appears as a comment author
    /// anywhere in this thread.
    pub is_you_in: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThreadHeadComment {
    pub author_login: String,
    pub body_text: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConversationStats {
    pub threads_total: i64,
    pub threads_unresolved: i64,
    pub threads_resolved: i64,
    pub threads_outdated: i64,
    /// Oldest `review_threads.created_at` among non-resolved + non-outdated
    /// threads. `None` when there are zero active threads.
    pub oldest_unresolved_at: Option<i64>,
    /// Average gap (in seconds) between consecutive `review_comments.created_at`
    /// within each thread, averaged across threads with >= 2 comments.
    /// `None` when no thread has a reply yet.
    pub avg_response_seconds: Option<i64>,
    /// `active_resolved / (total - outdated)`, where `active_resolved`
    /// counts threads with `is_resolved = 1 AND is_outdated = 0`. `0.0`
    /// when total-non-outdated is zero. Stays in `[0.0, 1.0]`.
    pub resolution_rate: f64,
    pub comment_breakdown: CommentBreakdown,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommentBreakdown {
    pub review: i64,    // count of review_comments rows for this PR
    pub issue: i64,     // pull_requests.issue_comments_count
    pub summary: i64,   // count of reviews with non-empty body
    pub total: i64,     // sum of the three above
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PullRequestReview {
    pub id: i64,
    pub node_id: String,
    pub author_login: String,
    /// GraphQL `PullRequestReviewState`: APPROVED, CHANGES_REQUESTED,
    /// COMMENTED, DISMISSED, PENDING.
    pub state: String,
    pub body: Option<String>,
    pub submitted_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ThreadComment {
    pub id: i64,
    pub thread_id: i64,
    pub author_login: String,
    pub body: String,
    pub created_at: i64,
    pub line: Option<i64>,
    pub side: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IssueComment {
    pub id: i64,
    pub author_login: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HydratedConversation {
    pub pull_request_id: i64,
    pub threads: Vec<PullRequestThread>,
    pub thread_comments: Vec<ThreadComment>,
    pub issue_comments: Vec<IssueComment>,
    pub reviews: Vec<PullRequestReview>,
    pub stats: ConversationStats,
}
```

```rust
// src-tauri/src/conversation/commands.rs

/// List per-thread state for a PR. Reads from the local cache only; no
/// network round-trip. Always returns the latest sync-cycle snapshot.
#[tauri::command]
pub fn list_pr_threads(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
) -> Result<Vec<PullRequestThread>, String>;

/// Compute conversation stats for a PR from the local cache.
#[tauri::command]
pub fn get_pr_conversation_stats(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
) -> Result<ConversationStats, String>;

/// Lazy hydration: fetch full thread replies + issue-comment bodies from
/// GitHub, persist them, return the hydrated DTO. Called when the drawer /
/// route mounts.
///
/// Idempotent — subsequent calls within the same cache window re-render
/// from SQLite without a new network round-trip when the underlying
/// `pull_requests.updated_at` is unchanged.
#[tauri::command]
pub async fn fetch_pr_conversation(
    pull_request_id: i64,
    db: State<'_, DbHandle>,
    clients: State<'_, ClientFactoryHandle>,
    accounts: State<'_, AccountStoreHandle>,
) -> Result<HydratedConversation, String>;
```

The lazy-hydrator signature uses Tauri's `async` command form because the GitHub round-trip is non-trivial; the other two are synchronous DB reads.

## Conversation stats math

All stats are computed at read time inside `conversation::query::get_conversation_stats`. The SQL is one CTE per metric; the function returns a single `ConversationStats` row.

### Oldest unresolved

```sql
SELECT MIN(created_at)
FROM   review_threads
WHERE  pull_request_id = ?
  AND  is_resolved = 0
  AND  is_outdated = 0
  AND  created_at IS NOT NULL;
```

Returned as a unix timestamp; frontend renders relative.

### Avg time-to-response

Per-thread gaps between consecutive `review_comments.created_at`, averaged across threads with ≥ 2 comments. The frontend renders relative ("3h 12m").

```sql
WITH gaps AS (
  SELECT
    c.review_thread_id,
    c.created_at -
      LAG(c.created_at) OVER (PARTITION BY c.review_thread_id ORDER BY c.created_at)
      AS gap_seconds
  FROM review_comments c
  JOIN review_threads t ON t.id = c.review_thread_id
  WHERE t.pull_request_id = ?
)
SELECT AVG(gap_seconds) FROM gaps WHERE gap_seconds IS NOT NULL;
```

`NULL` when no thread has a reply yet. Frontend renders an em-dash placeholder in that case.

### Resolution rate

```
active_resolved / (total - outdated)

where  active_resolved = COUNT(*) WHERE is_resolved = 1 AND is_outdated = 0
       outdated        = COUNT(*) WHERE is_outdated = 1
       total           = COUNT(*)
```

The numerator is strict-active — threads that are *both* resolved AND outdated count only in the outdated bucket, not in the resolved numerator. GitHub's `isResolved` and `isOutdated` are orthogonal, so the resolved-and-outdated intersection is real; counting it in the numerator while excluding it from the denominator (the original implementation) made the rate overshoot 100% on PRs where code changes invalidated previously-resolved threads.

The three visible buckets — `threads_unresolved`, `threads_resolved`, `threads_outdated` — are disjoint over the active set, so they line up with the threads-list "Show N outdated" toggle and the segmented threads bar.

`0.0` when `(total - outdated)` is zero. Returned as a `f64` in `[0.0, 1.0]`; frontend renders as percent.

### Comment-type breakdown

```sql
SELECT
  (SELECT COUNT(*) FROM review_comments c
     JOIN review_threads t ON t.id = c.review_thread_id
     WHERE t.pull_request_id = ?)                                   AS review_count,
  (SELECT issue_comments_count FROM pull_requests WHERE id = ?)     AS issue_count,
  (SELECT COUNT(*) FROM reviews
     WHERE pull_request_id = ? AND body IS NOT NULL AND body <> '') AS summary_count;
```

`total = review + issue + summary`.

Note that `review_count` reads `review_comments` directly (not the thread-level `reply_count` denormalisation) so the number reflects post-lazy-hydration state. Pre-hydration the count is zero — the comment-type breakdown tile reads as `0 review · N issue · M summary` until the drawer / route is opened, at which point the next stats fetch reflects the full count. Acceptable for v1; documented for future read-state work.

## Dashboard rollup

Wave 2-C writes the rollup columns inside `write_pr_updates` after the thread upserts have committed. One UPDATE per PR:

```sql
UPDATE pull_requests
SET
  threads_total = (
    SELECT COUNT(*) FROM review_threads
    WHERE pull_request_id = ?
  ),
  threads_unresolved = (
    SELECT COUNT(*) FROM review_threads
    WHERE pull_request_id = ?
      AND is_resolved = 0
      AND is_outdated = 0
  ),
  threads_involved = (
    SELECT COUNT(DISTINCT t.id) FROM review_threads t
    JOIN review_comments c ON c.review_thread_id = t.id
    JOIN accounts a ON a.login = c.author_login
    WHERE t.pull_request_id = ?
      AND a.id = ?
  )
WHERE id = ?;
```

The `?` placeholders are `pull_request_id` (three times), `account_id`, `pull_request_id`. The `threads_involved` computation is per-account; the cycle runs per-account so this naturally writes the correct value.

`DashboardPullRequest` grows one field:

```rust
pub struct DashboardPullRequest {
    // ... existing fields
    pub threads: Option<ThreadsSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadsSummary {
    pub total: i64,
    pub unresolved: i64,
    pub involved: i64,
}
```

`None` when the PR has never had a thread (newly discovered PR before its first enrichment). The frontend renders the muted em-dash state.

## Frontend component interfaces

### `ThreadsBar.vue`

```ts
defineProps<{
  threads: ThreadsSummary | null;
}>();
```

Three CSS-variable-driven segments: `unresolved`, `involved`, `resolved`. The `null` and `threads.total === 0` cases render the muted bar + em-dash count.

### `PullRequestRow.vue` (extension)

Grid template grows from 7 to 8 columns. New column between the title block and the reviewer stack:

```
| strip | num | title-col | threads | reviewers | ci | time | kebab |
```

The component reads `pullRequest.threads` and renders `<ThreadsBar :threads="pullRequest.threads" />`.

### `PullRequestConversation.vue`

Host-agnostic content. Mounts the tabs and triggers the lazy fetch.

```ts
defineProps<{
  pullRequestId: number;
}>();
```

On mount: calls `invoke('fetch_pr_conversation', { pullRequestId })`, stores the result in `useConversationStore`, renders the active tab. Subsequent mounts for the same id read from the store without a refetch.

### `ThreadsList.vue`

```ts
defineProps<{
  threads: PullRequestThread[];
  showOutdated?: boolean;       // default false
}>();
```

Renders per-thread cards per the dashboard-expanded artboard. `unresolved && is_you_in` gets the "YOU'RE IN" badge + accent gradient highlight. Outdated threads hidden when `showOutdated === false` (default).

### `ConversationStats.vue`

```ts
defineProps<{
  stats: ConversationStats;
}>();
```

Renders the 2×2 stat grid. Each tile reads from `stats`; em-dash placeholders for `null` fields.

### `ReviewsTab.vue`

```ts
defineProps<{
  reviews: PullRequestReview[];
}>();
```

List of reviews ordered by `submitted_at` desc. State pill (Approved / Changes / Commented / Dismissed / Pending) + author + body + relative timestamp.

### `StatusTimelineTab.vue`

```ts
defineProps<{
  pullRequest: DashboardPullRequest;
}>();
```

Reads from the existing `pull_requests.latest_status_change_*` columns and (post-M3) the `timeline_events` table. v1 renders the qualifying event list per ADR 0007.

### `PullRequestDrawer.vue`

```ts
defineProps<{
  pullRequestId: number | null;
}>();

defineEmits<{
  close: [];
}>();
```

Right-hand overlay. `pullRequestId !== null` opens the drawer; `null` keeps it closed. Focus trap inside; Esc emits `close`. The host (`DashboardView`) sets / clears the id.

### `PullRequestDetailView.vue` (route)

Reads the route param `:id` and mounts `<PullRequestConversation :pull-request-id="id" />` with a back-navigation header. No drawer chrome.

### Pinia store (`src/stores/conversation.ts`)

```ts
export const useConversationStore = defineStore('conversation', () => {
  const cache = ref<Map<number, HydratedConversation>>(new Map());
  const loading = ref<Set<number>>(new Set());

  async function load(pullRequestId: number): Promise<HydratedConversation> {
    if (cache.value.has(pullRequestId)) return cache.value.get(pullRequestId)!;
    if (loading.value.has(pullRequestId)) {
      // de-duplicate concurrent mounts (e.g. drawer + status-bar prefetch).
      return new Promise(/* wait for the in-flight load */);
    }
    loading.value.add(pullRequestId);
    try {
      const result = await invoke<HydratedConversation>(
        'fetch_pr_conversation',
        { pullRequestId },
      );
      cache.value.set(pullRequestId, result);
      return result;
    } finally {
      loading.value.delete(pullRequestId);
    }
  }

  function invalidate(pullRequestId: number): void {
    cache.value.delete(pullRequestId);
  }

  return { cache, loading, load, invalidate };
});
```

Invalidation hook: the existing `sync://status` event subscriber should call `invalidate(pullRequestId)` for the PR that just had its enrichment phase complete, so a re-open re-fetches. Out of M3 scope as a polish item; the cache is acceptable for v1 because the open-once-then-close pattern is the common case.

### Appearance store extension (`src/stores/appearance.ts`)

```ts
type PrDetailSurface = 'drawer' | 'route';

const surface = ref<PrDetailSurface>('drawer');
// An `'inline'` third surface was initially reserved here and cancelled
// before launch (ADR 0011). Persisted `'inline'` values from earlier
// builds are coerced back to `'drawer'` on hydrate.
```

### Dashboard store extension (`src/stores/dashboard.ts`)

```ts
const expandedPullRequestId = ref<number | null>(null);

function openPullRequest(pr: DashboardPullRequest, router: Router): void {
  const surface = useAppearanceStore().prDetailSurface;
  if (surface === 'drawer') {
    expandedPullRequestId.value = pr.id;
  } else {
    router.push({ name: 'pr-detail', params: { view: view.value, id: pr.id } });
  }
}
```

### Router extension

```ts
{
  path: '/dashboard/:view/pr/:id',
  name: 'pr-detail',
  component: () => import('@/views/PullRequestDetailView.vue'),
  props: route => ({ pullRequestId: Number(route.params.id) }),
}
```

## File ownership map

### Wave 1 (contract PR — M3-0) — owns everything in this section

- `docs/contracts/conversation-depth.md` (this file)
- `docs/adr/0010-conversation-depth-storage.md`
- `src-tauri/migrations/0004_conversation_depth.sql` (the full migration above)
- `src-tauri/src/conversation/mod.rs` (module shell + re-exports)
- `src-tauri/src/conversation/types.rs` (DTO enums + structs from the contract)
- `src-tauri/src/conversation/commands.rs` (three Tauri commands with `unimplemented!()` bodies so the types check)
- `src-tauri/src/conversation/query.rs` (empty module — Wave 2-B fills)
- `src/types/conversation.ts` (TypeScript mirror of the Rust DTOs)
- `src-tauri/src/lib.rs` (mount `conversation` module + register the three commands)

### Wave 2 (parallel, after M3-0 merges)

| Issue | Owns | Touches but doesn't own | Don't touch |
|-------|------|------------------------|-------------|
| **A** Sync extension | `src/github/graphql/queries.rs` — extends `PR_DETAIL_QUERY` body + `PullRequestDetail` struct + new sibling structs for `reviews` / `reviewThreads.comments` / `issueComments.totalCount`; `src/sync/worker.rs::write_pr_updates` — upserts for `review_threads`, `reviews`, `pull_requests.issue_comments_count` | `Cargo.toml` | `src/conversation/`, `src/dashboard/`, `PR_COMMENTS_QUERY` (M3-B), `pull_requests.threads_*` writes (M3-C) |
| **B** Conversation query + commands | `src/conversation/query.rs`, `src/conversation/commands.rs` bodies, `src/github/graphql/queries.rs::PR_COMMENTS_QUERY` (new constant + deserialiser structs only) | `Cargo.toml` | `src/sync/`, `PR_DETAIL_QUERY` body, `src/dashboard/`, `pull_requests.threads_*` writes |
| **C** Dashboard rollup | `src/sync/worker.rs::write_pr_updates` — adds the `pull_requests.threads_*` UPDATE block after thread upserts; `src/dashboard/types.rs::DashboardPullRequest` — extends with `threads: Option<ThreadsSummary>`; `src/dashboard/query.rs` — extends SELECT + hydration; `src/types/dashboard.ts` — extends DTO type | `Cargo.toml` | `src/conversation/`, `PR_DETAIL_QUERY` / `PR_COMMENTS_QUERY`, `review_threads` writes (M3-A owns) |

**Merge order: A → C → B.** A owns the canonical thread / review upserts; C extends `write_pr_updates` to write the rollup that depends on A's writes; B reads the persisted state. A and C touch the same function — C rebases on A.

### Wave 3 (parallel, after Wave 2 lands)

| Issue | Owns | Don't touch |
|-------|------|-------------|
| **D** Threads bar + row | `src/components/dashboard/ThreadsBar.vue`, `src/components/dashboard/PullRequestRow.vue` (extension), additions to `src/assets/styles/primitives.css` if a segment-bar primitive is needed | `src/views/`, `src/stores/`, `src/components/conversation/` |
| **E** Conversation content | `src/components/conversation/PullRequestConversation.vue`, `ThreadsList.vue`, `ConversationStats.vue`, `ReviewsTab.vue`, `StatusTimelineTab.vue`, `src/stores/conversation.ts` | drawer / route hosts (M3-F), `src/components/dashboard/` (M3-D), `src/router/`, `src/views/settings/` |
| **F** Hosts + settings | `src/components/conversation/PullRequestDrawer.vue`, `src/views/PullRequestDetailView.vue`, `src/router/index.ts` (route addition), `src/stores/dashboard.ts` (extends with `expandedPullRequestId` + open helper), `src/stores/appearance.ts` (extends with `prDetailSurface`), `src/views/settings/AppearanceSettings.vue` (selector), `src/views/DashboardView.vue` (`@open` wiring) | `src/components/conversation/PullRequestConversation.vue` + siblings (M3-E), `src/components/dashboard/` (M3-D) |

E lands first; F rebases on top and imports E's content component. Alternatively F stubs the content import against the typed interface and rebases when E merges.

## Out of scope (deferred)

| Surface | Lands in | Why |
|---------|----------|-----|
| Inline expansion host | **cancelled (ADR 0011)** | Heavy DOM injection, focus management across compressed siblings, list-virtualisation interaction — for a non-default UX with no demand signal. Drawer + route are sufficient; revisit via a fresh ADR if user feedback changes that. |
| Files tab / inline diff | post-v1 | Inline diff viewer is post-v1 per the wiki roadmap. |
| Per-check Checks rows | M6 polish | Dashboard-expanded artboard shows per-check rows; M3 keeps the existing rollup `CiBadge`. |
| Read-state tracking | M4 | Requires a separate spec for what counts as "read" and how the state is stored. |
| "Needs my attention" composite | M4 | Composite signal — needs the threads-involved data this contract delivers plus M4-only signals. |
| Comment composer / write actions | post-v1 | v1 is read-only. |

## Implementation notes that aren't part of the interface

These belong here so Wave-2 / Wave-3 agents don't reinvent them, but they don't constrain the public types above.

- **Pruning removed threads.** After `write_pr_updates` upserts the threads returned by the latest fetch, delete any `review_threads` rows for the PR whose `node_id` doesn't appear in the fetched set. Comments cascade via the existing foreign key. Same pattern applies to `reviews`.
- **Lazy hydrator atomicity.** `fetch_pr_conversation` writes all comments + issue comments inside a single transaction so a half-fetched state never leaks. On error, the previous cached state is preserved.
- **Cache key for the conversation store.** Key by `pull_request_id` only — the `accounts.login = author_login` join for `is_you_in` is computed at SQL time and reflected in the returned DTO.
- **Body-text vs body.** `bodyText` is GraphQL's pre-rendered plain text (markdown stripped). The threads-list snippet uses `body_text`; the Reviews tab and full comment view render the markdown `body`. Both are persisted.
- **PullRequestReviewState `PENDING`.** A reviewer who hasn't submitted appears in `requested_reviewers` (M2) but not in `reviews`. The Reviews tab merges both sources: submitted reviews from `reviews` + pending placeholders from `requested_reviewers` where no `reviews` row exists for that login on this PR.
- **Outdated toggle persistence.** The `showOutdated` toggle state is local to the `ThreadsList.vue` component — not persisted. Re-opening the drawer / route starts with outdated hidden. If user signal demands it, persist via the appearance store post-M3.
- **Status timeline tab.** Reads from the existing `latest_status_change_*` columns plus the `timeline_events` table populated by M1. No backend work in M3; the visual is the new piece.

## ADR cross-references

- ADR [0004](../adr/0004-sync-polling-with-etag.md) — polling cadence and rate budget; the extended `PR_DETAIL_QUERY` still fits within the existing envelope.
- ADR [0006](../adr/0006-graphql-first-rest-fallback.md) — GraphQL-first stance; `PR_COMMENTS_QUERY` uses GraphQL for the same reasons.
- ADR [0007](../adr/0007-status-timeline-from-timeline-events-api.md) — the status-timeline tab consumes the derivation this ADR pinned.
- ADR 0010 (to be authored alongside this contract) — records the thread-ID storage choice, the pre-aggregate rollup decision, and the lazy-hydrate-on-detail-open strategy.
