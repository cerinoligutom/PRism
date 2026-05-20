<script setup lang="ts">
import { computed } from "vue";
import type { DashboardPullRequest, RowDensity } from "@/types/dashboard";
import { avatarSeed, formatRelativeAgo, initials, secondsSince } from "@/lib/format";
import ReviewerStack from "./ReviewerStack.vue";
import CiBadge from "./CiBadge.vue";
import MergeableBadge from "./MergeableBadge.vue";
import ThreadsBar from "./ThreadsBar.vue";

interface Props {
  pullRequest: DashboardPullRequest;
  /** Row vertical density. Default `comfortable`. */
  density?: RowDensity;
  /** M4 slot — unread dot on the title. Safe to leave undefined in M2. */
  unread?: boolean;
  /** M4 slot — accent tint highlighting rows needing the viewer. */
  needsAttention?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  density: "comfortable",
  unread: false,
  needsAttention: false,
});

const emit = defineEmits<{
  open: [pullRequest: DashboardPullRequest];
}>();

type RowStrip =
  | "row-strip-needs"
  | "row-strip-changes"
  | "row-strip-approved"
  | "row-strip-draft"
  | "row-strip-stale"
  | "row-strip-none";

const STALE_THRESHOLD_SECONDS = 7 * 24 * 60 * 60;

/**
 * Left strip colour. Mirrors the priority order in the artboard:
 *   draft > changes-requested > stale (no activity 7d+) > needs-review (you)
 *     > approved > none.
 *
 * Stale is detected from `updated_at` because M2 doesn't track per-account
 * read state yet; M4 will refine `needs-attention` and recompute the strip.
 */
const stripClass = computed<RowStrip>(() => {
  const pr = props.pullRequest;
  if (pr.is_draft) return "row-strip-draft";
  if (pr.review_decision === "CHANGES_REQUESTED") return "row-strip-changes";

  const updatedSeconds = Math.floor(Date.now() / 1000) - pr.updated_at;
  if (updatedSeconds > STALE_THRESHOLD_SECONDS) return "row-strip-stale";

  if (props.needsAttention) return "row-strip-needs";
  const youPending = pr.reviewers.some(
    (r) => r.is_you && r.state === "pending",
  );
  if (youPending) return "row-strip-needs";

  if (pr.review_decision === "APPROVED") return "row-strip-approved";
  return "row-strip-none";
});

const branchLabel = computed<string>(() => props.pullRequest.head_ref);

const linesAdditions = computed<string | null>(() =>
  props.pullRequest.additions === null ? null : formatNumber(props.pullRequest.additions),
);

const linesDeletions = computed<string | null>(() =>
  props.pullRequest.deletions === null ? null : formatNumber(props.pullRequest.deletions),
);

const changedFiles = computed<number | null>(
  () => props.pullRequest.changed_files,
);

const updatedRelative = computed<string>(() =>
  formatRelativeAgo(props.pullRequest.updated_at),
);

const sinceLabel = computed<string>(() => sinceLabelFor(props.pullRequest));

const isStale = computed<boolean>(
  () => secondsSince(props.pullRequest.updated_at) > STALE_THRESHOLD_SECONDS,
);

function formatNumber(value: number): string {
  return value.toLocaleString("en-AU");
}

function sinceLabelFor(pr: DashboardPullRequest): string {
  if (pr.is_draft) return "opened";
  if (pr.mergeable === "CONFLICTING") return "conflicts";
  if (secondsSince(pr.updated_at) > STALE_THRESHOLD_SECONDS) return "stale";
  if (pr.ci?.state === "FAILURE" || pr.ci?.state === "ERROR") return "CI failed";
  if (pr.review_decision === "CHANGES_REQUESTED") return "changes";
  if (pr.review_decision === "APPROVED") return "approved";
  return "updated";
}

function onClick(): void {
  emit("open", props.pullRequest);
}

function onKey(event: KeyboardEvent): void {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    emit("open", props.pullRequest);
  }
}
</script>

