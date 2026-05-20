<script setup lang="ts">
import { computed } from "vue";

import type { PullRequestReview } from "@/types/conversation";

import { avatarSeed, formatRelativeAgo, initials } from "@/lib/format";

interface Props {
  reviews: readonly PullRequestReview[];
}

const props = defineProps<Props>();

type StateKind = "approved" | "changes" | "commented" | "dismissed" | "pending";

interface PillSpec {
  readonly kind: StateKind;
  readonly label: string;
}

const PILL: Record<string, PillSpec> = {
  APPROVED: { kind: "approved", label: "Approved" },
  CHANGES_REQUESTED: { kind: "changes", label: "Changes" },
  COMMENTED: { kind: "commented", label: "Commented" },
  DISMISSED: { kind: "dismissed", label: "Dismissed" },
  PENDING: { kind: "pending", label: "Pending" },
};

interface ReviewView {
  readonly review: PullRequestReview;
  readonly pill: PillSpec;
  readonly relative: string;
  readonly bodyTrimmed: string;
}

const orderedReviews = computed<readonly ReviewView[]>(() => {
  const items = props.reviews
    .map<ReviewView>((review) => ({
      review,
      pill: PILL[review.state] ?? { kind: "commented", label: review.state },
      relative: formatRelativeAgo(review.submitted_at),
      bodyTrimmed: (review.body ?? "").trim(),
    }))
    .slice();
  items.sort((a, b) => {
    // Submitted reviews first (newest -> oldest), then pending placeholders.
    const aTs = a.review.submitted_at ?? 0;
    const bTs = b.review.submitted_at ?? 0;
    if (aTs === 0 && bTs === 0) {
      return a.review.author_login.localeCompare(b.review.author_login);
    }
    if (aTs === 0) return 1;
    if (bTs === 0) return -1;
    return bTs - aTs;
  });
  return items;
});
</script>

<template>
  <div v-if="orderedReviews.length === 0" class="reviews-tab__empty">
    No reviews yet.
  </div>
  <div v-else class="reviews-tab">
    <article
      v-for="entry in orderedReviews"
      :key="entry.review.id"
      class="review-card"
    >
      <span
        :class="['avatar', avatarSeed(entry.review.author_login), 'review-card__avatar']"
        :title="entry.review.author_login"
      >{{ initials(entry.review.author_login) }}</span>

      <div class="review-card__body">
        <div class="review-card__header">
          <span class="review-card__author">{{ entry.review.author_login }}</span>
          <span :class="['review-card__pill', `review-card__pill--${entry.pill.kind}`]">
            {{ entry.pill.label }}
          </span>
          <span class="review-card__time">{{ entry.relative }}</span>
        </div>
        <p v-if="entry.bodyTrimmed !== ''" class="review-card__text">
          {{ entry.bodyTrimmed }}
        </p>
        <p v-else class="review-card__text review-card__text--empty">
          No review summary.
        </p>
      </div>
    </article>
  </div>
</template>

<style scoped>
.reviews-tab {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}

.reviews-tab__empty {
  padding: var(--s-6) 0;
  text-align: center;
  font-size: var(--fs-12);
  color: var(--text-faint);
}

.review-card {
  display: grid;
  grid-template-columns: 28px 1fr;
  gap: var(--s-3);
  padding: var(--s-4) 0;
  border-bottom: 1px solid var(--border-1);
}

.review-card:last-child {
  border-bottom: 0;
}

.review-card__avatar {
  width: 28px;
  height: 28px;
  font-size: var(--fs-11);
  align-self: start;
}

.review-card__body {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.review-card__header {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
}

.review-card__author {
  font-size: var(--fs-12);
  font-weight: 600;
  color: var(--text-strong);
}

.review-card__pill {
  display: inline-flex;
  align-items: center;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  padding: 2px 6px;
  border-radius: var(--r-1);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  font-weight: 500;
}

.review-card__pill--approved {
  background: var(--success-bg);
  color: var(--success);
}

.review-card__pill--changes {
  background: var(--danger-bg);
  color: var(--danger);
}

.review-card__pill--commented {
  background: var(--info-bg);
  color: var(--info);
}

.review-card__pill--dismissed {
  background: var(--bg-4);
  color: var(--text-mute);
}

.review-card__pill--pending {
  background: var(--bg-4);
  color: var(--text-mute);
}

.review-card__time {
  margin-left: auto;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
}

.review-card__text {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text);
  line-height: var(--lh-body);
  white-space: pre-wrap;
  word-break: break-word;
}

.review-card__text--empty {
  color: var(--text-faint);
  font-style: italic;
}
</style>
