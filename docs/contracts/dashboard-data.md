# Dashboard data interface contract

This document is the shared interface contract for **M2: the dashboard PR list**. It pins the schema additions, GraphQL query updates, sync-cycle changes, Tauri command shape, frontend DTO + component props, and the file-ownership map for the four Wave-2 back-end issues and two Wave-3 front-end issues that fan out from it.

If you're implementing any M2 issue, **read this end-to-end before writing code**. Anything ambiguous is a spec bug — open a PR or comment on the issue to refine the contract rather than silently diverging.

## Why this exists

M1's [`github-client.md`](github-client.md) demonstrated the pattern: one contract PR lands the shared scaffolding (types, file boundaries, schema), then parallel agents implement against the contract without conflicting on `Cargo.toml`, `lib.rs`, migrations, or GraphQL query constants. M2 has more parallel surface than M1 (six implementation issues vs. four), so the contract is correspondingly heavier.

The decisions encoded here were agreed in scope discussion before this doc was drafted:

- **Team view ships in M2**, which makes repo discovery a Wave-2 issue (not a deferral).
- **PR discovery uses GitHub's Search API** for the three viewer-centric views (Authored / Assigned / Watching). See ADR 0009.
- **Relations are stored** in a new `pull_request_viewer_relations` table keyed by `(account_id, pull_request_id)`. Booleans on `pull_requests` would couple the row to a single account and break under multi-account.
- **Sort UI is deferred to M4.** M2 ships with a single hard-coded "Updated desc" order.
- **Filter chips, search, threads bar, unread dot, and "needs my attention" highlight are deferred** to M3 / M4. The row component is built to the artboard so M3 / M4 can light up additional slots without rewriting it.

## Scope

### In M2

- Sidebar with four views (Authored / Assigned / Watching / Team) — live counts per view.
- Group by Repo (default), Org, or None — visual grouping done on the frontend.
- PR row to artboard fidelity for the data we have: title, branch chip, author, draft / mergeable / conflicts badges, reviewer avatars with state dots, CI rollup, timing.
- Repo discovery + per-repo Team-tracked opt-in (Settings → Repositories + onboarding step 3).
- Density toggle (Comfortable / Tight / Roomy) — applies to row vertical spacing.
- Settings → Appearance (accent hue picker + light/dark theme toggle + density default).
- Status bar live wiring (synced-N-ago, next-in-M, account/repo counts, API budget %).

### Deferred (do not implement)

- Threads progress bar in the row → M3 (needs per-thread comment-type breakdown).
- Unread dot on the title → M4 (needs read-state tracking).
- "Needs my attention" row highlight and sort-by-attention → M4 (composite signal).
- Quick-filter chips ("Needs my attention", "Unresolved threads", "CI failing", "Stale", "Drafts") → M4.
- Search input + `cmd+K` palette → M4.
- Lines-diff column (`+847 / -203 / 12 files`) → optional cheap add, defer unless trivially included with detail fetch.
- Sort selector UI (Updated / Stale / Needs me) → M4. M2 hard-codes Updated desc.

The row's Vue props include slots for the deferred fields (typed as `optional`), so M3 / M4 wire data into existing component contracts without component rewrites.

## Module layout

```
src-tauri/
  migrations/
    0002_dashboard_fields.sql        # Wave 1 — owned by the contract PR
  src/
    dashboard/                       # Wave 1 creates module shell; Wave 2-C implements
      mod.rs                         # public surface + Tauri command registration
      types.rs                       # DashboardPullRequest DTO + enums (Wave 1 lands stubs)
      query.rs                       # Wave 2-C — SQL composition + execution
      commands.rs                    # Wave 2-C — Tauri command body
    sync/
      discovery.rs                   # Wave 2-A NEW
      worker.rs                      # Wave 2-A modifies run_cycle; Wave 2-B modifies write_pr_updates
    github/
      graphql/
        queries.rs                   # Wave 2-A adds DISCOVERY_QUERY; Wave 2-B extends PR_DETAIL_QUERY
      rest/
        repos.rs                     # Wave 2-D NEW — REST repo listing for discovery
    repos/                           # Wave 2-D NEW — Tauri commands for repo opt-in
      mod.rs
      commands.rs

src/
  views/
    DashboardView.vue                # Wave 3-F rewrites
    settings/
      AppearanceSettings.vue         # Independent slice
      RepositoriesSettings.vue       # Wave 2-D
  components/
    SidebarNav.vue                   # Wave 3-F refactors (views + filter sections)
    StatusBar.vue                    # Independent slice — status-bar wiring
    dashboard/                       # Wave 3-E NEW
      PullRequestRow.vue
      GroupHeader.vue
      ReviewerStack.vue
      CiBadge.vue
      MergeableBadge.vue
      DensityToggle.vue
      GroupSelector.vue
  stores/
    dashboard.ts                     # Wave 3-F NEW
    appearance.ts                    # Independent slice
  router/
    index.ts                         # Wave 3-F adds four dashboard routes
```

