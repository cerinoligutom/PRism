<script setup lang="ts">
import { computed, ref } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";

import type {
  PullRequestThread,
  ThreadComment,
  ThreadState,
} from "@/types/conversation";

import { secondsSince } from "@/lib/format";
import { threadAnchorId } from "@/composables/useThreadDeepLink";
import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismAvatarStack from "@/components/ui/PRismAvatarStack.vue";
import PRismButton from "@/components/ui/PRismButton.vue";
import PRismMarkdown from "@/components/ui/PRismMarkdown.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import ThreadStateIcon from "./icons/ThreadStateIcon.vue";
import DiffHunkBlock from "./DiffHunkBlock.vue";

interface Props {
  threads: readonly PullRequestThread[];
  /** Comments for the active PR, already hydrated by `fetch_pr_conversation`.
   * The expand affordance filters this by `thread_id` and renders inline; no
   * extra round-trip. */
  threadComments?: readonly ThreadComment[];
  /** Whether the viewer holds a relation for this PR (ADR 0031). Gates the
   * per-thread "Mark seen" affordance - a unit can't be marked seen when
   * there's no viewer whose watermark to advance. */
  canMarkSeen?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  threadComments: () => [],
  canMarkSeen: false,
});

const emit = defineEmits<{
  "mark-seen": [thread: PullRequestThread];
}>();

function onMarkSeen(event: Event, thread: PullRequestThread): void {
  // Sits inside the row's expand-toggle container; stop the click bubbling so
  // marking seen doesn't also expand / collapse the card.
  event.stopPropagation();
  emit("mark-seen", thread);
}

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

