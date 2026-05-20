/**
 * Frontend mirrors of the Rust DTO types in
 * `src-tauri/src/conversation/types.rs`.
 *
 * The serde rename in Rust emits `kebab-case` for the enum (`ThreadState`),
 * so the string union below matches the wire shape exactly. Field names are
 * `snake_case` because Rust serde emits them verbatim from struct fields.
 *
 * Keep this file in lock-step with `conversation/types.rs` and the
 * "Tauri command surface" section of `docs/contracts/conversation-depth.md`.
 */

export type ThreadState = "unresolved" | "resolved" | "outdated";

export interface ThreadHeadComment {
  readonly author_login: string;
  readonly body_text: string;
  /** Unix seconds. */
  readonly created_at: number;
}

export interface PullRequestThread {
  readonly id: number;
  readonly node_id: string;
  readonly pull_request_id: number;
  readonly state: ThreadState;
  readonly path: string | null;
  readonly line: number | null;
  readonly start_line: number | null;
  readonly original_line: number | null;
  readonly reply_count: number;
  readonly head_comment: ThreadHeadComment | null;
  /** Unix seconds. */
  readonly created_at: number | null;
  /** Unix seconds. */
  readonly resolved_at: number | null;
  /** Unix seconds. */
  readonly last_reply_at: number | null;
  /** True when the active account's login appears anywhere in the thread. */
  readonly is_you_in: boolean;
}

export interface CommentBreakdown {
  readonly review: number;
  readonly issue: number;
  readonly summary: number;
  readonly total: number;
}

export interface ConversationStats {
  readonly threads_total: number;
  readonly threads_unresolved: number;
  readonly threads_resolved: number;
  readonly threads_outdated: number;
  /** Unix seconds; null when no active threads. */
  readonly oldest_unresolved_at: number | null;
  /** Average seconds between replies; null when no thread has a reply. */
  readonly avg_response_seconds: number | null;
  /** `[0.0, 1.0]`. */
  readonly resolution_rate: number;
  readonly comment_breakdown: CommentBreakdown;
}

export interface PullRequestReview {
  readonly id: number;
  readonly node_id: string;
  readonly author_login: string;
  /** `APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED | PENDING`. */
  readonly state: string;
  readonly body: string | null;
  /** Unix seconds. */
  readonly submitted_at: number | null;
}

export interface ThreadComment {
  readonly id: number;
  readonly thread_id: number;
  readonly author_login: string;
  readonly body: string;
  /** Unix seconds. */
  readonly created_at: number;
  readonly line: number | null;
  /** `LEFT | RIGHT`. */
  readonly side: string | null;
}

export interface IssueComment {
  readonly id: number;
  readonly author_login: string;
  readonly body: string;
  /** Unix seconds. */
  readonly created_at: number;
}

export interface HydratedConversation {
  readonly pull_request_id: number;
  readonly threads: readonly PullRequestThread[];
  readonly thread_comments: readonly ThreadComment[];
  readonly issue_comments: readonly IssueComment[];
  readonly reviews: readonly PullRequestReview[];
  readonly stats: ConversationStats;
}

/**
 * Detail-surface selector value. The `inline` variant is reserved for the
 * post-M3 follow-up host; the settings selector renders it as disabled.
 */
export type PrDetailSurface = "drawer" | "route" | "inline";