## Schema additions

The contract PR lands the full migration. Wave-2 agents must not edit this file — additional columns post-M2 go in `0003+`.

```sql
-- src-tauri/migrations/0002_dashboard_fields.sql

-- Per-PR enrichments visible on the dashboard row.
ALTER TABLE pull_requests ADD COLUMN mergeable         TEXT;       -- "MERGEABLE" | "CONFLICTING" | "UNKNOWN"
ALTER TABLE pull_requests ADD COLUMN review_decision   TEXT;       -- "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED"
ALTER TABLE pull_requests ADD COLUMN additions         INTEGER;
ALTER TABLE pull_requests ADD COLUMN deletions         INTEGER;
ALTER TABLE pull_requests ADD COLUMN changed_files     INTEGER;

-- CI rollup. Pre-aggregated by sync rather than re-counted at query time.
ALTER TABLE pull_requests ADD COLUMN ci_state          TEXT;       -- "SUCCESS" | "FAILURE" | "PENDING" | "ERROR" | "EXPECTED" | null
ALTER TABLE pull_requests ADD COLUMN ci_total          INTEGER;
ALTER TABLE pull_requests ADD COLUMN ci_passing        INTEGER;

-- Per-repo Team-view opt-in. Driven by Settings -> Repositories.
ALTER TABLE repos ADD COLUMN is_team_tracked INTEGER NOT NULL DEFAULT 0;

-- Reviewers requested but not yet submitted. Distinct from `reviews` (submitted reviews).
CREATE TABLE requested_reviewers (
    id                  INTEGER PRIMARY KEY,
    pull_request_id     INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    login               TEXT    NOT NULL,        -- user login or team slug
    reviewer_type       TEXT    NOT NULL,        -- "user" | "team"
    UNIQUE (pull_request_id, reviewer_type, login)
);

CREATE INDEX idx_requested_reviewers_pr
    ON requested_reviewers (pull_request_id);

-- Viewer relations. Rebuilt each sync cycle from Search-API results.
-- One row per (account, PR) where the account has any relationship to the PR.
CREATE TABLE pull_request_viewer_relations (
    account_id              INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    pull_request_id         INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    is_authored             INTEGER NOT NULL DEFAULT 0,
    is_review_requested     INTEGER NOT NULL DEFAULT 0,
    is_involved             INTEGER NOT NULL DEFAULT 0,
    last_seen_at            INTEGER NOT NULL,
    PRIMARY KEY (account_id, pull_request_id)
);

CREATE INDEX idx_pull_request_viewer_relations_account_authored
    ON pull_request_viewer_relations (account_id, is_authored, pull_request_id) WHERE is_authored = 1;

CREATE INDEX idx_pull_request_viewer_relations_account_review_requested
    ON pull_request_viewer_relations (account_id, is_review_requested, pull_request_id) WHERE is_review_requested = 1;

CREATE INDEX idx_pull_request_viewer_relations_account_involved
    ON pull_request_viewer_relations (account_id, is_involved, pull_request_id) WHERE is_involved = 1;
```

### Rationale for the relations design

- **Why a table, not booleans on `pull_requests`.** A single PR can be `authored` from account A's perspective and `review_requested` from account B (different humans, same PR). Boolean columns can't represent that without per-account fan-out.
- **Why `last_seen_at`.** When the next sync cycle's Search-API results don't include a PR the viewer previously interacted with (e.g. they were unassigned), we drop the row. `last_seen_at` is set to the cycle start time on every confirmed relation; rows older than the cycle get pruned.
- **Why three flags, not a single enum.** A PR can be authored and involved simultaneously (you authored it and commented later). One row per (account, PR) is cleaner than three.
- **Why partial indexes.** Each view query selects `WHERE account_id = ? AND is_<flag> = 1`. Partial indexes keep the read narrow without bloating storage.
- **Team view does not use this table.** Team PRs are joined via `repos.is_team_tracked = 1`. The team relationship is a property of the repo, not the (account, PR) pair.

