<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";

import type { DashboardPullRequest } from "@/types/dashboard";
import type { TimelineEventRecord } from "@/types/conversation";

import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";

interface Props {
  pullRequest: DashboardPullRequest;
}

const props = defineProps<Props>();

/**
 * The tab reads persisted rows from `timeline_events` via
 * `list_pr_timeline_events`. The sync worker writes the qualifying-event set
 * (ADR 0007) every cycle (wipe-and-rewrite); a freshly-discovered PR whose
 * timeline hasn't synced yet falls back to a synthesised view built from
 * `DashboardPullRequest` so the tab never renders blank.
 */

type Icon = "circle" | "dot" | "check" | "bang" | "x";

interface TimelineRow {
  readonly key: string;
  readonly icon: Icon;
  readonly label: string;
  readonly who: string | null;
  /** Bare GitHub login for the avatar lookup (no leading `@`). */
  readonly whoLogin: string | null;
  readonly whoAvatarUrl: string | null;
  /** Unix seconds for the event; rendered via `PRismRelativeTime`. */
  readonly when: number;
  readonly state: "done" | "current";
}

const events = ref<readonly TimelineEventRecord[]>([]);
const isLoading = ref(false);
const loadError = ref<string | null>(null);

async function load(prId: number): Promise<void> {
  isLoading.value = true;
  loadError.value = null;
  try {
    const result = await invoke<TimelineEventRecord[]>("list_pr_timeline_events", {
      pullRequestId: prId,
    });
    events.value = result;
  } catch (err) {
    loadError.value = formatError(err);
    events.value = [];
  } finally {
    isLoading.value = false;
  }
}

function formatError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "Couldn't load timeline.";
}

onMounted(() => {
  void load(props.pullRequest.id);
});

watch(
  () => props.pullRequest.id,
  (next) => {
    void load(next);
  },
);

/**
 * Map a persisted GitHub timeline event to its visible row. The label mirrors
 * what `synthesisedRows` produces for the equivalent state, so a PR with a
 * fully-populated `timeline_events` table renders the same shape as a PR
 * whose row falls back to the dashboard-DTO heuristic.
 */
function persistedRow(event: TimelineEventRecord, index: number): TimelineRow {
  const when = event.created_at;
  const who = event.actor_login !== null ? `@${event.actor_login}` : null;
  const whoLogin = event.actor_login;
  const whoAvatarUrl = event.actor_avatar_url;
  const key = `${event.event_type}-${event.created_at}-${index}`;
  const base = { key, who, whoLogin, whoAvatarUrl, when, state: "done" as const };
  switch (event.event_type) {
    case "ready_for_review":
      return { ...base, icon: "dot", label: "Marked ready" };
    case "convert_to_draft":
      return { ...base, icon: "circle", label: "Converted to draft" };
    case "review_requested":
      return { ...base, icon: "dot", label: "Review requested" };
    case "reviewed":
      return {
        ...base,
        icon: reviewIcon(event.review_state),
        label: reviewLabel(event.review_state),
      };
    case "merged":
      return { ...base, icon: "check", label: "Merged" };
    case "closed":
      return { ...base, icon: "x", label: "Closed" };
    case "reopened":
      return { ...base, icon: "circle", label: "Reopened" };
    default:
      return { ...base, icon: "circle", label: event.event_type };
  }
}

function reviewIcon(state: string | null): Icon {
  switch (state) {
    case "APPROVED":
      return "check";
    case "CHANGES_REQUESTED":
      return "bang";
    case "DISMISSED":
      return "x";
    default:
      return "dot";
  }
}

function reviewLabel(state: string | null): string {
  switch (state) {
    case "APPROVED":
      return "Approved";
    case "CHANGES_REQUESTED":
      return "Changes requested";
    case "DISMISSED":
      return "Review dismissed";
    case "COMMENTED":
      return "Reviewed";
    default:
      return "Reviewed";
  }
}

/**
 * Synthesise a timeline from the dashboard DTO. Used as a fallback when the
 * persisted `timeline_events` table is empty (e.g. a PR discovered this cycle
 * whose enrichment hasn't landed yet).
 */
