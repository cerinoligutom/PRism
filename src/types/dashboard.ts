/**
 * Frontend mirrors of the Rust DTO types in `src-tauri/src/dashboard/types.rs`.
 *
 * The serde rename in Rust emits `kebab-case` for the enums (`ReviewerState`,
 * `DashboardView`, `DashboardSort`), so the string unions below match the wire
 * shape exactly. Field names are `snake_case` because Rust serde emits them
 * verbatim from struct fields.
 *
 * Keep this file in lock-step with `dashboard/types.rs` and the
 * "Tauri command surface" section of `docs/contracts/dashboard-data.md`.
 */

export type DashboardView =
  | "authored"
  | "assigned"
  | "watching"
  | "tracked"
  | "archive";

/**
 * Mirrors `DashboardSort` in `src-tauri/src/dashboard/types.rs`. M2 shipped
 * `"updated"` only; M4 (`docs/contracts/triage-ux.md`, ADR 0015) adds
 * `"stale"` (oldest activity first) and `"needs-me"` (attention-first
 * within the active view).
 */
export type DashboardSort = "updated" | "stale" | "needs-me";

export type ReviewerState =
  | "approved"
  | "changes-requested"
  | "commented"
  | "pending";

/**
 * Mirrors `MyReviewState` in `src-tauri/src/dashboard/types.rs`. The viewer's
 * own relationship to the PR for the my-review-state row slot, derived
 * server-side because the client cannot express `"author"` / `"requested"` /
 * `"none"` from the submitted-review projection alone (ADR 0031). Precedence
 * (highest wins): author > requested > changes-requested > approved >
 * commented > none.
 */
export type MyReviewState =
  | "author"
  | "requested"
  | "changes-requested"
  | "approved"
  | "commented"
  | "none";

export type RowDensity = "comfortable" | "tight" | "roomy";

export type DashboardGroup = "repo" | "org" | "none";

/**
 * Mirrors the kebab-case Rust enum tag used by the chip filter pipeline (see
 * `docs/contracts/triage-ux.md` "Filter chip semantics"). Active chips compose
 * as AND across keys; per-chip counts are independent (each shows what would
 * match if that chip alone were toggled).
 */
export type ChipKey =
  | "needs-attention"
  | "unresolved-threads"
  | "ci-failing"
  | "stale"
  | "drafts";

/**
 * Mirrors `FilterChipCounts` in `src-tauri/src/triage/types.rs`. Per-chip
 * counts are returned by `list_filter_chip_counts` once per (view, account)
 * change; the frontend renders them inline on each chip.
 */
export interface FilterChipCounts {
  readonly needs_attention: number;
  readonly unresolved_threads: number;
  readonly ci_failing: number;
  readonly stale: number;
  readonly drafts: number;
}

export interface CiSummary {
  /** `"SUCCESS" | "FAILURE" | "PENDING" | "ERROR" | "EXPECTED"`. */
  readonly state: string;
  readonly total: number;
  readonly passing: number;
}

/**
 * Per-PR review-thread rollup pre-aggregated by the sync cycle. `null` on the
 * parent DTO when the PR has never had a thread; the frontend renders the
 * muted em-dash state in that case. See `docs/contracts/conversation-depth.md`
 * "Dashboard rollup", ADR 0010, and ADR 0012 (four-bucket redesign).
 *
 * The four bucket fields are disjoint over the full thread set (including
 * outdated). `total` equals the sum of the four. Outdated threads sort into
 * whichever bucket matches their (resolved x involved) state.
 */
export interface ThreadsSummary {
  readonly total: number;
  readonly unresolved_involved: number;
  readonly unresolved_uninvolved: number;
  readonly resolved_involved: number;
  readonly resolved_uninvolved: number;
}

export interface ReviewerEntry {
  readonly login: string;
  readonly state: ReviewerState;
  readonly is_you: boolean;
  /** GitHub avatar URL for `login`; see ADR 0013. */
  readonly avatar_url: string | null;
}

export interface RepoRef {
  readonly id: number;
  readonly owner: string;
  readonly name: string;
}

/**
 * Render-ready projection of a tracked account for the dashboard row marker.
 * The view derives this from the accounts store and feeds a shared lookup
 * to every row so `pullRequest.account_ids` resolves without per-row store
 * coupling. See ADR 0016 ("Dashboard row shape - option 1").
 */
export interface AccountMarker {
  readonly id: number;
  readonly label: string;
  readonly login: string;
  readonly avatar_url: string | null;
}

export interface DashboardPullRequest {
  readonly id: number;
  readonly number: number;
  readonly title: string;
  readonly url: string;
  /** `"open" | "closed" | "merged"`. */
  readonly state: string;
  readonly is_draft: boolean;
  /** `"MERGEABLE" | "CONFLICTING" | "UNKNOWN"`. */
  readonly mergeable: string | null;
  /** `"APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED"`. */
  readonly review_decision: string | null;
  readonly author_login: string;
  /** GitHub avatar URL for `author_login`; see ADR 0013. `null` when the
   * sync cycle hasn't seen this login yet. */
  readonly author_avatar_url: string | null;
  readonly base_ref: string;
  readonly head_ref: string;
  /** Unix seconds. */
  readonly created_at: number;
  /** Unix seconds. */
  readonly updated_at: number;
  /** Unix seconds. */
  readonly latest_status_change_at: number | null;
  readonly additions: number | null;
  readonly deletions: number | null;
  readonly changed_files: number | null;
  readonly ci: CiSummary | null;
  readonly threads: ThreadsSummary | null;
  readonly reviewers: readonly ReviewerEntry[];
  /** The viewer's own relationship to the PR for the my-review-state row slot.
   * Derived server-side with a fixed precedence (author > requested >
   * changes-requested > approved > commented > none); host-gated. See
   * ADR 0031. */
  readonly my_review_state: MyReviewState;
  readonly repo: RepoRef;
  /** Tracked accounts with a relation to this PR. Sorted ascending. Length
   * 1 in the single-account-filter path; 1..N in the unified path; empty
   * for Tracked-view PRs in the unified path that have no relation rows. See
   * ADR 0016 ("Dashboard row shape - option 1"). */
  readonly account_ids: readonly number[];
  /** True when the viewer hasn't opened this PR since its last upstream
   * update. In unified mode `unread` reads as true when any in-scope
   * account is unread (MAX merge). See ADR 0015 and
   * `docs/contracts/triage-ux.md` ("Read-state derivation"). */
  readonly unread: boolean;
  /** Precomputed "needs my attention" composite flag. In unified mode
   * merges via MAX across relation owners. See ADR 0015
   * ("Composite formula"). */
  readonly needs_attention: boolean;
  /** Mentions of the viewer login seen since the last read. Summed across
   * relation owners in unified mode. Reset to zero by `mark_pr_read`.
   * See ADR 0015 ("Mention detection"). */
  readonly mentioned_count_unread: number;
}
