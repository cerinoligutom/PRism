/**
 * Typed demo data for the "How signals work" reference page (#436). Every
 * fixture is declared `as const satisfies` the real DTO, so a field rename or
 * type change in `@/types/dashboard` / `@/types/conversation` breaks the build
 * here rather than silently rotting the guide. The page embeds the live
 * `PullRequestRow` and `ThreadsList` on these, so what the user sees is the
 * real component, not a replica.
 *
 * Timestamps are offsets from a reference instant captured once at module load
 * so the relative-time labels ("3h ago", "2d ago") read as recent activity and
 * the demo row never trips the 7-day stale threshold. Anchoring at load (rather
 * than to a hard-coded date) keeps the sample fresh however long after build
 * the app runs, and is stable for the lifetime of a session.
 */
import type {
  AccountMarker,
  DashboardPullRequest,
} from "@/types/dashboard";
import type {
  PullRequestThread,
  ThreadComment,
} from "@/types/conversation";

/** Reference "now" for the fixtures, in unix seconds, captured once at module
 * load so the demo activity reads as recent rather than stale. */
const NOW = Math.floor(Date.now() / 1000);
const HOUR = 60 * 60;
const DAY = 24 * HOUR;

/** The viewer's login in the demo, so the involved/mention thread reads as
 * "someone replied to you" rather than "you replied to yourself". */
export const DEMO_VIEWER_LOGIN = "you";

/**
 * A believable mid-review PR: you've been asked to review (`requested`), a
 * conversation you're in just moved (`needs_attention`), and there's content
 * you haven't opened (`unread`). CI is partially green, one thread per bucket,
 * and two reviewers so the embedded row exercises every cell.
 */
export const DEMO_PR = {
  id: 9001,
  number: 482,
  title: "Stream sync deltas instead of full snapshots",
  url: "https://github.com/octo-org/prism-demo/pull/482",
  state: "open",
  is_draft: false,
  mergeable: "MERGEABLE",
  review_decision: "REVIEW_REQUIRED",
  author_login: "marsha",
  author_avatar_url: null,
  base_ref: "main",
  head_ref: "sync/stream-deltas",
  created_at: NOW - 4 * DAY,
  updated_at: NOW - 3 * HOUR,
  latest_status_change_at: NOW - 3 * HOUR,
  additions: 412,
  deletions: 96,
  changed_files: 11,
  ci: { state: "PENDING", total: 6, passing: 4 },
  threads: {
    total: 3,
    unresolved_involved: 1,
    unresolved_uninvolved: 1,
    resolved_involved: 1,
    resolved_uninvolved: 0,
  },
  reviewers: [
    { login: "you", state: "pending", is_you: true, avatar_url: null },
    { login: "dmitri", state: "commented", is_you: false, avatar_url: null },
  ],
  my_review_state: "requested",
  repo: { id: 71, owner: "octo-org", name: "prism-demo" },
  account_ids: [1],
  unread: true,
  needs_attention: true,
} as const satisfies DashboardPullRequest;

/**
 * Three threads, one per interesting state the conversation surface encodes:
 *
 *   1. `unresolved-involved` + unread - a reply by someone else landed after
 *      your last comment. This is the "needs you" thread (left-edge accent,
 *      brighter badge).
 *   2. `unresolved-uninvolved` - an open thread you aren't part of.
 *   3. `resolved-involved` - a thread you were in that's since been resolved.
 *
 * Reply counts and the head comment authors are set so the participant stack
 * and snippet render with real names rather than placeholders.
 */
