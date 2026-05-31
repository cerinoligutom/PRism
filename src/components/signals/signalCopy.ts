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

export const SIGNAL_COPY: {
  readonly attention: SignalCopy;
  readonly myReview: MyReviewCopy;
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
} as const;
