<script setup lang="ts">
import { computed } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";

import type { IssueComment } from "@/types/conversation";

import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismMarkdown from "@/components/ui/PRismMarkdown.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

/**
 * PR-level "issue comments" (top-level PR conversation, distinct from inline
 * review threads and review summary bodies). Renders the cards Cloudflare /
 * CodeRabbit / human chat lands in. Mirrors the `ReviewsTab` card pattern;
 * markdown rendering goes through `PRismMarkdown` so the same DOMPurify +
 * Shiki path applies. See ADR 0014.
 */

interface Props {
  issueComments: readonly IssueComment[];
}

const props = defineProps<Props>();

interface CommentView {
  readonly comment: IssueComment;
  readonly bodyTrimmed: string;
}

const orderedComments = computed<readonly CommentView[]>(() => {
  // Oldest -> newest. PR-level threads on GitHub read top-down chronologically,
  // matching how a user scans the conversation timeline.
  return props.issueComments
    .map<CommentView>((comment) => ({
      comment,
      bodyTrimmed: (comment.body ?? "").trim(),
    }))
    .slice()
    .sort((a, b) => a.comment.created_at - b.comment.created_at);
});

async function openCommentOnGitHub(url: string | null): Promise<void> {
  if (url === null || url.length === 0) return;
  try {
    await openUrl(url);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("failed to open issue comment url", err);
  }
}
</script>

<template>
  <div v-if="orderedComments.length === 0" class="issue-comments-tab__empty">
    No comments on the PR conversation yet.
  </div>
  <div v-else class="issue-comments-tab">
    <article
      v-for="entry in orderedComments"
      :key="entry.comment.id"
      class="issue-comment-card"
    >
      <PRismAvatar
        :login="entry.comment.author_login"
        :avatar-url="entry.comment.avatar_url"
        size="lg"
        class="issue-comment-card__avatar"
      />

      <div class="issue-comment-card__body">
        <div class="issue-comment-card__header">
          <span class="issue-comment-card__author">{{ entry.comment.author_login }}</span>
          <PRismRelativeTime
            :value="entry.comment.created_at"
            class="issue-comment-card__time"
          />
          <PRismTooltip
            v-if="entry.comment.url !== null && entry.comment.url.length > 0"
            text="Open comment on GitHub"
          >
            <button
              type="button"
              class="issue-comment-card__icon-btn"
              aria-label="Open comment on GitHub"
              @click="openCommentOnGitHub(entry.comment.url)"
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
          :html="entry.comment.body_html"
          :fallback="entry.bodyTrimmed"
          class="issue-comment-card__text"
        >
          <template #empty>
            <span class="issue-comment-card__text--empty">No comment body.</span>
          </template>
        </PRismMarkdown>
      </div>
    </article>
  </div>
</template>

<style scoped>
.issue-comments-tab {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}

.issue-comments-tab__empty {
  padding: var(--s-6) 0;
  text-align: center;
  font-size: var(--fs-12);
  color: var(--text-faint);
}

.issue-comment-card {
  display: grid;
  grid-template-columns: 28px 1fr;
  gap: var(--s-3);
  padding: var(--s-4);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  background: var(--bg-1);
  transition: background 0.12s;
}

.issue-comment-card:hover {
  background: var(--bg-0);
}

.issue-comment-card__avatar {
  width: 28px;
  height: 28px;
  font-size: var(--fs-11);
  align-self: start;
}

.issue-comment-card__body {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.issue-comment-card__header {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
}

.issue-comment-card__author {
  font-size: var(--fs-12);
  font-weight: 600;
  color: var(--text-strong);
}

.issue-comment-card__time {
  margin-left: auto;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
}

.issue-comment-card__icon-btn {
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

.issue-comment-card__icon-btn:hover {
  background: var(--bg-3);
  color: var(--text-strong);
}

.issue-comment-card__icon-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.issue-comment-card__text {
  margin: 0;
  font-size: var(--fs-12);
  color: var(--text);
  word-break: break-word;
}

.issue-comment-card__text--empty {
  color: var(--text-faint);
  font-style: italic;
}
</style>