## GraphQL queries

### New: `DISCOVERY_QUERY`

Added to `src-tauri/src/github/graphql/queries.rs`. One query string, called three times per account per cycle with different `q` values.

```rust
pub const DISCOVERY_QUERY: &str = r#"
query DiscoverPrs($q: String!, $after: String) {
  search(type: ISSUE, query: $q, first: 50, after: $after) {
    pageInfo { hasNextPage endCursor }
    nodes {
      __typename
      ... on PullRequest {
        id
        databaseId
        number
        title
        url
        state
        isDraft
        createdAt
        updatedAt
        author { login }
        baseRefName
        headRefName
        repository {
          databaseId
          owner { login }
          name
          isPrivate
        }
      }
    }
  }
}
"#;
```

**Query strings** (one per relation flag):

- Authored: `is:pr is:open author:@me sort:updated`
- Review-requested: `is:pr is:open review-requested:@me sort:updated`
- Involves: `is:pr is:open involves:@me sort:updated`

The `@me` token resolves to the authenticated viewer on the server side; we never have to send the login. Search returns a max of 1000 results per query; pagination via `endCursor`. Practical caps in v1: 200 results per query (4 pages), then truncate with a warning logged.

### Extension: `PR_DETAIL_QUERY` additions

Wave 2-B adds the following fields to the existing `pullRequest` selection in `PR_DETAIL_QUERY`. Layered into the same query (no second round-trip).

```graphql
additions
deletions
changedFiles

reviewRequests(first: 20) {
  nodes {
    requestedReviewer {
      __typename
      ... on User { login }
      ... on Team { slug }
    }
  }
}

commits(last: 1) {
  nodes {
    commit {
      statusCheckRollup {
        state
        contexts(first: 100) {
          totalCount
          nodes {
            __typename
            ... on CheckRun     { conclusion status }
            ... on StatusContext { state }
          }
        }
      }
    }
  }
}
```

Wave 2-B also extends `PullRequestDetail` in `queries.rs` to deserialise the new fields, computes the CI rollup tally (`passing` = count where `conclusion == "SUCCESS"` or `state == "SUCCESS"`), and persists everything in `write_pr_updates`.

## Sync cycle changes

The current cycle (per account) does:

1. Read `repos` rows for the account.
2. For each repo, read seeded `pull_requests` rows.
3. For each PR, fetch detail + timeline, persist.

Post-M2 (per account):

1. **Discovery phase.** Three GraphQL `DISCOVERY_QUERY` calls (`authored`, `review-requested`, `involves`). For each PR returned:
   - Upsert `repos` row (auto-discovers repos the user touches).
   - Upsert `pull_requests` row with the minimal data from the search response.
   - Upsert `pull_request_viewer_relations` row with the matching flag set to `1` and `last_seen_at = cycle_start`.
   - Newly-discovered PRs join the per-PR enrichment list for this cycle.
2. **Team phase.** For each repo with `is_team_tracked = 1`, one paginated REST `GET /repos/{owner}/{name}/pulls?state=open` to enumerate that repo's open PRs. Upsert `pull_requests` rows. No relation row written (Team view joins on `repos.is_team_tracked`).
3. **Enrichment phase** (existing behaviour, extended). For each PR in the union of (a) freshly discovered, (b) seen this cycle in relations, (c) belonging to a Team-tracked repo: fetch the extended `PR_DETAIL_QUERY` and timeline. Persist.
4. **Pruning phase.** Delete `pull_request_viewer_relations` rows where `last_seen_at < cycle_start` (the viewer no longer has that relationship). Closed PRs older than the retention window (14 days) get archived from `pull_requests` — out of scope for M2; tracked separately.

### Rate budget impact

- 3 search calls per account per cycle (Authored / Assigned / Watching).
- N REST list calls per Team-tracked repo (typically 1 per repo, since most repos have < 30 open PRs).
- Detail fetches scale with discovered PRs. Practical cap per cycle: hard limit at 100 PRs per account per cycle to keep budget under 20% of 5000/hr (PRD §8.2). PRs beyond the cap defer to the next cycle, FIFO by `updated_at`.

The existing rate-budget guard in `sync::scheduler` (`RATE_BUDGET_GUARD_PCT`) stays unchanged. If the budget trips mid-cycle, the cycle exits at the next phase boundary.