export const DEMO_THREADS = [
  {
    id: 5001,
    node_id: "PRRT_demo_involved",
    pull_request_id: DEMO_PR.id,
    state: "unresolved",
    path: "src/sync/stream.rs",
    line: 142,
    start_line: null,
    original_line: 142,
    reply_count: 3,
    head_comment: {
      author_login: "you",
      avatar_url: null,
      body_text:
        "Should we bound the channel here so a slow consumer can't balloon memory?",
      created_at: NOW - 2 * DAY,
      url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r1",
    },
    created_at: NOW - 2 * DAY,
    resolved_at: null,
    last_reply_at: NOW - 3 * HOUR,
    is_involved: true,
    is_resolved: false,
    is_outdated: false,
    url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r1",
    unread: true,
    diff_hunk: null,
  },
  {
    id: 5002,
    node_id: "PRRT_demo_uninvolved",
    pull_request_id: DEMO_PR.id,
    state: "unresolved",
    path: "src/sync/worker.rs",
    line: 88,
    start_line: null,
    original_line: 88,
    reply_count: 1,
    head_comment: {
      author_login: "dmitri",
      avatar_url: null,
      body_text: "Nit: this log line should be debug, not info.",
      created_at: NOW - DAY,
      url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r2",
    },
    created_at: NOW - DAY,
    resolved_at: null,
    last_reply_at: NOW - DAY,
    is_involved: false,
    is_resolved: false,
    is_outdated: false,
    url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r2",
    unread: false,
    diff_hunk: null,
  },
  {
    id: 5003,
    node_id: "PRRT_demo_resolved",
    pull_request_id: DEMO_PR.id,
    state: "resolved",
    path: "src/sync/mod.rs",
    line: 17,
    start_line: null,
    original_line: 17,
    reply_count: 2,
    head_comment: {
      author_login: "marsha",
      avatar_url: null,
      body_text: "Can you add a doc comment explaining the delta envelope?",
      created_at: NOW - 3 * DAY,
      url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r3",
    },
    created_at: NOW - 3 * DAY,
    resolved_at: NOW - 2 * DAY,
    last_reply_at: NOW - 2 * DAY,
    is_involved: true,
    is_resolved: true,
    is_outdated: false,
    url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r3",
    unread: false,
    diff_hunk: null,
  },
] as const satisfies readonly PullRequestThread[];

/**
 * Comments for the involved thread, ordered so the last word is someone
 * other than you - which is exactly what lights the unit (ADR 0031): your
 * comment, then a reply mentioning you. The general comment is carried on
 * the resolved thread to show a plain back-and-forth.
 */
export const DEMO_THREAD_COMMENTS = [
  {
    id: 6001,
    thread_id: 5001,
    author_login: "you",
    avatar_url: null,
    body: "Should we bound the channel here so a slow consumer can't balloon memory?",
    body_html: null,
    created_at: NOW - 2 * DAY,
    line: 142,
    side: "RIGHT",
    url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r1",
  },
  {
    id: 6002,
    thread_id: 5001,
    author_login: "marsha",
    avatar_url: null,
    body: "Good call - bounded it at 256. @you can you sanity-check the back-pressure path?",
    body_html: null,
    created_at: NOW - 3 * HOUR,
    line: 142,
    side: "RIGHT",
    url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r1b",
  },
  {
    id: 6003,
    thread_id: 5003,
    author_login: "marsha",
    avatar_url: null,
    body: "Can you add a doc comment explaining the delta envelope?",
    body_html: null,
    created_at: NOW - 3 * DAY,
    line: 17,
    side: "RIGHT",
    url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r3",
  },
  {
    id: 6004,
    thread_id: 5003,
    author_login: "you",
    avatar_url: null,
    body: "Done - added the envelope doc.",
    body_html: null,
    created_at: NOW - 2 * DAY,
    line: 17,
    side: "RIGHT",
    url: "https://github.com/octo-org/prism-demo/pull/482#discussion_r3b",
  },
] as const satisfies readonly ThreadComment[];

/**
 * The account-marker lookup `PullRequestRow` reads to resolve
 * `DEMO_PR.account_ids` into avatars. One entry keyed by id `1`. The row is
 * rendered with `single-account-scope` on the page so the marker stays hidden,
 * but the map is provided so the prop contract is satisfied honestly.
 */
export const DEMO_ACCOUNTS_BY_ID: ReadonlyMap<number, AccountMarker> = new Map([
  [
    1,
    {
      id: 1,
      label: "Work",
      login: "you",
      avatar_url: null,
    } satisfies AccountMarker,
  ],
]);
