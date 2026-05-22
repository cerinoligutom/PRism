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
  /** GitHub avatar URL resolved from the local `users` cache. `null` when the
   * sync cycle hasn't seen this login yet — the frontend falls back to the
   * initials avatar. See ADR 0013. */
  readonly avatar_url: string | null;
  readonly body_text: string;
  /** Unix seconds. */
  readonly created_at: number;
  /** GitHub permalink for the head comment. Equals `PullRequestThread.url`
   * (the thread permalink is derived from the head comment per issue
   * #115). `null` for rows written before that column existed. */
  readonly url: string | null;
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
  readonly is_involved: boolean;
  /** Resolved-state flag carried separately from `state` so the frontend can
   * pick the four-state icon palette `(is_resolved, is_involved)` without
   * losing the orthogonal `is_outdated` cue. See issue #102. */
  readonly is_resolved: boolean;
  /** `is_outdated` mirror so the frontend can apply the dim treatment + badge
   * regardless of which bucket the four-state icon resolves to. */
  readonly is_outdated: boolean;
  /** GitHub permalink for the thread; powers the per-thread
   * "Open in GitHub" action. `null` for rows written before the column
   * existed. */
  readonly url: string | null;
  /** True when this thread has activity the active account hasn't seen yet.
   * Derived by `conversation::query::list_pr_threads` against the viewer's
   * `pull_request_viewer_relations.read_at` watermark — see issue #158.
   * Always `false` when no account is in scope (anonymous read). */
  readonly unread: boolean;
  /** Unified-diff hunk GitHub attaches to every inline review comment in the
   * thread. Persisted once per thread by the lazy hydrator; rendered as the
   * file-context block above each thread card. `null` until the hydrator has
   * run for the PR (legacy rows + PRs whose drawer has never been opened).
   * See issue #162. */
  readonly diff_hunk: string | null;
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
  /** Four-bucket breakdown matching the dashboard `ThreadsSummary`. Sourced
   * from `pull_requests.threads_*_*` so the conversation surface's bar
   * renders identically to the dashboard row's (issue #102, ADR 0012). */
  readonly threads_unresolved_involved: number;
  readonly threads_unresolved_uninvolved: number;
  readonly threads_resolved_involved: number;
  readonly threads_resolved_uninvolved: number;
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
  /** GitHub avatar URL for `author_login`; see ADR 0013. */
  readonly avatar_url: string | null;
  /** `APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED | PENDING`. */
  readonly state: string;
  readonly body: string | null;
  /** Pre-rendered HTML from GitHub for the review summary body. Rendered via
   * `PRismMarkdown`; falls back to plain `body` when `null` (legacy rows).
   * See ADR 0014 and issue #138. */
  readonly body_html: string | null;
  /** Unix seconds. */
  readonly submitted_at: number | null;
  /** GitHub permalink for the review (`PullRequestReview.url`). Used by the
   *  Reviews tab's per-review "Open in GitHub" action. `null` for rows
   *  written before migration 0011. */
  readonly url: string | null;
}

export interface ThreadComment {
  readonly id: number;
  readonly thread_id: number;
  readonly author_login: string;
  /** GitHub avatar URL for `author_login`; see ADR 0013. */
  readonly avatar_url: string | null;
  readonly body: string;
  /** Pre-rendered HTML from GitHub for the comment body. Rendered via
   * `PRismMarkdown`; falls back to plain `body` when `null` (legacy rows).
   * See ADR 0014 and issue #138. */
  readonly body_html: string | null;
  /** Unix seconds. */
  readonly created_at: number;
  readonly line: number | null;
  /** `LEFT | RIGHT`. */
  readonly side: string | null;
  /** GitHub permalink for the comment. Powers the per-comment "Open in
   * GitHub" icon button on the expanded thread view. `null` for rows
   * written before issue #115. */
  readonly url: string | null;
}

export interface IssueComment {
  readonly id: number;
  readonly author_login: string;
  /** GitHub avatar URL for `author_login`; see ADR 0013. */
  readonly avatar_url: string | null;
  readonly body: string;
  /** Pre-rendered HTML from GitHub for the issue-comment body. Captured for
   * parity; not yet rendered on the conversation surface. See ADR 0014. */
  readonly body_html: string | null;
  /** Unix seconds. */
  readonly created_at: number;
  /** GitHub permalink for the issue comment. Captured for parity; not yet
   * rendered on the conversation surface (M3 contract). See issue #115. */
  readonly url: string | null;
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
 * One persisted row from `timeline_events`. Mirrors the Rust
 * `TimelineEventRecord` DTO. The `event_type` is the GitHub wire name per
 * ADR 0007 (`ready_for_review`, `convert_to_draft`, `review_requested`,
 * `reviewed`, `merged`, `closed`, `reopened`). `review_state` is populated
 * only on `reviewed` events (`APPROVED` / `CHANGES_REQUESTED` / `COMMENTED`
 * / `DISMISSED`).
 */
export interface TimelineEventRecord {
  readonly event_type: string;
  readonly actor_login: string | null;
  /** GitHub avatar URL for `actor_login`; see ADR 0013. */
  readonly actor_avatar_url: string | null;
  /** Unix seconds. */
  readonly created_at: number;
  readonly review_state: string | null;
}

/**
 * Detail-surface selector value. Inline expansion was considered as a
 * third surface and cancelled before launch — see ADR 0011.
 */
export type PrDetailSurface = "drawer" | "route";