function synthesisedRows(): TimelineRow[] {
  const pr = props.pullRequest;
  const rows: TimelineRow[] = [];

  rows.push({
    key: "opened",
    icon: "circle",
    label: "Opened",
    who: `@${pr.author_login}`,
    whoLogin: pr.author_login,
    whoAvatarUrl: pr.author_avatar_url,
    when: pr.created_at,
    state: "done",
  });

  if (!pr.is_draft) {
    rows.push({
      key: "ready",
      icon: "dot",
      label: "Marked ready",
      who: null,
      whoLogin: null,
      whoAvatarUrl: null,
      when: pr.latest_status_change_at ?? pr.created_at,
      state: "done",
    });
  }

  if (pr.review_decision === "APPROVED") {
    rows.push({
      key: "approved",
      icon: "check",
      label: "Approved",
      who: null,
      whoLogin: null,
      whoAvatarUrl: null,
      when: pr.updated_at,
      state: "done",
    });
  } else if (pr.review_decision === "CHANGES_REQUESTED") {
    rows.push({
      key: "changes",
      icon: "bang",
      label: "Changes requested",
      who: null,
      whoLogin: null,
      whoAvatarUrl: null,
      when: pr.updated_at,
      state: "done",
    });
  }

  if (pr.ci !== null) {
    if (pr.ci.state === "FAILURE" || pr.ci.state === "ERROR") {
      rows.push({
        key: "ci",
        icon: "x",
        label: `CI failed (${pr.ci.passing}/${pr.ci.total} passing)`,
        who: null,
        whoLogin: null,
        whoAvatarUrl: null,
        when: pr.updated_at,
        state: "done",
      });
    } else if (pr.ci.state === "SUCCESS") {
      rows.push({
        key: "ci",
        icon: "check",
        label: `CI passing (${pr.ci.passing}/${pr.ci.total})`,
        who: null,
        whoLogin: null,
        whoAvatarUrl: null,
        when: pr.updated_at,
        state: "done",
      });
    }
  }

  if (pr.state === "merged") {
    rows.push({
      key: "current",
      icon: "check",
      label: "Merged",
      who: null,
      whoLogin: null,
      whoAvatarUrl: null,
      when: pr.updated_at,
      state: "current",
    });
  } else if (pr.state === "closed") {
    rows.push({
      key: "current",
      icon: "x",
      label: "Closed",
      who: null,
      whoLogin: null,
      whoAvatarUrl: null,
      when: pr.updated_at,
      state: "current",
    });
  } else if (pr.is_draft) {
    rows.push({
      key: "current",
      icon: "circle",
      label: "Draft",
      who: null,
      whoLogin: null,
      whoAvatarUrl: null,
      when: pr.updated_at,
      state: "current",
    });
  } else if (pr.mergeable === "CONFLICTING") {
    rows.push({
      key: "current",
      icon: "bang",
      label: "Conflicts",
      who: null,
      whoLogin: null,
      whoAvatarUrl: null,
      when: pr.updated_at,
      state: "current",
    });
  } else if (pr.mergeable === "MERGEABLE") {
    rows.push({
      key: "current",
      icon: "dot",
      label: "Mergeable",
      who: null,
      whoLogin: null,
      whoAvatarUrl: null,
      when: pr.updated_at,
      state: "current",
    });
  }

  if (rows.length > 0 && rows.every((r) => r.state === "done")) {
    const last = rows[rows.length - 1]!;
    rows[rows.length - 1] = { ...last, state: "current" };
  }
  return rows;
}

const rows = computed<readonly TimelineRow[]>(() => {
  if (events.value.length === 0) {
    return synthesisedRows();
  }

  // Opened row is synthesised from the PR DTO because GitHub's timeline events
  // start with the first non-creation event; the dashboard row already carries
  // `created_at` and `author_login` and we want the user to see the same first
  // row regardless of persistence state.
  const pr = props.pullRequest;
  const opened: TimelineRow = {
    key: "opened",
    icon: "circle",
    label: "Opened",
    who: `@${pr.author_login}`,
    whoLogin: pr.author_login,
    whoAvatarUrl: pr.author_avatar_url,
    when: pr.created_at,
    state: "done",
  };
  const persisted: TimelineRow[] = events.value.map((e, i) => persistedRow(e, i));
  const combined = [opened, ...persisted];

  if (combined.length > 0) {
    const last = combined[combined.length - 1]!;
    combined[combined.length - 1] = { ...last, state: "current" };
  }
  return combined;
});

