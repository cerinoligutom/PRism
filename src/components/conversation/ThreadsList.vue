<script setup lang="ts">
import { computed, ref } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";

import type {
  PullRequestThread,
  ThreadComment,
  ThreadState,
} from "@/types/conversation";

import { secondsSince } from "@/lib/format";
import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismMarkdown from "@/components/ui/PRismMarkdown.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";

interface Props {
  threads: readonly PullRequestThread[];
  /** Comments for the active PR, already hydrated by `fetch_pr_conversation`.
   * The expand affordance filters this by `thread_id` and renders inline; no
   * extra round-trip. */
  threadComments?: readonly ThreadComment[];
}

const props = withDefaults(defineProps<Props>(), {
  threadComments: () => [],
});

/**
 * Per-thread bucket matching ADR 0012's four-state palette. Drives the icon
 * colour and tooltip label on the leftmost icon. Computed from
 * `(is_resolved, is_involved)` so outdated-but-resolved threads pick the
 * resolved (blue/green) icon rather than collapsing into an "unresolved"
 * variant. `is_outdated` is rendered orthogonally via the dim treatment +
 * existing `OUTDATED` badge.
 */
type ThreadBucket =
  | "unresolved-uninvolved"
  | "unresolved-involved"
  | "resolved-uninvolved"
  | "resolved-involved";

interface BucketSpec {
  readonly key: ThreadBucket;
  readonly label: string;
  readonly resolvedShape: boolean;
}

const BUCKETS: Readonly<Record<ThreadBucket, BucketSpec>> = {
  "unresolved-uninvolved": {
    key: "unresolved-uninvolved",
    label: "Unresolved",
    resolvedShape: false,
  },
  "unresolved-involved": {
    key: "unresolved-involved",
    label: "Unresolved (involved)",
    resolvedShape: false,
  },
  "resolved-uninvolved": {
    key: "resolved-uninvolved",
    label: "Resolved",
    resolvedShape: true,
  },
  "resolved-involved": {
    key: "resolved-involved",
    label: "Resolved (involved)",
    resolvedShape: true,
  },
};

function bucketFor(thread: PullRequestThread): BucketSpec {
  const resolvedKey = thread.is_resolved ? "resolved" : "unresolved";
  const involvedKey = thread.is_involved ? "involved" : "uninvolved";
  return BUCKETS[`${resolvedKey}-${involvedKey}` as ThreadBucket];
}

function bucketTooltip(thread: PullRequestThread): string {
  const spec = bucketFor(thread);
  if (thread.is_outdated) return `${spec.label} (outdated)`;
  return spec.label;
}

const orderedThreads = computed<readonly PullRequestThread[]>(() => {
  // Unresolved-and-involved first, then unresolved, then resolved, then
  // outdated. Inside each bucket, fall back to the most recent activity so
  // the active conversation surfaces first. Outdated rows always render now
  // (ADR 0012); the per-row dim treatment + badge carry the visual cue.
  const buckets: Record<ThreadState, number> = {
    unresolved: 0,
    resolved: 1,
    outdated: 2,
  };
  return [...props.threads].sort((a, b) => {
    const aWeight = buckets[a.state] + (a.is_involved && a.state === "unresolved" ? -1 : 0);
    const bWeight = buckets[b.state] + (b.is_involved && b.state === "unresolved" ? -1 : 0);
    if (aWeight !== bWeight) return aWeight - bWeight;
    const aTs = a.last_reply_at ?? a.created_at ?? 0;
    const bTs = b.last_reply_at ?? b.created_at ?? 0;
    return bTs - aTs;
  });
});

const expanded = ref<Set<number>>(new Set());

function isExpanded(id: number): boolean {
  return expanded.value.has(id);
}

