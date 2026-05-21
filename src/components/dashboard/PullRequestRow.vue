<script setup lang="ts">
import { computed } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuPortal,
  DropdownMenuRoot,
  DropdownMenuTrigger,
} from "reka-ui";
import type {
  AccountMarker,
  DashboardPullRequest,
  RowDensity,
} from "@/types/dashboard";
import { secondsSince } from "@/lib/format";
import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismAvatarStack from "@/components/ui/PRismAvatarStack.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
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
  /**
   * Lookup from account id to a render-ready marker. Shared across rows so
   * the dashboard doesn't allocate a per-row computed; the row picks its
   * own subset via `pullRequest.account_ids`. See ADR 0016 ("Dashboard row
   * shape - option 1") for the merged-row contract.
   */
  accountsById?: ReadonlyMap<number, AccountMarker>;
  /**
   * True when the dashboard is scoped to a single account. In that mode the
   * marker is hidden entirely - every row's `account_ids` collapses to one,
   * which the picker already names. Unified mode renders the marker (single
   * muted dot for a single relation, stack for multi).
   */
  singleAccountScope?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  density: "comfortable",
  unread: false,
  needsAttention: false,
  accountsById: () => new Map<number, AccountMarker>(),
  singleAccountScope: false,
});

const emit = defineEmits<{
  open: [pullRequest: DashboardPullRequest];
  /** M4 row action — viewer asked to flip this PR back to unread. The parent
   * invokes `mark_pr_unread` and reloads; the dot returns on the next paint. */
  "mark-unread": [pullRequest: DashboardPullRequest];
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

const stripTooltip = computed<string>(() => {
  switch (stripClass.value) {
    case "row-strip-draft":
      return "Draft";
    case "row-strip-changes":
      return "Changes requested";
    case "row-strip-stale":
      return "Stale (no activity 7d+)";
    case "row-strip-needs":
      return "Needs your review";
    case "row-strip-approved":
      return "Approved";
    case "row-strip-none":
    default:
      return "";
  }
});

const branchLabel = computed<string>(() => props.pullRequest.head_ref);

const linesAdditions = computed<string | null>(() =>
  props.pullRequest.additions === null
    ? null
    : formatNumber(props.pullRequest.additions),
);

const linesDeletions = computed<string | null>(() =>
  props.pullRequest.deletions === null
    ? null
    : formatNumber(props.pullRequest.deletions),
);

const changedFiles = computed<number | null>(
  () => props.pullRequest.changed_files,
);

const sinceLabel = computed<string>(() => sinceLabelFor(props.pullRequest));

const isStale = computed<boolean>(
  () => secondsSince(props.pullRequest.updated_at) > STALE_THRESHOLD_SECONDS,
);

/**
 * Accounts with a relation to this PR, resolved to render-ready markers.
 * Ids that don't resolve in the lookup are skipped - typically a transient
 * race when an account was just removed and the dashboard hasn't reloaded.
 */
const accountMarkers = computed<readonly AccountMarker[]>(() => {
  const ids = props.pullRequest.account_ids;
  const lookup = props.accountsById;
  const out: AccountMarker[] = [];
  for (const id of ids) {
    const entry = lookup.get(id);
    if (entry !== undefined) out.push(entry);
  }
  return out;
});

const accountStackUsers = computed<readonly { login: string; avatar_url: string | null }[]>(
  () => accountMarkers.value.map((a) => ({ login: a.login, avatar_url: a.avatar_url })),
);

/**
 * Hide the marker when the dashboard is scoped to one account - the picker
 * already names the scope, so every row's marker would be redundant. Unified
 * mode always renders, even single-relation rows (one muted dot so the user
 * can read "this only matches via Work" without scanning).
 */
const showAccountMarker = computed<boolean>(
  () => !props.singleAccountScope && accountMarkers.value.length > 0,
);

const isSingleRelation = computed<boolean>(() => accountMarkers.value.length === 1);

const accountTooltipText = computed<string>(() => {
  if (accountMarkers.value.length === 0) return "";
  const labels = accountMarkers.value.map((a) => a.label || a.login);
  return `Visible from ${labels.join(", ")}`;
});

function formatNumber(value: number): string {
  return value.toLocaleString("en-AU");
}

function sinceLabelFor(pr: DashboardPullRequest): string {
  if (pr.is_draft) return "opened";
  if (pr.mergeable === "CONFLICTING") return "conflicts";
  if (secondsSince(pr.updated_at) > STALE_THRESHOLD_SECONDS) return "stale";
  if (pr.ci?.state === "FAILURE" || pr.ci?.state === "ERROR")
    return "CI failed";
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

function openOnGitHub(event: MouseEvent): void {
  event.stopPropagation();
  void openUrl(props.pullRequest.url);
}

function onMarkUnread(): void {
  emit("mark-unread", props.pullRequest);
}
</script>

<template>
  <article
    :class="[
      'pr-row',
      `pr-row--${density}`,
      needsAttention && 'pr-row--attention',
      unread && 'pr-row--unread',
    ]"
    role="button"
    tabindex="0"
    @click="onClick"
    @keydown="onKey"
  >
    <span
      class="pr-row__dot"
      :aria-label="unread ? 'Unread' : undefined"
      :aria-hidden="unread ? undefined : 'true'"
    ></span>

    <PRismTooltip
      :text="stripTooltip"
      :disabled="stripClass === 'row-strip-none'"
      side="right"
      as-child
    >
      <div
        :class="['pr-row__state', stripClass]"
        :aria-label="stripTooltip || undefined"
        :aria-hidden="stripClass === 'row-strip-none' ? 'true' : undefined"
      >
        <svg
          v-if="stripClass === 'row-strip-needs'"
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M1.5 8s2.5-5 6.5-5 6.5 5 6.5 5-2.5 5-6.5 5S1.5 8 1.5 8Z" />
          <circle cx="8" cy="8" r="2" />
        </svg>
        <svg
          v-else-if="stripClass === 'row-strip-changes'"
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <circle cx="8" cy="8" r="6.25" />
          <path d="M5.75 5.75l4.5 4.5M10.25 5.75l-4.5 4.5" />
        </svg>
        <svg
          v-else-if="stripClass === 'row-strip-approved'"
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <circle cx="8" cy="8" r="6.25" />
          <path d="M5.25 8.25l2 2 3.5-4" />
        </svg>
        <svg
          v-else-if="stripClass === 'row-strip-draft'"
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M11.5 2.5l2 2-8 8H3.5v-2l8-8Z" />
          <path d="M10 4l2 2" />
        </svg>
        <svg
          v-else-if="stripClass === 'row-strip-stale'"
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.5"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <circle cx="8" cy="8" r="6.25" />
          <path d="M8 4.5V8l2.25 1.5" />
        </svg>
      </div>
    </PRismTooltip>

    <div class="pr-row__num">#{{ pullRequest.number }}</div>

    <div class="pr-row__title-col">
      <div class="pr-row__title-row">
        <PRismTooltip :text="pullRequest.title" :as-child="true">
          <span class="pr-row__title">{{ pullRequest.title }}</span>
        </PRismTooltip>
        <MergeableBadge
          :state="pullRequest.mergeable"
          :review-decision="pullRequest.review_decision"
          :is-draft="pullRequest.is_draft"
        />
        <PRismTooltip
          v-if="showAccountMarker"
          :text="accountTooltipText"
          :as-child="true"
        >
          <span
            :class="[
              'pr-row__accounts',
              isSingleRelation && 'pr-row__accounts--single',
            ]"
            :aria-label="accountTooltipText"
          >
            <PRismAvatarStack
              :users="accountStackUsers"
              :max="2"
              size="sm"
              layout="overlap"
            />
          </span>
        </PRismTooltip>
      </div>
      <div class="pr-row__meta-row">
        <PRismTooltip
          :text="`${pullRequest.base_ref} ← ${pullRequest.head_ref}`"
          :as-child="true"
        >
          <span class="pr-row__branch">
          <svg
            width="9"
            height="9"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.5"
            aria-hidden="true"
          >
            <circle cx="4" cy="3" r="1.5" />
            <circle cx="4" cy="13" r="1.5" />
            <circle cx="12" cy="6" r="1.5" />
            <path
              d="M4 4.5v7M4 8a4 4 0 004 4h0a4 4 0 004-4V7.5"
              stroke-linecap="round"
            />
          </svg>
          <span class="pr-row__branch-name">{{ branchLabel }}</span>
          </span>
        </PRismTooltip>
        <span class="pr-row__sep" aria-hidden="true">·</span>
        <span class="pr-row__author">
          <PRismAvatar
            :login="pullRequest.author_login"
            :avatar-url="pullRequest.author_avatar_url"
            size="sm"
            class="pr-row__author-avatar"
          />
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
      <PRismRelativeTime
        :value="pullRequest.updated_at"
        class="pr-row__time-value"
      />
      <span class="pr-row__time-since">{{ sinceLabel }}</span>
    </div>

    <PRismTooltip text="Open on GitHub" :as-child="true">
      <button
        type="button"
        class="pr-row__github"
        aria-label="Open on GitHub"
        @click="openOnGitHub"
        @keydown.stop
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="currentColor"
          aria-hidden="true"
        >
          <path
            fill-rule="evenodd"
            d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8Z"
          />
        </svg>
      </button>
    </PRismTooltip>

    <DropdownMenuRoot>
      <DropdownMenuTrigger as-child>
        <button
          type="button"
          class="pr-row__kebab"
          aria-label="Pull request actions"
          @click.stop
          @keydown.stop
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 16 16"
            fill="currentColor"
            aria-hidden="true"
          >
            <circle cx="3" cy="8" r="1.4" />
            <circle cx="8" cy="8" r="1.4" />
            <circle cx="13" cy="8" r="1.4" />
          </svg>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuPortal>
        <DropdownMenuContent
          class="pr-row__menu"
          align="end"
          :side-offset="4"
          @click.stop
        >
          <DropdownMenuItem
            class="pr-row__menu-item"
            :disabled="unread"
            @select="onMarkUnread"
          >
            Mark unread
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>
  </article>