## Tauri command surface

```rust
// src-tauri/src/dashboard/types.rs

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DashboardView {
    Authored,
    Assigned,
    Watching,
    Team,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DashboardSort {
    Updated,  // M2 default — descending by `latest_status_change_at COALESCE updated_at`
    // M4: NeedsAttention, Stale, Comments
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DashboardPullRequest {
    pub id: i64,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,                    // "open" | "closed" | "merged"
    pub is_draft: bool,
    pub mergeable: Option<String>,        // "MERGEABLE" | "CONFLICTING" | "UNKNOWN"
    pub review_decision: Option<String>,  // "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED"
    pub author_login: String,
    pub base_ref: String,
    pub head_ref: String,
    pub created_at: i64,                  // unix seconds
    pub updated_at: i64,
    pub latest_status_change_at: Option<i64>,
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub changed_files: Option<i64>,
    pub ci: Option<CiSummary>,
    pub reviewers: Vec<ReviewerEntry>,
    pub repo: RepoRef,
    pub account_id: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CiSummary {
    pub state: String,    // "SUCCESS" | "FAILURE" | "PENDING" | "ERROR" | "EXPECTED"
    pub total: i64,
    pub passing: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReviewerEntry {
    pub login: String,
    pub state: ReviewerState,
    pub is_you: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewerState {
    Approved,
    ChangesRequested,
    Commented,
    Pending,   // requested-but-not-submitted
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RepoRef {
    pub id: i64,
    pub owner: String,
    pub name: String,
}
```

```rust
// src-tauri/src/dashboard/commands.rs

#[tauri::command]
pub async fn list_dashboard_pull_requests(
    view: DashboardView,
    sort: DashboardSort,
    account_id: Option<i64>,        // None = all accounts
    db: State<'_, DbHandle>,
) -> Result<Vec<DashboardPullRequest>, String>;
```

**Grouping is a frontend concern.** The command returns a flat sorted vector. The frontend Pinia store buckets by repo / org / none for display. This keeps the SQL composition trivial and lets the user re-group without a round-trip.

**The viewer reviewer entry (`is_you = true`) is computed at query time** by joining `accounts.login` against `reviews.reviewer_login` and `requested_reviewers.login`. The frontend uses `is_you` to render the "you" highlight ring in `ReviewerStack`.

## Frontend component interfaces

### `PullRequestRow.vue`

```ts
defineProps<{
  pullRequest: DashboardPullRequest;
  density?: 'comfortable' | 'tight' | 'roomy';   // default: comfortable
  // M4 slots — undefined in M2
  unread?: boolean;
  needsAttention?: boolean;
}>();

defineEmits<{
  open: [pullRequest: DashboardPullRequest];               // row click -> open on github.com
}>();
```

### `ReviewerStack.vue`

```ts
defineProps<{
  reviewers: ReviewerEntry[];
  max?: number;        // overflow into "+N" pill; default 4
}>();
```

### `CiBadge.vue`

```ts
defineProps<{
  ci: CiSummary | null;
}>();
```

`null` renders an em-dash placeholder (used for draft PRs without checks).

### `MergeableBadge.vue`

```ts
defineProps<{
  state: string | null;        // mergeable
  reviewDecision: string | null;
  isDraft: boolean;
}>();
```

Resolves to one of: `DRAFT`, `CONFLICTS`, `MERGEABLE`, or no badge. Priority order: `isDraft` > `state === 'CONFLICTING'` > `state === 'MERGEABLE' && reviewDecision === 'APPROVED'` > nothing.

### `GroupHeader.vue`

```ts
defineProps<{
  label: string;             // e.g. "sitemate / web"
  org: string | null;        // for nested-style rendering
  count: number;
  needYou?: number;          // M4 — undefined in M2
  failing?: number;          // count of `ci.state === 'FAILURE'`
  latestUpdatedAt: number;   // unix sec
  collapsible?: boolean;     // default true
}>();
```

### `DensityToggle.vue`

```ts
defineProps<{
  modelValue: 'comfortable' | 'tight' | 'roomy';
}>();

defineEmits<{
  'update:modelValue': [value: 'comfortable' | 'tight' | 'roomy'];
}>();
```

### `GroupSelector.vue`