<template>
  <div
    :class="[
      'pr-row',
      `pr-row--${density}`,
      needsAttention && 'pr-row--attention',
      unread && 'pr-row--unread',
    ]"
    role="button"
    tabindex="0"
    :title="pullRequest.title"
    @click="onClick"
    @keydown="onKey"
  >
    <div :class="['pr-row__strip', stripClass]" aria-hidden="true"></div>

    <div class="pr-row__num">#{{ pullRequest.number }}</div>

    <div class="pr-row__title-col">
      <div class="pr-row__title-row">
        <span class="pr-row__title">{{ pullRequest.title }}</span>
        <MergeableBadge
          :state="pullRequest.mergeable"
          :review-decision="pullRequest.review_decision"
          :is-draft="pullRequest.is_draft"
        />
      </div>
      <div class="pr-row__meta-row">
        <span class="pr-row__branch" :title="`${pullRequest.base_ref} ← ${pullRequest.head_ref}`">
          <svg width="9" height="9" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
            <circle cx="4" cy="3" r="1.5" />
            <circle cx="4" cy="13" r="1.5" />
            <circle cx="12" cy="6" r="1.5" />
            <path d="M4 4.5v7M4 8a4 4 0 004 4h0a4 4 0 004-4V7.5" stroke-linecap="round" />
          </svg>
          <span class="pr-row__branch-name">{{ branchLabel }}</span>
        </span>
        <span class="pr-row__sep" aria-hidden="true">·</span>
        <span class="pr-row__author">
          <span :class="['avatar', 'sm', avatarSeed(pullRequest.author_login), 'pr-row__author-avatar']">
            {{ initials(pullRequest.author_login) }}
          </span>
          {{ pullRequest.author_login }}
        </span>
        <template v-if="linesAdditions !== null && linesDeletions !== null">
          <span class="pr-row__sep" aria-hidden="true">·</span>
          <span class="pr-row__lines">
            <span class="pr-row__lines-add">+{{ linesAdditions }}</span>
            <span class="pr-row__lines-del">&minus;{{ linesDeletions }}</span>
            <span v-if="changedFiles !== null" class="pr-row__lines-files">
              · {{ changedFiles }} {{ changedFiles === 1 ? "file" : "files" }}
            </span>
          </span>
        </template>
      </div>
    </div>

    <div class="pr-row__threads">
      <ThreadsBar :threads="pullRequest.threads" />
    </div>

    <div class="pr-row__reviewers">
      <ReviewerStack :reviewers="pullRequest.reviewers" />
    </div>

    <div class="pr-row__ci">
      <CiBadge :ci="pullRequest.ci" />
    </div>

    <div :class="['pr-row__time', isStale && 'pr-row__time--stale']">
      <span class="pr-row__time-value">{{ updatedRelative }}</span>
      <span class="pr-row__time-since">{{ sinceLabel }}</span>
    </div>

    <div class="pr-row__kebab" aria-hidden="true">⋯</div>
  </div>
</template>

<style scoped>
.pr-row {
  position: relative;
  display: grid;
  grid-template-columns: 4px 54px 1fr 144px 180px 80px 80px 28px;
  align-items: center;
  gap: 14px;
  padding: 0 var(--s-6) 0 0;
  height: var(--row-h-comfortable);
  border-top: 1px solid var(--border-1);
  background: var(--bg-1);
  cursor: pointer;
  transition: background 0.12s;
}

.pr-row:hover { background: var(--bg-2); }

.pr-row:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
}

.pr-row--tight       { height: var(--row-h-tight); }
.pr-row--comfortable { height: var(--row-h-comfortable); }
.pr-row--roomy       { height: var(--row-h-roomy); }

.pr-row--attention {
  background: var(--accent-bg);
}

.pr-row--attention:hover {
  background: var(--accent-bg);
  filter: brightness(1.08);
}

.pr-row__strip {
  width: 3px;
  height: 30px;
  border-radius: 2px;
  margin-left: 1px;
  align-self: center;
}

.pr-row__num {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
  padding-left: var(--s-2);
}

.pr-row__title-col {
  min-width: 0;
}

.pr-row__title-row {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  min-width: 0;
}

.pr-row__title {
  font-size: var(--fs-13);
  font-weight: 500;
  color: var(--text-strong);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  flex: 0 1 auto;
}

.pr-row--unread .pr-row__title::after {
  content: "";
  display: inline-block;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent);
  margin-left: 6px;
  vertical-align: middle;
}

.pr-row__meta-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 2px;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
}

/* Tight density drops the meta row so the row can sit at 36px. */
.pr-row--tight .pr-row__meta-row {
  display: none;
}

.pr-row__branch {
  color: var(--text-mute);
  display: inline-flex;
  align-items: center;
  gap: 3px;
  min-width: 0;
}

.pr-row__branch-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.pr-row__sep {
  color: var(--text-disabled);
}

.pr-row__author {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--text-mute);
}

.pr-row__author-avatar {
  width: 14px;
  height: 14px;
  font-size: 7px;
}

.pr-row__lines {
  display: inline-flex;
  gap: 4px;
}

.pr-row__lines-add { color: var(--success); }
.pr-row__lines-del { color: var(--danger); }
.pr-row__lines-files { color: var(--text-faint); }

.pr-row__threads {
  display: flex;
  align-items: center;
  min-width: 0;
}

.pr-row__reviewers {
  display: flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
}

.pr-row__ci {
  display: flex;
  align-items: center;
  gap: 5px;
}

.pr-row__time {
  text-align: right;
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-mute);
  font-variant-numeric: tabular-nums;
  line-height: var(--lh-tight);
}

.pr-row__time-value {
  display: block;
}

.pr-row__time-since {
  color: var(--text-faint);
  font-size: var(--fs-9);
  letter-spacing: 0.3px;
  display: block;
  margin-top: 1px;
}

.pr-row__time--stale .pr-row__time-value {
  color: var(--warning);
}

.pr-row__kebab {
  color: var(--text-faint);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--r-2);
  cursor: pointer;
}

.pr-row:hover .pr-row__kebab {
  color: var(--text-mute);
}

.pr-row__kebab:hover {
  background: var(--bg-3);
  color: var(--text);
}
</style>
