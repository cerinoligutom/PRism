<script setup lang="ts">
/**
 * "How signals work" reference page (#436, ADR 0031). A always-reachable,
 * fixture-driven explainer composed from the live PR-row and thread-list
 * components rather than copies, so the page can't drift from the real
 * surfaces. Reads every label and explanation from `SIGNAL_COPY`, the same
 * map the row tooltips and the legend popovers use.
 *
 * The page calls no auth- or sync-gated store, so it renders before any
 * account is connected. Section ids (`attention`, `review-state`, `counts`,
 * ...) are the deep-link targets for the dashboard and conversation legends.
 */
import type { MyReviewState } from "@/types/dashboard";
import { SIGNAL_COPY } from "@/components/signals/signalCopy";
import {
  DEMO_ACCOUNTS_BY_ID,
  DEMO_PR,
  DEMO_THREADS,
  DEMO_THREAD_COMMENTS,
} from "@/components/signals/signalsFixtures";

import PullRequestRow from "@/components/dashboard/PullRequestRow.vue";
import ThreadsList from "@/components/conversation/ThreadsList.vue";
import MyReviewStateIcon from "@/components/dashboard/icons/MyReviewStateIcon.vue";
import ThreadStateIcon from "@/components/conversation/icons/ThreadStateIcon.vue";

// The five `my_review_state` glyphs the row can show, in row-precedence order.
// `none` is omitted - it renders a muted dash and reads as "no signal", which
// the guide explains in prose rather than in the swatch list.
const REVIEW_STATES: readonly MyReviewState[] = [
  "author",
  "requested",
  "changes-requested",
  "approved",
  "commented",
];

// The four thread buckets in the order the conversation surface sorts them:
// the one that can need you first.
const THREAD_BUCKETS = [
  { key: "unresolved-involved", shape: "unresolved" },
  { key: "unresolved-uninvolved", shape: "unresolved" },
  { key: "resolved-involved", shape: "resolved" },
  { key: "resolved-uninvolved", shape: "resolved" },
] as const;

// The conversation-unit involvement model, in the order it reads top to
// bottom: what's yours, what lights it, what clears it, the two edge cases.
const MODEL_STEPS = [
  SIGNAL_COPY.model.involvement,
  SIGNAL_COPY.model.lights,
  SIGNAL_COPY.model.clears,
  SIGNAL_COPY.model.resolved,
  SIGNAL_COPY.model.obligations,
] as const;

// The three count surfaces shown agreeing. The illustrative value is shared
// so they read as one roll-up rather than three independent tallies.
const COUNT_VALUE = 3;
const COUNTS = [
  SIGNAL_COPY.count.badge,
  SIGNAL_COPY.count.sidebar,
  SIGNAL_COPY.count.inbox,
] as const;

// The embedded row's `@open` is a no-op here: the page is an explainer, not a
// navigation surface, so clicking the demo row shouldn't route anywhere.
function noop(): void {}
</script>

