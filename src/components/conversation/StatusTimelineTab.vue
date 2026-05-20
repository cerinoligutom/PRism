<script setup lang="ts">
import { computed } from "vue";

import type { DashboardPullRequest } from "@/types/dashboard";

import { formatRelativeAgo } from "./_format";

interface Props {
  pullRequest: DashboardPullRequest;
}

const props = defineProps<Props>();

/**
 * v1 reads only the DTO fields already on `DashboardPullRequest`. The M1
 * `timeline_events` table is populated by the sync worker but no Tauri command
 * exposes its rows yet; a follow-up ticket (post-M3) will wire the full event
 * stream into this tab. Today's events: opened, marked ready, current state.
 */

type EventKind = "opened" | "ready" | "approvals" | "changes" | "ci" | "current";

interface TimelineEvent {
  readonly kind: EventKind;
  readonly icon: "circle" | "dot" | "check" | "bang" | "x";
  readonly label: string;
  readonly who: string | null;
  readonly when: string;
  readonly state: "done" | "current";
}

const events = computed<readonly TimelineEvent[]>(() => {
  const pr = props.pullRequest;
  const items: TimelineEvent[] = [];

  items.push({
    kind: "opened",
    icon: "circle",
    label: "Opened",
    who: `@${pr.author_login}`,
    when: formatRelativeAgo(pr.created_at),
    state: "done",
  });

  if (!pr.is_draft) {
    items.push({
      kind: "ready",
      icon: "dot",
      label: "Marked ready",
      who: null,
      when: pr.latest_status_change_at !== null
        ? formatRelativeAgo(pr.latest_status_change_at)
        : formatRelativeAgo(pr.created_at),
      state: "done",
    });
  }

  if (pr.review_decision === "APPROVED") {
    items.push({
      kind: "approvals",
      icon: "check",
      label: "Approved",
      who: null,
      when: formatRelativeAgo(pr.updated_at),
      state: "done",
    });
  } else if (pr.review_decision === "CHANGES_REQUESTED") {
    items.push({
      kind: "changes",
      icon: "bang",
      label: "Changes requested",
      who: null,
      when: formatRelativeAgo(pr.updated_at),
      state: "done",
    });
  }

  if (pr.ci !== null) {
    if (pr.ci.state === "FAILURE" || pr.ci.state === "ERROR") {
      items.push({
        kind: "ci",
        icon: "x",
        label: `CI failed (${pr.ci.passing}/${pr.ci.total} passing)`,
        who: null,
        when: formatRelativeAgo(pr.updated_at),
        state: "done",
      });
    } else if (pr.ci.state === "SUCCESS") {
      items.push({
        kind: "ci",
        icon: "check",
        label: `CI passing (${pr.ci.passing}/${pr.ci.total})`,
        who: null,
        when: formatRelativeAgo(pr.updated_at),
        state: "done",
      });
    }
  }

  // Current state: mergeable / merged / closed / draft / open.
  if (pr.state === "merged") {
    items.push({
      kind: "current",
      icon: "check",
      label: "Merged",
      who: null,
      when: formatRelativeAgo(pr.updated_at),
      state: "current",
    });
  } else if (pr.state === "closed") {
    items.push({
      kind: "current",
      icon: "x",
      label: "Closed",
      who: null,
      when: formatRelativeAgo(pr.updated_at),
      state: "current",
    });
  } else if (pr.is_draft) {
    items.push({
      kind: "current",
      icon: "circle",
      label: "Draft",
      who: null,
      when: formatRelativeAgo(pr.updated_at),
      state: "current",
    });
  } else if (pr.mergeable === "CONFLICTING") {
    items.push({
      kind: "current",
      icon: "bang",
      label: "Conflicts",
      who: null,
      when: formatRelativeAgo(pr.updated_at),
      state: "current",
    });
  } else if (pr.mergeable === "MERGEABLE") {
    items.push({
      kind: "current",
      icon: "dot",
      label: "Mergeable",
      who: null,
      when: formatRelativeAgo(pr.updated_at),
      state: "current",
    });
  }

  // Ensure the last entry is rendered as "current" even if no terminal state
  // qualifier landed above (e.g. unknown mergeable, no CI, no review).
  if (items.length > 0 && items.every((e) => e.state === "done")) {
    const last = items[items.length - 1]!;
    items[items.length - 1] = { ...last, state: "current" };
  }

  return items;
});

function iconChar(kind: TimelineEvent["icon"]): string {
  switch (kind) {
    case "circle":
      return "o";
    case "dot":
      return "*";
    case "check":
      return "v";
    case "bang":
      return "!";
    case "x":
      return "x";
  }
}
</script>

<template>
  <div class="timeline-tab">
    <div class="timeline-tab__list">
      <template v-for="(event, idx) in events" :key="idx">
        <div
          :class="[
            'timeline-row',
            event.state === 'current' && 'timeline-row--current',
            event.state === 'done' && 'timeline-row--done',
          ]"
        >
          <span class="timeline-row__icon" aria-hidden="true">
            <svg
              v-if="event.icon === 'check'"
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
              v-else-if="event.icon === 'x'"
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
              v-else-if="event.icon === 'dot'"
              width="8"
              height="8"
              viewBox="0 0 8 8"
            >
              <circle cx="4" cy="4" r="3" fill="currentColor" />
            </svg>
            <svg
              v-else-if="event.icon === 'bang'"
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
            <span class="sr-only">{{ iconChar(event.icon) }}</span>
          </span>
          <div class="timeline-row__label">
            <span>{{ event.label }}</span>
            <span v-if="event.who !== null" class="timeline-row__who">{{ event.who }}</span>
          </div>
          <div class="timeline-row__when">{{ event.when }}</div>
        </div>
        <div v-if="idx < events.length - 1" class="timeline-tab__rule" aria-hidden="true">
          <div class="timeline-tab__line"></div>
        </div>
      </template>
    </div>
    <p class="timeline-tab__note">
      Showing a summary view. Full event history lands in a follow-up ticket.
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

.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
</style>