function toggleExpanded(id: number): void {
  const next = new Set(expanded.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  expanded.value = next;
}

function threadKey(t: PullRequestThread): string {
  return `${t.id}:${t.node_id}`;
}

function lineSuffix(t: PullRequestThread): string {
  if (t.line === null) {
    return t.original_line !== null ? `:${t.original_line}` : "";
  }
  if (t.start_line !== null && t.start_line !== t.line) {
    return `:${t.start_line}-${t.line}`;
  }
  return `:${t.line}`;
}


function snippetText(t: PullRequestThread): string {
  return t.head_comment?.body_text.trim() ?? "";
}

function snippetAuthor(t: PullRequestThread): string {
  return t.head_comment?.author_login ?? "";
}

function isStale(t: PullRequestThread): boolean {
  // Visual stale cue: highlight unresolved threads that haven't moved in a week
  // so reviewers can spot stalled conversations. Not part of the contract;
  // local presentation hint only.
  if (t.state !== "unresolved") return false;
  const reference = t.last_reply_at ?? t.created_at;
  if (reference === null) return false;
  return secondsSince(reference) > 7 * 24 * 60 * 60;
}

/** Comments belonging to a single thread, ordered chronologically. */
function commentsFor(threadId: number): readonly ThreadComment[] {
  const matches = props.threadComments.filter((c) => c.thread_id === threadId);
  return [...matches].sort((a, b) => a.created_at - b.created_at);
}

function expandTooltip(t: PullRequestThread): string {
  if (isExpanded(t.id)) return "Hide comments";
  const count = commentsFor(t.id).length;
  if (count === 0) return "No comments to show";
  const noun = count === 1 ? "comment" : "comments";
  return `Show ${count} ${noun}`;
}

async function openThreadOnGitHub(url: string | null): Promise<void> {
  if (url === null || url.length === 0) return;
  try {
    await openUrl(url);
  } catch (err) {
    // Avoid throwing into the template; surface a console hint for diagnostics.
    // eslint-disable-next-line no-console
    console.warn("failed to open thread url", err);
  }
}

async function openCommentOnGitHub(
  event: Event,
  url: string | null,
): Promise<void> {
  // The comment row is inside the expand toggle's container; without
  // stopPropagation a click on this button would also fire the row-level
  // toggle handler and collapse the thread.
  event.stopPropagation();
  if (url === null || url.length === 0) return;
  try {
    await openUrl(url);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("failed to open comment url", err);
  }
}
</script>

<template>
  <div class="threads-list">
    <div v-if="orderedThreads.length === 0" class="threads-list__empty">
      <p>No review threads yet.</p>
    </div>

    <div v-else class="threads-list__items">
      <article
        v-for="thread in orderedThreads"
        :key="threadKey(thread)"
        :class="[
          'thread-card',
          `thread-card--${bucketFor(thread).key}`,
          thread.is_outdated && 'thread-card--outdated',
          thread.is_involved && !thread.is_resolved && 'thread-card--mine',
          isStale(thread) && 'thread-card--stale',
        ]"
      >
        <PRismTooltip :text="bucketTooltip(thread)" :as-child="true">
          <span
            :class="[
              'thread-card__state',
              `thread-card__state--${bucketFor(thread).key}`,
            ]"
            :aria-label="bucketTooltip(thread)"
          >
            <svg
              v-if="bucketFor(thread).resolvedShape"
              width="8"
              height="8"
              viewBox="0 0 8 8"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
            >
              <path d="M2 4l1.5 1.5L6 2.5" />
            </svg>
            <svg v-else width="7" height="7" viewBox="0 0 8 8">
              <circle cx="4" cy="4" r="3" fill="currentColor" />
            </svg>
          </span>
        </PRismTooltip>

        <div class="thread-card__body">
          <div class="thread-card__file">
            <span v-if="thread.path !== null" class="thread-card__path">{{ thread.path }}</span>
            <span v-else class="thread-card__path thread-card__path--missing">No file path</span>
            <span v-if="lineSuffix(thread) !== ''" class="thread-card__line">{{ lineSuffix(thread) }}</span>
            <span
              v-if="thread.is_involved && !thread.is_resolved"
              class="thread-card__chip thread-card__chip--mine"
            >INVOLVED</span>
            <span
              v-if="thread.is_outdated"
              class="thread-card__chip thread-card__chip--outdated"
            >OUTDATED</span>
          </div>

          <div class="thread-card__snippet">
            <PRismAvatar
              v-if="snippetAuthor(thread) !== ''"
              :login="snippetAuthor(thread)"
              :avatar-url="thread.head_comment?.avatar_url ?? null"
              size="sm"
              class="thread-card__avatar"
            />
            <p>
              <span v-if="snippetAuthor(thread) !== ''" class="thread-card__author">
                {{ snippetAuthor(thread) }}:
              </span>
              <span v-if="snippetText(thread) !== ''">{{ snippetText(thread) }}</span>
              <span v-else class="thread-card__snippet-missing">No preview available.</span>
            </p>
          </div>

          <div
            v-if="isExpanded(thread.id) && commentsFor(thread.id).length > 0"
            class="thread-card__replies-list"
          >
            <div
              v-for="comment in commentsFor(thread.id)"
              :key="comment.id"
              class="thread-comment"
            >
              <PRismAvatar
                :login="comment.author_login"
                :avatar-url="comment.avatar_url"
                size="sm"
                class="thread-comment__avatar"
              />
              <div class="thread-comment__body">
                <div class="thread-comment__meta">
                  <span class="thread-comment__author">{{ comment.author_login }}</span>
                  <PRismRelativeTime
                    :value="comment.created_at"
                    class="thread-comment__ts"
                  />
                  <PRismTooltip
                    v-if="comment.url !== null && comment.url.length > 0"
                    text="Open comment on GitHub"
                  >
                    <button
                      type="button"
                      class="thread-comment__icon-btn"
                      aria-label="Open comment on GitHub"
                      @click="openCommentOnGitHub($event, comment.url)"
                    >
                      <svg
                        width="11"
                        height="11"
                        viewBox="0 0 12 12"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="1.5"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                      >
                        <path d="M5 3H3v6h6V7" />
                        <path d="M7 2h3v3" />
                        <path d="M6 6l4-4" />
                      </svg>
                    </button>
                  </PRismTooltip>
                </div>
                <PRismMarkdown
                  :html="comment.body_html"
                  :fallback="comment.body"
                  class="thread-comment__text"
                />
              </div>
            </div>
          </div>
          <p
            v-else-if="isExpanded(thread.id)"
            class="thread-card__replies-empty"
          >No comments loaded for this thread.</p>
        </div>

        <div class="thread-card__meta">
          <div v-if="thread.reply_count > 0" class="thread-card__replies">
            {{ thread.reply_count }} {{ thread.reply_count === 1 ? "reply" : "replies" }}
          </div>
          <div class="thread-card__opened">
            <template v-if="thread.created_at !== null">
              opened <PRismRelativeTime :value="thread.created_at" />
            </template>
            <template v-else>—</template>
          </div>
          <div class="thread-card__activity">
            <template v-if="thread.state === 'resolved'">
              <template v-if="thread.resolved_at !== null">
                resolved <PRismRelativeTime :value="thread.resolved_at" />
              </template>
              <template v-else>resolved</template>
            </template>
            <template v-else-if="thread.state === 'outdated'">outdated</template>
            <template v-else-if="thread.last_reply_at !== null">
              last <PRismRelativeTime :value="thread.last_reply_at" />
            </template>
            <template v-else>—</template>
          </div>
          <div class="thread-card__actions">
            <PRismTooltip :text="expandTooltip(thread)">
              <button
                type="button"
                class="thread-card__icon-btn"
                :aria-expanded="isExpanded(thread.id)"
                :aria-label="expandTooltip(thread)"
                @click="toggleExpanded(thread.id)"
              >
                <svg
                  width="12"
                  height="12"
                  viewBox="0 0 12 12"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  :class="[
                    'thread-card__chevron',
                    isExpanded(thread.id) && 'thread-card__chevron--open',
                  ]"
                >
                  <path d="M3 4.5l3 3 3-3" />
                </svg>
              </button>
            </PRismTooltip>
            <PRismTooltip
              v-if="thread.url !== null && thread.url.length > 0"
              text="Open thread on GitHub"
            >
              <button
                type="button"
                class="thread-card__icon-btn"
                aria-label="Open thread on GitHub"
                @click="openThreadOnGitHub(thread.url)"
              >
                <svg
                  width="12"
                  height="12"
                  viewBox="0 0 12 12"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <path d="M5 3H3v6h6V7" />
                  <path d="M7 2h3v3" />
                  <path d="M6 6l4-4" />
                </svg>
              </button>
            </PRismTooltip>
          </div>
        </div>
      </article>
    </div>
  </div>