<template>
  <section class="signals-guide">
    <header class="signals-guide__head">
      <h1 class="signals-guide__title">How signals work</h1>
      <p class="signals-guide__lede">
        PRism points you at the conversations that involve you and the reviews
        you owe, and keeps every count reading from the same place. Here's what
        each mark means and when it clears.
      </p>
    </header>

    <article id="attention" class="card signals-guide__section">
      <h2 class="signals-guide__section-title">Your signals</h2>
      <p class="signals-guide__section-lede">
        Every PR row carries two marks on its left edge: an attention dot and a
        glyph for your own relationship to the review.
      </p>

      <div id="review-state" class="signals-guide__swatch-grid">
        <div class="signals-guide__swatch-row">
          <span class="dashboard-legend__attention-swatch" aria-hidden="true">
            <span class="dashboard-legend__attention-dot"></span>
          </span>
          <div class="signals-guide__swatch-copy">
            <span class="signals-guide__swatch-label">{{ SIGNAL_COPY.attention.label }}</span>
            <span class="signals-guide__swatch-desc">{{ SIGNAL_COPY.attention.description }}</span>
          </div>
        </div>
        <div
          v-for="state in REVIEW_STATES"
          :key="state"
          class="signals-guide__swatch-row"
        >
          <span :class="['pr-row__state', `my-review--${state}`]" aria-hidden="true">
            <MyReviewStateIcon :state="state" />
          </span>
          <div class="signals-guide__swatch-copy">
            <span class="signals-guide__swatch-label">{{ SIGNAL_COPY.myReview[state].label }}</span>
            <span class="signals-guide__swatch-desc">{{ SIGNAL_COPY.myReview[state].description }}</span>
          </div>
        </div>
      </div>

      <h3 id="threads" class="signals-guide__sub-title">Thread badges</h3>
      <p class="signals-guide__section-lede">
        In a PR's conversation, each thread carries a badge for its state.
        Warm colours mean the thread can need you; cool colours mean it's
        others only.
      </p>
      <div class="signals-guide__swatch-grid">
        <div
          v-for="bucket in THREAD_BUCKETS"
          :key="bucket.key"
          class="signals-guide__swatch-row"
        >
          <span
            :class="['thread-card__state', `thread-card__state--${bucket.key}`]"
            aria-hidden="true"
          >
            <ThreadStateIcon :state="bucket.shape" />
          </span>
          <div class="signals-guide__swatch-copy">
            <span class="signals-guide__swatch-label">{{ SIGNAL_COPY.threadBucket[bucket.key].label }}</span>
            <span class="signals-guide__swatch-desc">{{ SIGNAL_COPY.threadBucket[bucket.key].description }}</span>
          </div>
        </div>
      </div>

      <h3 class="signals-guide__sub-title">A real row</h3>
      <p class="signals-guide__section-lede">
        This is the actual dashboard row, fed demo data. It's been asked of you
        for review, a thread you're in moved, and the bold title flags content
        you haven't opened.
      </p>
      <div class="signals-guide__embed signals-guide__embed--row">
        <PullRequestRow
          :pull-request="DEMO_PR"
          :unread="DEMO_PR.unread"
          :needs-attention="DEMO_PR.needs_attention"
          :accounts-by-id="DEMO_ACCOUNTS_BY_ID"
          :single-account-scope="true"
          @open="noop"
        />
      </div>
    </article>

    <article id="notifications" class="card signals-guide__section">
      <h2 class="signals-guide__section-title">How you get notified</h2>
      <p class="signals-guide__section-lede">
        PRism tracks attention per conversation unit - a single review thread,
        or a PR's general comment stream. That's finer than per-PR, so a
        notification can point at the exact thread that moved.
      </p>

      <ol class="signals-guide__model">
        <li
          v-for="step in MODEL_STEPS"
          :key="step.label"
          class="signals-guide__model-step"
        >
          <span class="signals-guide__model-label">{{ step.label }}</span>
          <span class="signals-guide__model-desc">{{ step.description }}</span>
        </li>
      </ol>

      <h3 class="signals-guide__sub-title">A thread that needs you</h3>
      <p class="signals-guide__section-lede">
        The live thread list, fed demo data. The top thread is lit because
        someone replied and mentioned you after your last comment; the second
        is an open thread you aren't in; the third is one you were in that's
        since been resolved.
      </p>
      <div class="signals-guide__embed">
        <ThreadsList
          :threads="DEMO_THREADS"
          :thread-comments="DEMO_THREAD_COMMENTS"
        />
      </div>
    </article>

    <article id="counts" class="card signals-guide__section">
      <h2 class="signals-guide__section-title">The counts</h2>
      <p class="signals-guide__section-lede">
        The dock badge, the sidebar chip, and the inbox all read the same
        roll-up, so they agree by construction. Mark a unit seen and all three
        drop together.
      </p>
      <div class="signals-guide__counts">
        <div
          v-for="count in COUNTS"
          :key="count.label"
          class="signals-guide__count"
        >
          <span class="signals-guide__count-value">{{ COUNT_VALUE }}</span>
          <span class="signals-guide__count-label">{{ count.label }}</span>
          <span class="signals-guide__count-desc">{{ count.description }}</span>
        </div>
      </div>
      <p class="signals-guide__settings-link">
        Choose when toasts fire in
        <RouterLink class="signals-guide__link" :to="{ name: 'settings.notifications' }">
          Notification settings
        </RouterLink>.
      </p>
    </article>
  </section>