function toggleExpanded(thread: PullRequestThread): void {
  const id = thread.id;
  const next = new Set(expanded.value);
  const wasExpanded = next.has(id);
  if (wasExpanded) next.delete(id);
  else next.add(id);
  expanded.value = next;
  // ADR 0033 expand-to-seen: expanding an unread thread is a deliberate
  // interaction, so mark it seen via the same event the manual button emits.
  // Fires only on collapsed -> expanded, and no-ops once the thread is already
  // seen. Collapsing never clears anything.
  if (!wasExpanded && props.canMarkSeen && thread.unread) {
    emit("mark-seen", thread);
  }
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

interface ThreadParticipant {
  readonly login: string;
  readonly avatar_url: string | null;
}

/** Unique participants in a thread, head-comment author first, then reply
 * authors in chronological order. Dedupes by login. Thread comments
 * lazy-hydrate per ADR 0010; avatars come from the cached `users` table
 * (ADR 0013). Returns an empty array when only the head-comment author is
 * present so the consumer can skip rendering a single-avatar stack. */
function participantsFor(t: PullRequestThread): readonly ThreadParticipant[] {
  const seen = new Set<string>();
  const out: ThreadParticipant[] = [];
  const headLogin = t.head_comment?.author_login ?? "";
  if (headLogin.length > 0) {
    seen.add(headLogin);
    out.push({ login: headLogin, avatar_url: t.head_comment?.avatar_url ?? null });
  }
  for (const c of commentsFor(t.id)) {
    if (c.author_login.length === 0 || seen.has(c.author_login)) continue;
    seen.add(c.author_login);
    out.push({ login: c.author_login, avatar_url: c.avatar_url });
  }
  // Single-author threads are just the head comment - skip the stack.
  return out.length > 1 ? out : [];
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
        :id="threadAnchorId(thread.node_id)"
        :key="threadKey(thread)"
        :class="[
          'thread-card',
          `thread-card--${bucketFor(thread).key}`,
          thread.is_outdated && 'thread-card--outdated',
          thread.is_involved && !thread.is_resolved && 'thread-card--mine',
          isStale(thread) && 'thread-card--stale',
          thread.unread && 'thread-card--unread',
        ]"
        role="button"
        tabindex="0"
        :aria-expanded="isExpanded(thread.id)"
        @click="toggleExpanded(thread)"
        @keydown.enter.prevent="toggleExpanded(thread)"
        @keydown.space.prevent="toggleExpanded(thread)"
      >
        <PRismTooltip :as-child="true">
          <span
            :class="[
              'thread-card__state',
              `thread-card__state--${bucketFor(thread).key}`,
            ]"
            :aria-label="bucketTooltip(thread)"
          >
            <ThreadStateIcon
              :state="bucketFor(thread).resolvedShape ? 'resolved' : 'unresolved'"
            />
          </span>
          <template #content>
            <div class="thread-state-tip">
              <span class="thread-state-tip__label">
                {{ thread.is_resolved ? "Resolved" : "Unresolved" }}
              </span>
              <span
                v-if="thread.is_involved && !thread.is_resolved"
                class="thread-card__chip thread-card__chip--mine"
              >INVOLVED</span>
              <span
                v-if="thread.is_outdated"
                class="thread-card__chip thread-card__chip--outdated"
              >OUTDATED</span>
            </div>
          </template>
        </PRismTooltip>

        <div class="thread-card__file">
          <div class="thread-card__file-line">
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

          <div
            v-if="participantsFor(thread).length > 0 || thread.reply_count > 0"
            class="thread-card__file-people"
          >
            <PRismTooltip
              v-if="participantsFor(thread).length > 0"
              :as-child="true"
            >
              <div class="thread-card__participants">
                <PRismAvatarStack
                  :users="participantsFor(thread)"
                  size="sm"
                  layout="overlap"
                />
              </div>
              <template #content>
                <ul class="thread-card__participants-tooltip">
                  <li
                    v-for="p in participantsFor(thread)"
                    :key="p.login"
                    class="thread-card__participants-tooltip-row"
                  >
                    <PRismAvatar
                      :login="p.login"
                      :avatar-url="p.avatar_url"
                      size="sm"
                      :tooltip="null"
                    />
                    <span class="thread-card__participants-tooltip-login">{{ p.login }}</span>
                  </li>
                </ul>
              </template>
            </PRismTooltip>
            <div v-if="thread.reply_count > 0" class="thread-card__replies">
              {{ thread.reply_count }} {{ thread.reply_count === 1 ? "reply" : "replies" }}
            </div>
          </div>
        </div>

        <div class="thread-card__body">
          <!-- The wrapping div carries `@click.stop` so a click on the
               diff-hunk block (e.g. selecting text) doesn't bubble up to
               the row-level expand toggle. -->
          <div
            v-if="thread.diff_hunk !== null && thread.diff_hunk.length > 0"
            class="thread-card__diff-hunk"
            @click.stop
          >
            <DiffHunkBlock
              :hunk="thread.diff_hunk"
              :path="thread.path"
            />
          </div>

          <div v-if="!isExpanded(thread.id)" class="thread-card__snippet">
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
            @click.stop
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
            <PRismButton
              v-if="canMarkSeen && thread.unread"
              variant="ghost"
              size="sm"
              @click="onMarkSeen($event, thread)"
            >
              Mark seen
            </PRismButton>
            <PRismTooltip :text="expandTooltip(thread)">
              <button
                type="button"
                class="thread-card__icon-btn"
                :aria-expanded="isExpanded(thread.id)"
                :aria-label="expandTooltip(thread)"
                @click.stop="toggleExpanded(thread)"
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
                @click.stop="openThreadOnGitHub(thread.url)"
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
  gap: var(--s-4);
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
  grid-template-columns: 22px 1fr auto;
  grid-template-rows: auto auto;
  column-gap: var(--s-3);
  row-gap: var(--s-2);
  padding: var(--s-4);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  background: var(--bg-1);
  transition: border-color 0.12s;
}

.thread-card__file {
  grid-column: 2;
  grid-row: 1;
  align-self: start;
  min-width: 0;
}

.thread-card__meta {
  grid-column: 3;
  grid-row: 1;
  align-self: start;
}

.thread-card__body {
  grid-column: 2 / -1;
  grid-row: 2;
  min-width: 0;
}

.thread-card:hover {
  border-color: var(--accent);
}

.thread-card:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
}

.thread-card--mine {
  /* Needs-attention thread: clear coloured left-edge accent only. No bg tint -
   * the strip is the scan signal on its own. Uses `--accent` so the cue says
   * "this needs YOU", matching the row attention signal M4 uses elsewhere. */
  border-left-width: 3px;
  border-left-color: var(--accent);
}

.thread-card--outdated {
  opacity: 0.65;
}

/* Deep-link landing highlight (ADR 0031, issue #437). Added for ~2s by the
 * conversation surface after scrolling a notification's target thread into
 * view, then removed. A ring + faint tint so the user's eye lands on the
 * right card without a layout shift. */
.thread-card--deep-link {
  box-shadow: 0 0 0 2px var(--accent);
  background: var(--accent-bg);
  transition: box-shadow 0.2s, background 0.2s;
}

/* Inline state badge layout. Shape + colour variants live in
 * `assets/styles/pr-status.css` so the legend tooltips on other surfaces
 * pick up the same look. Only the alignment offset is local. */
.thread-card__state {
  margin-top: 2px;
}