</template>

<style scoped>
.threads-list {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

.threads-list__items {
  display: flex;
  flex-direction: column;
}

.threads-list__empty {
  padding: var(--s-5) 0;
  text-align: center;
  font-size: var(--fs-12);
  color: var(--text-faint);
}

.threads-list__empty p {
  margin: 0;
}

.thread-card {
  display: grid;
  grid-template-columns: 14px 1fr auto;
  gap: var(--s-3);
  padding: var(--s-4) 0;
  border-bottom: 1px solid var(--border-1);
}

.thread-card:last-child {
  border-bottom: 0;
}

.thread-card--mine {
  background: linear-gradient(
    90deg,
    oklch(0.4 0.12 var(--accent-h) / 0.18),
    transparent 60%
  );
  border-radius: var(--r-2);
  padding-left: var(--s-3);
  padding-right: var(--s-3);
  margin-left: calc(-1 * var(--s-3));
  margin-right: calc(-1 * var(--s-3));
}

.thread-card--outdated {
  opacity: 0.65;
}

.thread-card__state {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin-top: 4px;
  flex: 0 0 14px;
}

.thread-card__state--unresolved-uninvolved {
  background: oklch(from var(--danger) l c h / 0.18);
  color: var(--danger);
}

.thread-card__state--unresolved-involved {
  background: oklch(from var(--warning) l c h / 0.2);
  color: var(--warning);
}

.thread-card__state--resolved-uninvolved {
  background: oklch(from var(--info) l c h / 0.18);
  color: var(--info);
}

.thread-card__state--resolved-involved {
  background: var(--success-bg);
  color: var(--success);
}

.thread-card__body {
  min-width: 0;
}

.thread-card__file {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-mute);
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
}