</template>

<style scoped>
.signals-guide {
  display: flex;
  flex-direction: column;
  gap: var(--s-5);
  padding: var(--s-5);
  max-width: 880px;
  margin: 0 auto;
  width: 100%;
}

.signals-guide__title {
  margin: 0;
  font-size: var(--fs-20);
  font-weight: 600;
  letter-spacing: -0.01em;
  color: var(--text-strong);
}

.signals-guide__lede {
  margin: var(--s-2) 0 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
  line-height: var(--lh-body);
  max-width: 620px;
}

.signals-guide__section {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  /* Anchored sections sit below the app's scroll container; nudge the scroll
   * target down so a deep-link doesn't bury the heading under the edge. */
  scroll-margin-top: var(--s-5);
}

.signals-guide__section-title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  letter-spacing: -0.2px;
  color: var(--text-strong);
}

.signals-guide__sub-title {
  margin: var(--s-3) 0 0;
  font-size: var(--fs-13);
  font-weight: 600;
  color: var(--text-strong);
  scroll-margin-top: var(--s-5);
}

.signals-guide__section-lede {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
  line-height: var(--lh-body);
  max-width: 620px;
}

.signals-guide__swatch-grid {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

.signals-guide__swatch-row {
  display: flex;
  align-items: flex-start;
  gap: var(--s-3);
}

.signals-guide__swatch-copy {
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 0;
}

.signals-guide__swatch-label {
  font-size: var(--fs-13);
  font-weight: 500;
  color: var(--text-strong);
}

.signals-guide__swatch-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
}

/* Demo embeds. The row needs an explicit border + radius because outside the
 * dashboard table it has no surrounding chrome; the `--row` variant gives it
 * an edge so it reads as a contained sample. */
.signals-guide__embed {
  margin-top: var(--s-2);
}

.signals-guide__embed--row {
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  overflow: hidden;
}

.signals-guide__model {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  counter-reset: model-step;
}

.signals-guide__model-step {
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding-left: var(--s-5);
  position: relative;
}

.signals-guide__model-step::before {
  counter-increment: model-step;
  content: counter(model-step);
  position: absolute;
  left: 0;
  top: 0;
  width: 20px;
  height: 20px;
  border-radius: 50%;
  background: var(--accent-bg);
  color: var(--accent-strong);
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  display: flex;
  align-items: center;
  justify-content: center;
}

.signals-guide__model-label {
  font-size: var(--fs-13);
  font-weight: 500;
  color: var(--text-strong);
}

.signals-guide__model-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
}

.signals-guide__counts {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-3);
}

.signals-guide__count {
  flex: 1 1 180px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: var(--s-4);
  background: var(--bg-1);
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
}

.signals-guide__count-value {
  font-family: var(--font-mono);
  font-size: var(--fs-20);
  font-weight: 600;
  color: var(--accent-strong);
  font-variant-numeric: tabular-nums;
}

.signals-guide__count-label {
  font-size: var(--fs-13);
  font-weight: 500;
  color: var(--text-strong);
}

.signals-guide__count-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
  line-height: var(--lh-body);
}

.signals-guide__settings-link {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
}

.signals-guide__link {
  color: var(--accent);
  text-decoration: none;
}

.signals-guide__link:hover {
  text-decoration: underline;
}
</style>

<!--
  The demo PR row reuses `.pr-row__state` / `.my-review--*` and the demo thread
  badges reuse `.thread-card__state--*`, both defined globally in
  `pr-status.css`, plus the dashboard legend's attention swatch. They're
  referenced from this page's swatch list, which scoped CSS can't reach, so the
  attention-swatch rule is mirrored here in an unscoped block (the same shape
  DashboardView uses for its legend).
-->
<style>
.signals-guide .dashboard-legend__attention-swatch {
  width: 22px;
  height: 22px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}

.signals-guide .dashboard-legend__attention-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent-strong);
}
</style>