/* Brighter bucket badge for unread threads. The bg alpha jumps from 0.18-0.2
 * to ~0.4 so the badge itself communicates "new activity" without extra chrome. */
.thread-card--unread .thread-card__state--unresolved-uninvolved {
  background: oklch(from var(--danger) l c h / 0.4);
}
.thread-card--unread .thread-card__state--unresolved-involved {
  background: oklch(from var(--warning) l c h / 0.4);
}
.thread-card--unread .thread-card__state--resolved-uninvolved {
  background: oklch(from var(--info) l c h / 0.4);
}
.thread-card--unread .thread-card__state--resolved-involved {
  background: oklch(from var(--success) l c h / 0.4);
}

.thread-card__body {
  min-width: 0;
}

.thread-card__file {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-mute);
  display: flex;
  flex-direction: column;
  gap: 6px;
  min-width: 0;
}

.thread-card__file-line {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: nowrap;
  min-width: 0;
}

.thread-card__file-people {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  color: var(--text-faint);
}

.thread-card__path {
  color: var(--text-mute);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  flex: 0 1 auto;
}

.thread-card__line {
  flex-shrink: 0;
  color: var(--text-faint);
}

.thread-card__path--missing {
  color: var(--text-faint);
  font-style: italic;
}


/* `.thread-card__chip` / `--mine` / `--outdated` live in
 * `assets/styles/pr-status.css` so the same chip pill renders identically
 * inside the state-badge tooltip and the conversation legend popover. */

.thread-card__diff-hunk {
  /* Sits between the file-path row and the snippet; the component's own
   * top margin handles the gap from the path row. Reset the cursor since
   * the parent card carries `role="button"` - the hunk itself isn't an
   * action target. */
  cursor: default;
}

.thread-card__snippet {
  margin-top: var(--s-2);
  display: flex;
  align-items: flex-start;
  gap: 6px;
  font-size: var(--fs-13);
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

/* Unread threads carry three layered cues: brighter bucket badge (above),
 * full-strength bold text on the snippet author / body / file-path, and a
 * small accent dot prefixing the last-activity timestamp. Read threads
 * default to muted text so the contrast carries without extra chrome. */
.thread-card--unread .thread-card__author,
.thread-card--unread .thread-card__snippet p {
  font-weight: 600;
  color: var(--text-strong);
}

.thread-card--unread .thread-card__path {
  color: var(--text-strong);
}

.thread-card--unread .thread-card__activity::before {
  content: "\2022";
  color: var(--accent);
  margin-right: 4px;
  font-size: 1.4em;
  vertical-align: middle;
  line-height: 0;
}

.thread-card__snippet-missing {
  color: var(--text-faint);
  font-style: italic;
}

/* Participants strip - "who's in this conversation". Sits below the snippet
 * / replies list inside the body column. Only renders when 2+ distinct
 * authors are involved (single-author threads collapse it out). The stack's
 * `--bg-1` ring matches the card background so the avatars read as inset
 * peers rather than floating chips. */
/* Participants stack lives at the top of the meta column - reads as thread
 * metadata (alongside reply count, opened, last activity) rather than as a
 * floating row of avatars below the snippet body. Right-aligned to match the
 * rest of the meta column. */
.thread-card__participants {
  display: flex;
  align-items: center;
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
  font-size: var(--fs-13);
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
  /* PRismMarkdown renders the body now; line-height + `white-space: pre-wrap`
   * are owned by `.prism-markdown` in `markdown.css`. Re-asserting them here
   * (scoped selectors get higher specificity than the global rule) preserves
   * literal whitespace between `<p>` tags and inflates the line-box, surfacing
   * as huge gaps between paragraphs. Leave only the host-level margin. */
  margin: 2px 0 0;
  font-size: var(--fs-13);
  color: var(--text);
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
  padding: 6px;
  border-radius: var(--r-2);
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

<!-- Participants tooltip lives outside scoped CSS because Reka's TooltipPortal
     teleports content outside the component's `data-v-*` boundary. Mirrors the
     ReviewerStack overflow-tooltip pattern. -->
<style>
.thread-card__participants-tooltip {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-width: 280px;
}

.thread-card__participants-tooltip-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.thread-card__participants-tooltip-login {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-strong);
}
</style>

<!--
  Unscoped because Reka's `TooltipPortal` teleports the state-badge tooltip
  out of the scoped-CSS attribute boundary. Only the local layout for the
  tooltip wrapper needs to survive the portal here; the chip + badge classes
  themselves come from `assets/styles/pr-status.css`.
-->
<style>
.thread-state-tip {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.thread-state-tip__label {
  font-weight: 600;
  color: var(--text-strong);
}
</style>