```ts
defineProps<{
  modelValue: 'repo' | 'org' | 'none';
}>();

defineEmits<{
  'update:modelValue': [value: 'repo' | 'org' | 'none'];
}>();
```

### Pinia store (Wave 3-F)

```ts
// src/stores/dashboard.ts

export const useDashboardStore = defineStore('dashboard', () => {
  const view = ref<DashboardView>('authored');
  const group = ref<'repo' | 'org' | 'none'>('repo');
  const sort = ref<DashboardSort>('updated');
  const density = ref<'comfortable' | 'tight' | 'roomy'>('comfortable');
  const accountFilter = ref<number | null>(null);

  const pullRequests = ref<DashboardPullRequest[]>([]);
  const loading = ref(false);

  // Grouped on demand for the current view.
  const groups = computed<{ key: string; label: string; items: DashboardPullRequest[] }[]>(() => ...);

  // Sidebar counts — derived from `pullRequests` after each load.
  const counts = computed(() => ({
    authored: ...,
    assigned: ...,
    watching: ...,
    team: ...,
  }));

  async function load() { /* invoke('list_dashboard_pull_requests', ...) */ }

  return { view, group, sort, density, accountFilter, pullRequests, loading, groups, counts, load };
});
```

The store subscribes to the existing `sync://status` event and refreshes `pullRequests` after each completed cycle.

## File ownership map

### Wave 1 (contract PR) — owns everything in this section

- `docs/contracts/dashboard-data.md` (this file)
- `docs/adr/0009-pull-request-discovery-via-search-api.md`
- `src-tauri/migrations/0002_dashboard_fields.sql` (the full migration above)
- `src-tauri/src/dashboard/mod.rs` (module shell + re-exports)
- `src-tauri/src/dashboard/types.rs` (DTO enums + structs from the contract)
- `src-tauri/src/dashboard/commands.rs` (Tauri command with `unimplemented!()` body so the type checks)
- `src-tauri/src/dashboard/query.rs` (empty module — Wave 2-C fills)
- `src-tauri/src/lib.rs` (mount `dashboard` module + register the command)

### Wave 2 (parallel)

| Issue | Owns                                                                                                                                                 | Touches but doesn't own                                            | Don't touch                                                                            |
| ----- | ---------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ | -------------------------------------------------------------------------------------- |
| **A** Discovery + relations | `src/sync/discovery.rs` (new), `src/github/graphql/queries.rs` (adds `DISCOVERY_QUERY` const + types), `src/sync/worker.rs::run_cycle` (calls discovery before existing per-PR enrichment loop) | `Cargo.toml` (graphql-search may need no new deps; if so leave it) | `dashboard/`, `repos/`, `write_pr_updates`, `PR_DETAIL_QUERY` body                     |
| **B** PR detail enrichments | `src/sync/worker.rs::write_pr_updates` (extends INSERT/UPDATE with new columns), `src/github/graphql/queries.rs::PR_DETAIL_QUERY` (extends query string + `PullRequestDetail` struct + new sibling structs), `requested_reviewers` upsert helper | `Cargo.toml`                                                       | `dashboard/`, `repos/`, `discovery.rs`, `run_cycle`, `DISCOVERY_QUERY`                 |
| **C** Dashboard query       | `src/dashboard/query.rs`, `src/dashboard/commands.rs::list_dashboard_pull_requests` body, integration tests for each view's SQL                              | `Cargo.toml`                                                       | `sync/`, `github/`, `repos/`                                                           |
| **D** Repo discovery        | `src/repos/` (new module), `src/github/rest/repos.rs` (new — REST `/user/repos` + `/orgs/{org}/repos`), `src/views/settings/RepositoriesSettings.vue`, `src/stores/repos.ts`, `src/views/OnboardingView.vue` (extends step 3 if not present) | `Cargo.toml`, `lib.rs` (mount `repos` module)                      | `dashboard/`, `sync/`, `pull_requests` writes                                          |

**Merge order: A → B → C → D.** A owns the canonical discovery types (the search-API response deserialiser), and B's enrichment writes depend on the PR row existing — A creates those rows first. C only needs the schema. D is independent of A/B/C but touches `lib.rs` so cleanest at the end of the cascade.

### Wave 3 (parallel, after Wave 2 lands)

