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

export type DashboardView = "authored" | "assigned" | "watching" | "team";

export type DashboardSort = "updated";

export type ReviewerState =
  | "approved"
  | "changes-requested"
  | "commented"
  | "pending";

export type RowDensity = "comfortable" | "tight" | "roomy";

export type DashboardGroup = "repo" | "org" | "none";

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
}

export interface RepoRef {
  readonly id: number;
  readonly owner: string;
  readonly name: string;
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
  readonly repo: RepoRef;
  readonly account_id: number;
}