const showFallbackNote = computed(() => events.value.length === 0 && loadError.value === null);
</script>

<template>
  <div class="timeline-tab">
    <div v-if="isLoading && rows.length === 0" class="timeline-tab__loading" aria-busy="true">
      <span class="dot dot-pulse" aria-hidden="true"></span>
      <span>Loading timeline…</span>
    </div>
    <div v-else-if="loadError !== null" class="timeline-tab__error" role="alert">
      {{ loadError }}
    </div>
    <div v-else class="timeline-tab__list">
      <template v-for="(row, idx) in rows" :key="row.key">
        <div
          :class="[
            'timeline-row',
            row.state === 'current' && 'timeline-row--current',
            row.state === 'done' && 'timeline-row--done',
          ]"
        >
          <span class="timeline-row__icon" aria-hidden="true">
            <svg
              v-if="row.icon === 'check'"
              width="10"
              height="10"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="2.4"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <path d="M3 8.5l3 3 7-7" />
            </svg>
            <svg
              v-else-if="row.icon === 'x'"
              width="10"
              height="10"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="2.2"
              stroke-linecap="round"
            >
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
            <svg
              v-else-if="row.icon === 'dot'"
              width="8"
              height="8"
              viewBox="0 0 8 8"
            >
              <circle cx="4" cy="4" r="3" fill="currentColor" />
            </svg>
            <svg
              v-else-if="row.icon === 'bang'"
              width="10"
              height="10"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
            >
              <path d="M8 4v5M8 11.5v.5" />
            </svg>
            <svg
              v-else
              width="10"
              height="10"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
            >
              <circle cx="8" cy="8" r="5" />
            </svg>
          </span>
          <div class="timeline-row__label">
            <span>{{ row.label }}</span>
            <span v-if="row.who !== null" class="timeline-row__who-block">
              <PRismAvatar
                v-if="row.whoLogin !== null"
                :login="row.whoLogin"
                :avatar-url="row.whoAvatarUrl"
                size="sm"
                class="timeline-row__avatar"
              />
              <span class="timeline-row__who">{{ row.who }}</span>
            </span>
          </div>
          <PRismRelativeTime :value="row.when" class="timeline-row__when" />
        </div>
        <div v-if="idx < rows.length - 1" class="timeline-tab__rule" aria-hidden="true">
          <div class="timeline-tab__line"></div>
        </div>
      </template>
    </div>
    <p v-if="showFallbackNote" class="timeline-tab__note">
      Showing a summary view until the first sync cycle lands the full event history.
    </p>
  </div>
</template>

<style scoped>
.timeline-tab {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

.timeline-tab__list {
  display: flex;
  flex-direction: column;
}

.timeline-tab__loading,
.timeline-tab__error {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  color: var(--text-mute);
  font-size: var(--fs-12);
  padding: var(--s-3) 0;
}

.timeline-tab__error {
  color: var(--danger);
}

.timeline-row {
  display: grid;
  grid-template-columns: 16px 1fr auto;
  gap: var(--s-3);
  align-items: center;
  padding: 6px 0;
  font-size: var(--fs-12);
}

.timeline-row__icon {
  width: 16px;
  height: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  background: var(--bg-3);
  border: 1.5px solid var(--border-2);
  color: var(--text-faint);
}

.timeline-row--done .timeline-row__icon {
  background: var(--bg-4);
  border-color: var(--border-2);
  color: var(--text-mute);
}

.timeline-row--current .timeline-row__icon {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--accent-fg);
}

.timeline-row__label {
  color: var(--text);
  display: flex;
  align-items: center;
  gap: 6px;
}

.timeline-row__who {
  color: var(--text-faint);
  font-family: var(--font-mono);
  font-size: var(--fs-10);
}

.timeline-row__who-block {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.timeline-row__when {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  text-align: right;
}

.timeline-row--current .timeline-row__when {
  color: var(--accent);
}

.timeline-tab__rule {
  margin-left: 7px;
}

.timeline-tab__line {
  width: 1.5px;
  height: 12px;
  background: var(--border-2);
}

.timeline-tab__note {
  margin: var(--s-2) 0 0;
  font-size: var(--fs-10);
  color: var(--text-faint);
  font-style: italic;
}
</style>