.thread-card__path {
  color: var(--text-mute);
  word-break: break-all;
}

.thread-card__path--missing {
  color: var(--text-faint);
  font-style: italic;
}

.thread-card__line {
  color: var(--text-faint);
}

.thread-card__chip {
  margin-left: 4px;
  font-family: var(--font-mono);
  font-size: var(--fs-9);
  padding: 1px 5px;
  border-radius: 2px;
  letter-spacing: 0.5px;
}

.thread-card__chip--mine {
  background: var(--accent-bg);
  color: var(--accent-strong);
}

.thread-card__chip--outdated {
  background: var(--bg-4);
  color: var(--text-mute);
}

.thread-card__snippet {
  margin-top: var(--s-2);
  display: flex;
  align-items: flex-start;
  gap: 6px;
  font-size: var(--fs-12);
  color: var(--text);
}

.thread-card__avatar {
  flex: 0 0 16px;
  margin-top: 2px;
}

.thread-card__snippet p {
  margin: 0;
  flex: 1;
  line-height: var(--lh-body);
  color: var(--text);
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}

.thread-card--outdated .thread-card__snippet p {
  color: var(--text-mute);
}

.thread-card__author {
  color: var(--text-mute);
  font-weight: 500;
  margin-right: 4px;
}

.thread-card__snippet-missing {
  color: var(--text-faint);
  font-style: italic;
}

.thread-card__replies-list {
  margin-top: var(--s-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  border-left: 2px solid var(--border-1);
  padding-left: var(--s-3);
}

.thread-card__replies-empty {
  margin: var(--s-3) 0 0;
  font-size: var(--fs-11);
  color: var(--text-faint);
  font-style: italic;
}

.thread-comment {
  display: flex;
  align-items: flex-start;
  gap: var(--s-2);
}

.thread-comment__avatar {
  flex: 0 0 16px;
  margin-top: 2px;
}

.thread-comment__body {
  flex: 1;
  min-width: 0;
}

.thread-comment__meta {
  display: flex;
  align-items: baseline;
  gap: var(--s-2);
  font-size: var(--fs-11);
}

.thread-comment__author {
  color: var(--text-strong);
  font-weight: 500;
}

.thread-comment__ts {
  color: var(--text-faint);
  font-family: var(--font-mono);
  font-size: var(--fs-10);
}

.thread-comment__icon-btn {
  background: transparent;
  border: 0;
  padding: 2px;
  border-radius: var(--r-1);
  color: var(--text-faint);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  margin-left: auto;
}

.thread-comment__icon-btn:hover {
  background: var(--bg-3);
  color: var(--text-strong);
}

.thread-comment__icon-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.thread-comment__text {
  margin: 2px 0 0;
  font-size: var(--fs-12);
  line-height: var(--lh-body);
  color: var(--text);
  white-space: pre-wrap;
  word-break: break-word;
}

.thread-card__meta {
  text-align: right;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  white-space: nowrap;
}

.thread-card__replies {
  color: var(--text-mute);
  margin-bottom: 2px;
}

.thread-card--stale .thread-card__activity {
  color: var(--warning);
}

.thread-card__actions {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  margin-top: var(--s-2);
  justify-content: flex-end;
}

.thread-card__icon-btn {
  background: transparent;
  border: 0;
  padding: 4px;
  border-radius: var(--r-1);
  color: var(--text-mute);
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.thread-card__icon-btn:hover {
  background: var(--bg-3);
  color: var(--text-strong);
}

.thread-card__icon-btn:focus-visible {
  outline: none;
  box-shadow: 0 0 0 2px var(--focus-ring);
}

.thread-card__chevron {
  transition: transform 120ms ease-out;
}

.thread-card__chevron--open {
  transform: rotate(180deg);
}
</style>
