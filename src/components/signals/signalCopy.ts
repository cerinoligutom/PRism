import type { MyReviewState } from "@/types/dashboard";

/**
 * Single source of truth for the labels and one-line explanations behind the
 * PR-row left-edge signals: the attention dot (the `needs_attention` roll-up)
 * and each of the six `my_review_state` values (ADR 0031, "Left-edge
 * encoding"). The row reads these for its tooltips; a later slice (#436)
 * extends the same map for the "How signals work" guide, so the row and the
 * guide cannot drift.
 *
 * Each entry pairs a terse `label` (the tooltip header / glyph name) with a
 * one-sentence `description` (what the signal means and what clears it).
 */
export interface SignalCopy {
  readonly label: string;
  readonly description: string;
}

/**
 * Which branch of the `needs_attention` roll-up lit the dot, derived on the row
 * from `my_review_state` / `review_decision`. `conversation` covers the thread,
 * general-stream, and reviews-mention branches (they clear by reply / resolve /
 * mark-seen); the two obligation reasons clear on PR open (ADR 0033).
 */
export type AttentionReason =
  | "conversation"
  | "review_request"
  | "changes_requested";

/**
 * Attention-dot copy. `label` / `description` are the one-line headline the
 * `/signals` guide renders; `byReason` gives the live row tooltip a description
 * matched to the branch that actually lit the dot, so the row reads true while
 * the guide stays a summary.
 */
export interface AttentionCopy extends SignalCopy {
  readonly byReason: Readonly<Record<AttentionReason, string>>;
}

/** Keyed by `my_review_state`. The `none` entry covers "not a reviewer". */
type MyReviewCopy = Readonly<Record<MyReviewState, SignalCopy>>;

/** The four review-thread buckets (resolved x involved) from ADR 0012's
 * palette, keyed identically to `ThreadsList`'s `ThreadBucket` so the guide's
 * legend and the live thread cards read from one source. */
type ThreadBucketKey =
  | "unresolved-involved"
  | "unresolved-uninvolved"
  | "resolved-involved"
  | "resolved-uninvolved";

type ThreadBucketCopy = Readonly<Record<ThreadBucketKey, SignalCopy>>;

/** The three count surfaces (ADR 0031, "The roll-up") the guide shows agreeing. */
type CountKey = "badge" | "sidebar" | "inbox";

type CountCopy = Readonly<Record<CountKey, SignalCopy>>;

/** The plain-English involvement model (ADR 0031): what lights a conversation
 * unit, what clears it, and the PR-level role obligations that sit beside it. */
type ModelKey =
  | "involvement"
  | "lights"
  | "clears"
  | "resolved"
  | "obligations";

type ModelCopy = Readonly<Record<ModelKey, SignalCopy>>;

export const SIGNAL_COPY: {
  readonly attention: AttentionCopy;
  readonly myReview: MyReviewCopy;
  readonly threadBucket: ThreadBucketCopy;
  readonly count: CountCopy;
  readonly model: ModelCopy;
} = {
  attention: {
    label: "Needs you",
    description:
      "A conversation you're in moved, or you owe a review. A conversation clears when you reply or mark it seen; a review clears when you open the PR.",
    byReason: {
      conversation:
        "A conversation you're in moved. Clears when you reply, resolve it, or mark it seen.",
      review_request: "You've been asked to review. Clears when you open the PR.",
      changes_requested:
        "Changes were requested on your PR. Clears when you open it.",
    },
  },
  myReview: {
    author: {
      label: "You authored this",
      description: "You opened this pull request.",
    },
    requested: {
      label: "Review requested",
      description:
        "You've been asked to review. A re-request shows here even after a prior review.",
    },
    "changes-requested": {
      label: "You requested changes",
      description: "You reviewed this and asked for changes.",
    },
    approved: {
      label: "You approved",
      description: "You've approved this pull request.",
    },
    commented: {
      label: "You commented",
      description: "You've commented without a formal review verdict.",
    },
    none: {
      label: "Not a reviewer",
      description: "You aren't a reviewer on this pull request.",
    },
  },
  threadBucket: {
    "unresolved-involved": {
      label: "Unresolved · you're in it",
      description:
        "An open thread you're part of. Warm colour because it can need you.",
    },
    "unresolved-uninvolved": {
      label: "Unresolved",
      description: "An open thread you aren't part of yet.",
    },
    "resolved-involved": {
      label: "Resolved · was yours",
      description:
        "A thread you were in that's since been resolved. A new reply still nags.",
    },
    "resolved-uninvolved": {
      label: "Resolved",
      description: "A settled thread you weren't part of.",
    },
  },
  count: {
    badge: {
      label: "Dock badge",
      description:
        "PRs with a conversation that needs you, or a review obligation you haven't opened yet.",
    },
    sidebar: {
      label: "Sidebar chip",
      description: "Unread notifications waiting in the inbox.",
    },
    inbox: {
      label: "Inbox",
      description: "The same lit units, listed so you can pick them up later.",
    },
  },
  model: {
    involvement: {
      label: "What counts as your conversation",
      description:
        "A thread is yours once you've commented in it, you're mentioned in it, or it's on a PR you opened. The PR's general comment stream is one unit the same way, and a formal review that @-mentions you is another.",
    },
    lights: {
      label: "What lights it up",
      description:
        "Someone else posts in a unit that's yours after you last engaged - a reply, a mention, a comment on your PR, or a review that mentions you.",
    },
    clears: {
      label: "What clears it",
      description:
        "Reply or resolve it on GitHub, or mark it seen - expanding a thread, or dwelling on the Comments or Reviews tab, counts. A reply clears it about a sync cycle later, once PRism sees it. Opening the PR doesn't clear a conversation.",
    },
    resolved: {
      label: "Resolved threads",
      description:
        "A resolved thread stays quiet until someone replies again - then it nags, because a post-resolution reply usually wants you.",
    },
    obligations: {
      label: "Review obligations",
      description:
        "Being asked to review, or changes requested on your own PR, lights the dot as a role you owe. It clears when you open the PR; the review-state glyph and the mergeable badge keep showing the obligation until you act on GitHub.",
    },
  },
} as const;
