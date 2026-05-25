<script setup lang="ts">
import { computed } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";

import type { PullRequestReview } from "@/types/conversation";

import { EM_DASH } from "@/lib/format";
import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismMarkdown from "@/components/ui/PRismMarkdown.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

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
  readonly bodyTrimmed: string;
}

async function openReviewOnGitHub(url: string | null): Promise<void> {
  if (url === null || url.length === 0) return;
  try {
    await openUrl(url);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("failed to open review url", err);
  }
}

const orderedReviews = computed<readonly ReviewView[]>(() => {
  const items = props.reviews
    .map<ReviewView>((review) => ({
      review,
      pill: PILL[review.state] ?? { kind: "commented", label: review.state },
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
      <PRismAvatar
        :login="entry.review.author_login"
        :avatar-url="entry.review.avatar_url"
        size="lg"
        class="review-card__avatar"
      />

      <div class="review-card__body">
        <div class="review-card__header">
          <span class="review-card__author">{{ entry.review.author_login }}</span>
          <span :class="['review-card__pill', `review-card__pill--${entry.pill.kind}`]">
            {{ entry.pill.label }}
          </span>
          <PRismRelativeTime
            v-if="entry.review.submitted_at !== null"
            :value="entry.review.submitted_at"
            class="review-card__time"
          />
          <span v-else class="review-card__time">{{ EM_DASH }}</span>
          <PRismTooltip
            v-if="entry.review.url !== null && entry.review.url.length > 0"
            text="Open review on GitHub"
          >
            <button
              type="button"
              class="review-card__icon-btn"
              aria-label="Open review on GitHub"
              @click="openReviewOnGitHub(entry.review.url)"
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 12 12"
                fill="none"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <path d="M5 3H3v6h6V7" />
                <path d="M7 2h3v3" />
                <path d="M6 6l4-4" />
              </svg>
            </button>
          </PRismTooltip>
        </div>
        <PRismMarkdown
          :html="entry.review.body_html"
          :fallback="entry.bodyTrimmed"
          class="review-card__text"
        >
          <template #empty>
            <span class="review-card__text--empty">No review summary.</span>
          </template>
        </PRismMarkdown>
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
  padding: var(--s-4);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  background: var(--bg-1);
  transition: border-color 0.12s;
}

.review-card:hover {
  border-color: var(--accent);
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

/* Per-review "Open in GitHub" affordance. Mirrors the thread-card icon-button
 * scale + interaction states. */
.review-card__icon-btn {
  background: transparent;
  border: 0;
  padding: 6px;
  border-radius: var(--r-2);
  color: var(--text-mute);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.review-card__icon-btn:hover {
  background: var(--bg-3);
  color: var(--text-strong);
}

.review-card__icon-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.review-card__text {
  /* PRismMarkdown renders the body now; `line-height` + `white-space: pre-wrap`
   * are owned by `.prism-markdown` (see ThreadsList for the same rationale).
   * Re-asserting them here inflates paragraph gaps and reads as dead space. */
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text);
  word-break: break-word;
}

.review-card__text--empty {
  color: var(--text-faint);
  font-style: italic;
}
</style>