</template>

<style scoped>
.pr-row {
  position: relative;
  /* Columns: [dot] [state icon + edge] [#num] [title] [threads] [reviewers] */
  /* [ci] [time] [github] [kebab] */
  display: grid;
  grid-template-columns: 10px 22px 54px 1fr 144px 180px 80px 80px 24px 28px;
  align-items: center;
  gap: 14px;
  padding: 0 var(--s-6) 0 var(--s-3);
  height: var(--row-h-comfortable);
  border-top: 1px solid var(--border-1);
  background: var(--bg-1);
  cursor: pointer;
  transition: background 0.12s;
}

.pr-row:hover {
  background: var(--bg-2);
}

.pr-row:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
}

.pr-row--tight {
  height: var(--row-h-tight);
}
.pr-row--comfortable {
  height: var(--row-h-comfortable);
}
.pr-row--roomy {
  height: var(--row-h-roomy);
}

.pr-row--attention {
  background: var(--attention-tint);
}

.pr-row--attention:hover {
  background: var(--attention-tint-hover);
}

/* Leftmost unread dot. Sits in its own grid cell so read / unread rows align
 * identically; only its background switches between transparent and the
 * accent token. Centred inside the 10px column. */
.pr-row__dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: transparent;
  justify-self: center;
}

.pr-row--unread .pr-row__dot {
  background: var(--accent-strong);
}

