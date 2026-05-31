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
  readonly attention: SignalCopy;
  readonly myReview: MyReviewCopy;
  readonly threadBucket: ThreadBucketCopy;
  readonly count: CountCopy;
  readonly model: ModelCopy;
} = {
  attention: {
    label: "Needs you",
    description:
      "A conversation you're in moved, or you owe a review. Clears when you reply or mark it seen.",
  },
  myReview: {
    author: {
      label: "You authored this",
      description: "You opened this pull request.",
    },
    requested: {
      label: "Review requested",
      description: "You've been asked to review and haven't submitted yet.",
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
      description: "PRs with a lit conversation or an open review obligation.",
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
        "A thread is yours once you've commented in it, you're mentioned in it, or it's on a PR you opened. The PR's general comment stream is one unit the same way.",
    },
    lights: {
      label: "What lights it up",
      description:
        "Someone else posts in a unit that's yours after you last engaged - a reply, a mention, or a comment on your PR.",
    },
    clears: {
      label: "What clears it",
      description:
        "Open it and mark it seen, or reply on GitHub. A reply clears it about a sync cycle later, once PRism sees it.",
    },
    resolved: {
      label: "Resolved threads",
      description:
        "A resolved thread stays quiet until someone replies again - then it nags, because a post-resolution reply usually wants you.",
    },
    obligations: {
      label: "Review obligations",
      description:
        "Being asked to review, or changes requested on your own PR, lights the badge as a role you owe - it clears from GitHub state, not by marking seen.",
    },
  },
} as const;
