<script setup lang="ts">
import { computed } from "vue";

import type { PullRequestThread, ThreadState } from "@/types/conversation";

import {
  EM_DASH,
  formatRelativeAgo,
  secondsSince,
} from "@/lib/format";
import PRismAvatar from "@/components/ui/PRismAvatar.vue";

interface Props {
  threads: readonly PullRequestThread[];
}

const props = defineProps<Props>();

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

function openedRelative(t: PullRequestThread): string {
  if (t.created_at === null) return EM_DASH;
  return `opened ${formatRelativeAgo(t.created_at)}`;
}

function activityRelative(t: PullRequestThread): string {
  if (t.state === "resolved") {
    return t.resolved_at !== null
      ? `resolved ${formatRelativeAgo(t.resolved_at)}`
      : "resolved";
  }
  if (t.state === "outdated") return "outdated";
  if (t.last_reply_at !== null) {
    return `last ${formatRelativeAgo(t.last_reply_at)}`;
  }
  return EM_DASH;
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
          `thread-card--${thread.state}`,
          thread.is_involved && thread.state === 'unresolved' && 'thread-card--mine',
          isStale(thread) && 'thread-card--stale',
        ]"
      >
        <span :class="['thread-card__state', `thread-card__state--${thread.state}`]" aria-hidden="true">
          <svg v-if="thread.state === 'unresolved'" width="7" height="7" viewBox="0 0 8 8">
            <circle cx="4" cy="4" r="3" fill="currentColor" />
          </svg>
          <svg v-else-if="thread.state === 'resolved'" width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <path d="M2 4l1.5 1.5L6 2.5" />
          </svg>
          <svg v-else width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M2 4h4M5 2.5l1.5 1.5L5 5.5" />
          </svg>
        </span>

        <div class="thread-card__body">
          <div class="thread-card__file">
            <span v-if="thread.path !== null" class="thread-card__path">{{ thread.path }}</span>
            <span v-else class="thread-card__path thread-card__path--missing">No file path</span>
            <span v-if="lineSuffix(thread) !== ''" class="thread-card__line">{{ lineSuffix(thread) }}</span>
            <span
              v-if="thread.is_involved && thread.state === 'unresolved'"
              class="thread-card__chip thread-card__chip--mine"
            >INVOLVED</span>
            <span
              v-if="thread.state === 'outdated'"
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
        </div>

        <div class="thread-card__meta">
          <div v-if="thread.reply_count > 0" class="thread-card__replies">
            {{ thread.reply_count }} {{ thread.reply_count === 1 ? "reply" : "replies" }}
          </div>
          <div class="thread-card__opened">{{ openedRelative(thread) }}</div>
          <div class="thread-card__activity">{{ activityRelative(thread) }}</div>
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
  opacity: 0.7;
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

.thread-card__state--unresolved {
  background: var(--accent-bg);
  color: var(--accent);
}

.thread-card__state--resolved {
  background: var(--success-bg);
  color: var(--success);
}

.thread-card__state--outdated {
  background: var(--bg-4);
  color: var(--text-faint);
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
</style>