/* State badge. A 22px tinted-square pill with a centred 14px Lucide-style svg.
 * The tinted background carries the colour-coded scan signal; the icon glyph
 * disambiguates on closer look. Same pattern as `.thread-card__state`. */
.pr-row__state {
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--r-1);
  color: var(--text-faint);
}

.pr-row__state svg {
  width: 14px;
  height: 14px;
}

.pr-row__state.row-strip-none {
  background: transparent;
}

.pr-row__state.row-strip-needs {
  color: var(--info);
  background: oklch(from var(--info) l c h / 0.18);
}

.pr-row__state.row-strip-changes {
  color: var(--danger);
  background: oklch(from var(--danger) l c h / 0.18);
}

.pr-row__state.row-strip-approved {
  color: var(--success);
  background: oklch(from var(--success) l c h / 0.18);
}

.pr-row__state.row-strip-draft {
  color: var(--text-mute);
  background: oklch(from var(--text-mute) l c h / 0.18);
}

.pr-row__state.row-strip-stale {
  color: var(--warning);
  background: oklch(from var(--warning) l c h / 0.18);
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

/* Account marker. Sits at the right of the title row when the dashboard is
 * in unified scope, hinting at which account(s) saw the PR. Pushed to the
 * trailing edge via `auto` margin so it stays clear of the title ellipsis.
 * Single-relation rows render at reduced opacity so the marker reads as a
 * scope hint without competing with the title or the reviewer stack. */
.pr-row__accounts {
  margin-left: auto;
  display: inline-flex;
  align-items: center;
  flex: 0 0 auto;
}

.pr-row__accounts--single {
  opacity: 0.55;
}

.pr-row:hover .pr-row__accounts--single {
  opacity: 0.8;
}

/* Unread rows lean on the title weight as the primary signal; the left-edge
 * dot is the secondary confirmation. */
.pr-row--unread .pr-row__title {
  font-weight: 600;
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

.pr-row__lines-add {
  color: var(--success);
}
.pr-row__lines-del {
  color: var(--danger);
}
.pr-row__lines-files {
  color: var(--text-faint);
  text-wrap: nowrap;
}

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

.pr-row__github {
  color: var(--text-faint);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--r-2);
  background: transparent;
  border: 0;
  padding: 0;
  cursor: pointer;
  transition:
    color 0.12s,
    background 0.12s;
}

.pr-row:hover .pr-row__github {
  color: var(--text-mute);
}

.pr-row__github:hover {
  background: var(--bg-3);
  color: var(--text);
}

.pr-row__github:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
  color: var(--text);
}