| Issue                            | Owns                                                                                                                                                                                                                                                | Touches but doesn't own       | Don't touch              |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------- | ------------------------ |
| **E** Row + atoms                | `src/components/dashboard/PullRequestRow.vue`, `ReviewerStack.vue`, `CiBadge.vue`, `MergeableBadge.vue`, `GroupHeader.vue`, `DensityToggle.vue`, `GroupSelector.vue`. Adds any new CSS primitives to `src/assets/styles/primitives.css` (e.g. `.row-strip-*`) | `package.json` (rare)         | `views/`, `stores/`      |
| **F** Shell + store + sidebar    | `src/views/DashboardView.vue` (rewrite), `src/stores/dashboard.ts`, `src/router/index.ts` (four dashboard routes), `src/components/SidebarNav.vue` (enable the three currently-disabled view entries, wire live counts, drop the `--disabled` modifier; Filters and Accounts sections from the artboard are M4 / later — do not add them) | `package.json`                | `components/dashboard/`  |

E lands first; F rebases on top and imports E's components. Alternatively F stubs the component imports against the typed interface and rebases when E merges — the agent's call.

### Independent slices (any time after Wave 1)

| Issue                       | Owns                                                                                                                       | Notes                                                                                                       |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------- |
| **G** Settings → Appearance | `src/views/settings/AppearanceSettings.vue`, `src/stores/appearance.ts`, hue / theme / density-default wiring to `:root`   | No dependency on dashboard data. Can land any time after Wave 1 (needs the `density` prop type on `PullRequestRow`). |
| **H** Status bar wiring     | `src/components/StatusBar.vue` (extends — already exists), reads existing `sync://*` events for counts + budget %          | No dependency on dashboard data. Touches no new types.                                                      |

## Out of scope (deferred)

| Surface                       | Lands in | Why                                                                                                |
| ----------------------------- | -------- | -------------------------------------------------------------------------------------------------- |
| Threads progress bar          | M3       | Needs per-thread comment-type breakdown (review vs. issue vs. mentioned-in).                       |
| Unread dot on row title       | M4       | Requires read-state tracking — no schema for it yet.                                               |
| "Needs my attention" highlight | M4      | Composite signal — needs threads data and a heuristic; both are post-M2.                           |
| Sort selector UI              | M4       | Multiple sort modes need the chips above the list, which arrive together with filter chips.        |
| Filter chips                  | M4       | Spec'd alongside the "Needs my attention" composite.                                               |
| Search input / cmd+K          | M4       | Requires the search-API surface used differently (free-text); deferred to keep M2 focused.         |
| Lines diff in row             | optional | Cheap if it sneaks in with detail fetch; otherwise defer. Not required for M2 acceptance.          |
| Archive bucket + TTL          | M6       | Retention policy + UI for archived PRs.                                                            |

## Implementation notes that aren't part of the interface

These belong here so Wave-2 agents don't reinvent them, but they don't constrain the public types above.

- **Search-API pagination cursor lives in memory, not the DB.** Discovery loops `endCursor` until exhausted or the cap is hit. There's no resumption between cycles.
- **`@me` resolves on GitHub's side.** Discovery doesn't need the viewer's login. (The viewer's login still appears on `accounts.login` for the `is_you` reviewer-entry computation.)
- **Search returns `databaseId` (int) and `id` (GraphQL global ID).** Use `databaseId` as the local `pull_requests.id` — it's stable and an `INTEGER`, which the existing schema expects.
- **`is_team_tracked` does not retro-fetch closed PRs.** Team-view PRs are discovered going forward only; historical data is out of scope.
- **CI rollup `passing` count.** `CheckRun` is "passing" when `conclusion == "SUCCESS"`. `StatusContext` is "passing" when `state == "SUCCESS"`. `null` conclusion = in progress (not counted in passing, counted in total).
- **Frontend opens PR on GitHub on row click.** No internal expanded view in M2 (the dashboard-expanded artboard is M3+). Use `openUrl(pullRequest.url)` from the already-installed `@tauri-apps/plugin-opener` — no new dependency.

## ADR cross-references

- ADR [0004](../adr/0004-sync-polling-with-etag.md) — polling cadence and rate-budget envelope still apply; this contract layers within it.
- ADR [0006](../adr/0006-graphql-first-rest-fallback.md) — Search API for discovery uses GraphQL by default; Team-list per repo uses REST because GraphQL's `repository.pullRequests` lacks the cheap pagination shape REST `Link rel="next"` provides.
- ADR 0009 (to be authored alongside this contract) — records the Search-API decision for discovery, the relations-table choice, and the rate-budget arithmetic.