.pr-row__kebab {
  color: var(--text-faint);
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border-radius: var(--r-2);
  background: transparent;
  border: 0;
  padding: 0;
  cursor: pointer;
  transition:
    color 0.12s,
    background 0.12s;
}

.pr-row:hover .pr-row__kebab {
  color: var(--text-mute);
}

.pr-row__kebab:hover {
  background: var(--bg-3);
  color: var(--text);
}

.pr-row__kebab:focus-visible {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
  color: var(--text);
}

.pr-row__kebab[data-state="open"] {
  background: var(--bg-3);
  color: var(--text);
}
</style>

<!--
  Dropdown menu content is teleported to `document.body` via Reka's
  `DropdownMenuPortal`. Scoped styles can't follow the teleport, so the menu
  rules live in an unscoped block alongside the row. Matches the same pattern
  used by `FilterChipsBar.vue`'s tooltip body and `ReviewerStack.vue`'s
  overflow row.
-->
<style>
.pr-row__menu {
  min-width: 160px;
  background: var(--bg-2);
  border: 1px solid var(--border-2);
  border-radius: var(--r-2);
  padding: 4px;
  box-shadow: var(--shadow-2);
  z-index: 50;
}

.pr-row__menu-item {
  display: flex;
  align-items: center;
  height: 28px;
  padding: 0 10px;
  font-size: var(--fs-12);
  color: var(--text);
  border-radius: var(--r-1);
  cursor: pointer;
  user-select: none;
  outline: none;
}

.pr-row__menu-item[data-highlighted] {
  background: var(--bg-4);
  color: var(--text-strong);
}

.pr-row__menu-item[data-disabled] {
  color: var(--text-disabled);
  cursor: not-allowed;
  pointer-events: none;
}
</style>
