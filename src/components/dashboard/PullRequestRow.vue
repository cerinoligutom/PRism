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
import { useToastStore } from "@/stores/toast";
import { useNowSeconds } from "@/composables/useNowSeconds";
import PRismAvatar from "@/components/ui/PRismAvatar.vue";
import PRismAvatarStack from "@/components/ui/PRismAvatarStack.vue";
import PRismCheckbox from "@/components/ui/PRismCheckbox.vue";
import PRismRelativeTime from "@/components/ui/PRismRelativeTime.vue";
import PRismTooltip from "@/components/ui/PRismTooltip.vue";
import { SIGNAL_COPY } from "@/components/signals/signalCopy";
import ReviewerStack from "./ReviewerStack.vue";
import CiBadge from "./CiBadge.vue";
import MergeableBadge from "./MergeableBadge.vue";
import ThreadsBar from "./ThreadsBar.vue";
import MyReviewStateIcon from "./icons/MyReviewStateIcon.vue";

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
  /**
   * True when the row is rendered inside the Archive view (ADR 0018). Flips
   * the overflow menu's archive entry from "Archive" to "Unarchive" - the
   * `account_ids` for an Archive-view row are the archived relation owners,
   * which an unarchive write clears. Defaults to false so default-view rows
   * keep the original Archive affordance.
   */
  isArchiveView?: boolean;
  /**
   * True when the row is the current target for keyboard-shortcut actions
   * (e.g. `E` to archive). Renders a subtle outline so the user can see
   * which row a hotkey will operate on. Independent of the browser's
   * `:focus-visible` ring on the underlying `<article role="button">`.
   */
  focused?: boolean;
  /** True when the row's bulk-archive checkbox is ticked (#331). */
  selected?: boolean;
  /**
   * True when any row in the active list is selected (#331). The checkbox
   * cell stays visible across every row while non-zero so the user can
   * extend the selection without hunting hover targets.
   */
  selectionActive?: boolean;
}

const props = withDefaults(defineProps<Props>(), {
  density: "comfortable",
  unread: false,
  needsAttention: false,
  accountsById: () => new Map<number, AccountMarker>(),
  singleAccountScope: false,
  isArchiveView: false,
  focused: false,
  selected: false,
  selectionActive: false,
});

const emit = defineEmits<{
  open: [pullRequest: DashboardPullRequest];
  /** M4 row action — viewer asked to flip this PR back to unread. The parent
   * invokes `mark_pr_unread` and reloads; the dot returns on the next paint. */
  "mark-unread": [pullRequest: DashboardPullRequest];
  /** ADR 0018 row action — viewer archived the PR. The parent fans the
   * write out across the relation owners in `pullRequest.account_ids`. */
  archive: [pullRequest: DashboardPullRequest];
  /** ADR 0018 inverse - viewer asked to pull the PR back out of archive
   * from the Archive view's overflow. */
  unarchive: [pullRequest: DashboardPullRequest];
  /** #331 bulk-archive selection - the leading checkbox flipped. `shiftKey`
   * carries the modifier so the parent can extend a range between the last
   * anchor and this row. */
  "toggle-select": [pullRequest: DashboardPullRequest, shiftKey: boolean];
}>();

const toastStore = useToastStore();

const STALE_THRESHOLD_SECONDS = 7 * 24 * 60 * 60;

/**
 * Shared reactive unix-seconds ref - one ticker across every row, broadcast
 * at 60s. Reading from this inside computeds means the stale label flips on
 * clock progression alone, with no surrounding store change needed.
 */
const nowSeconds = useNowSeconds();

/**
 * Copy for the my-review-state glyph (label + one-line explanation), read from
 * the shared `SIGNAL_COPY` map so the row tooltip and the signals guide (#436)
 * can't drift. See ADR 0031 ("Left-edge encoding").
 */
const myReviewCopy = computed(
  () => SIGNAL_COPY.myReview[props.pullRequest.my_review_state],
);

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

const sinceLabel = computed<string>(() =>
  sinceLabelFor(props.pullRequest, nowSeconds.value),
);

// One-sentence explanation of the since label, surfaced in the time-cell
// tooltip so the terse chip ("approved", "conflicts", "CI failed") doesn't
// leave the user guessing what triggered it.
const sinceExplanation = computed<string>(() => {
  switch (sinceLabel.value) {
    case "opened":
      return "Draft, not yet ready for review.";
    case "conflicts":
      return "Has merge conflicts with the base branch.";
    case "stale":
      return "No activity in the last 7 days.";
    case "CI failed":
      return "One or more CI checks are failing.";
    case "changes":
      return "A reviewer requested changes.";
    case "approved":
      return "Approved and ready to merge.";
    case "updated":
    default:
      return "Most recent activity on the PR.";
  }
});

const updatedAtExact = computed<string>(() =>
  new Intl.DateTimeFormat("en-AU", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(props.pullRequest.updated_at * 1000)),
);

const timeCellTooltip = computed<string>(
  () => `Last updated ${updatedAtExact.value}. ${sinceExplanation.value}`,
);

// Map the since label to a tone class so the chip under the relative time
// inherits the same colour language as the rest of the dashboard (success
// for approved, danger for failure / conflicts / changes-requested, warning
// for stale). `null` leaves the default muted colour.
const sinceTone = computed<string | null>(() => {
  switch (sinceLabel.value) {
    case "approved":
      return "pr-row__time-since--success";
    case "conflicts":
    case "CI failed":
    case "changes":
      return "pr-row__time-since--danger";
    case "stale":
      return "pr-row__time-since--warning";
    default:
      return null;
  }
});

const isStale = computed<boolean>(
  () =>
    nowSeconds.value - props.pullRequest.updated_at > STALE_THRESHOLD_SECONDS,
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

function sinceLabelFor(pr: DashboardPullRequest, now: number): string {
  if (pr.is_draft) return "opened";
  if (pr.mergeable === "CONFLICTING") return "conflicts";
  if (now - pr.updated_at > STALE_THRESHOLD_SECONDS) return "stale";
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

// Unravel mirrors GitHub's URL shape (owner/repo/pull/N) on the unravel.sh
// domain. We don't try to verify the PR is indexed there - a 404 is the
// caller's problem and not worth a round-trip per row to check.
const unravelUrl = computed<string>(
  () =>
    `https://www.unravel.sh/${props.pullRequest.repo.owner}/${props.pullRequest.repo.name}/pull/${props.pullRequest.number}`,
);

function openOnUnravel(event: MouseEvent): void {
  event.stopPropagation();
  void openUrl(unravelUrl.value);
}

function onMarkUnread(): void {
  emit("mark-unread", props.pullRequest);
}

/**
 * Copy the PR's GitHub URL via the standard Web Clipboard API. The Tauri
 * WebView supports `navigator.clipboard.writeText` out of the box for a
 * focused window, so a dedicated Tauri clipboard plugin would be extra
 * surface area for no benefit. Toast on success or failure either way -
 * silent failure here is worse than the click feeling unresponsive.
 */
async function onCopyLink(): Promise<void> {
  try {
    await navigator.clipboard.writeText(props.pullRequest.url);
    toastStore.show("Link copied", { variant: "success", duration: 2000 });
  } catch {
    toastStore.show("Couldn't copy link", { variant: "danger", duration: 2000 });
  }
}

/**
 * Whether the overflow menu can offer an archive action for this row. A
 * Tracked-view row in unified scope can surface without any relation rows
 * (`account_ids === []`) - there's no `(account, PR)` pair to write to, so
 * the archive entry is suppressed. Both Archive and Unarchive paths share
 * this guard; the `isArchiveView` prop picks the wording / command.
 */
const canArchive = computed<boolean>(
  () => props.pullRequest.account_ids.length > 0,
);

function onArchive(): void {
  emit("archive", props.pullRequest);
}

function onUnarchive(): void {
  emit("unarchive", props.pullRequest);
}

/**
 * Intercept the checkbox click on the leading select cell so the row's own
 * `@click` handler doesn't open the PR underneath it. The event's
 * `shiftKey` carries the range-extend modifier so the parent can flip a
 * contiguous slice of the visible list (#331).
 */
function onToggleSelect(event: MouseEvent): void {
  if (props.isArchiveView) return;
  event.stopPropagation();
  emit("toggle-select", props.pullRequest, event.shiftKey);
}

/**
 * Keyboard accessibility for the leading checkbox: Space toggles the
 * selection without opening the row. Mirrors the click handler so the
 * shift modifier still extends a range when the focus lands via Tab.
 */
function onSelectKey(event: KeyboardEvent): void {
  if (props.isArchiveView) return;
  if (event.key !== " " && event.key !== "Enter") return;
  event.preventDefault();
  event.stopPropagation();
  emit("toggle-select", props.pullRequest, event.shiftKey);
}
</script>

<template>
  <article
    :class="[
      'pr-row',
      `pr-row--${density}`,
      unread && 'pr-row--unread',
      focused && 'pr-row--focused',
      selectionActive && 'pr-row--selection-active',
      selected && 'pr-row--selected',
    ]"
    :data-row-pr-id="pullRequest.id"
    :data-needs-attention="needsAttention ? 'true' : undefined"
    role="button"
    tabindex="0"
    @click="onClick"
    @keydown="onKey"
  >
    <span
      class="pr-row__select"
      @click="onToggleSelect"
      @keydown="onSelectKey"
    >
      <PRismCheckbox
        v-if="!isArchiveView"
        :model-value="selected"
        aria-label="Select pull request for bulk archive"
        @update:model-value="() => {}"
      />
    </span>

    <PRismTooltip
      :disabled="!needsAttention"
      side="right"
      as-child
    >
      <span
        class="pr-row__dot"
        :aria-label="needsAttention ? SIGNAL_COPY.attention.label : undefined"
        :aria-hidden="needsAttention ? undefined : 'true'"
      ></span>
      <template #content>
        <div class="pr-row__signal-tip">
          <span class="pr-row__signal-tip-label">{{ SIGNAL_COPY.attention.label }}</span>
          <span class="pr-row__signal-tip-desc">{{ SIGNAL_COPY.attention.description }}</span>
        </div>
      </template>
    </PRismTooltip>

    <PRismTooltip
      :disabled="pullRequest.my_review_state === 'none'"
      side="right"
      as-child
    >
      <span
        :class="['pr-row__state', `my-review--${pullRequest.my_review_state}`]"
        :aria-label="pullRequest.my_review_state === 'none' ? undefined : myReviewCopy.label"
        :aria-hidden="pullRequest.my_review_state === 'none' ? 'true' : undefined"
      >
        <MyReviewStateIcon :state="pullRequest.my_review_state" />
      </span>
      <template #content>
        <div class="pr-row__signal-tip">
          <span class="pr-row__signal-tip-label">{{ myReviewCopy.label }}</span>
          <span class="pr-row__signal-tip-desc">{{ myReviewCopy.description }}</span>
        </div>
      </template>
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
          <span class="pr-row__author-login">{{ pullRequest.author_login }}</span>
        </span>
        <template v-if="linesAdditions !== null && linesDeletions !== null">
          <span class="pr-row__sep" aria-hidden="true">·</span>
          <PRismTooltip :as-child="true">
            <span class="pr-row__lines">
              <span class="pr-row__lines-add">+{{ linesAdditions }}</span>
              <span class="pr-row__lines-del">&minus;{{ linesDeletions }}</span>
              <span v-if="changedFiles !== null" class="pr-row__lines-files">
                · {{ changedFiles }} {{ changedFiles === 1 ? "file" : "files" }}
              </span>
            </span>
            <template #content>
              <div class="pr-row__lines-tooltip">
                <span class="pr-row__lines-tooltip-count pr-row__lines-tooltip-count--add">+{{ linesAdditions }}</span>
                <span class="pr-row__lines-tooltip-label">additions</span>
                <span class="pr-row__lines-tooltip-count pr-row__lines-tooltip-count--del">&minus;{{ linesDeletions }}</span>
                <span class="pr-row__lines-tooltip-label">deletions</span>
                <template v-if="changedFiles !== null">
                  <span class="pr-row__lines-tooltip-count">{{ changedFiles }}</span>
                  <span class="pr-row__lines-tooltip-label">{{ changedFiles === 1 ? "file" : "files" }} changed</span>
                </template>
              </div>
            </template>
          </PRismTooltip>
        </template>
      </div>
    </div>

    <div class="pr-row__accounts-col">
      <PRismTooltip v-if="showAccountMarker" :as-child="true">
        <span
          :class="[
            'pr-row__accounts',
            isSingleRelation && 'pr-row__accounts--single',
          ]"
          :aria-label="accountTooltipText"
        >
          <PRismAvatarStack
            :users="accountStackUsers"
            :max="3"
            size="sm"
            layout="overlap"
            :overflow-tooltip="false"
          />
        </span>
        <template #content>
          <div class="pr-row__accounts-tooltip">
            <div class="pr-row__accounts-tooltip-header">
              Visible from
            </div>
            <ul class="pr-row__accounts-tooltip-list">
              <li
                v-for="account in accountMarkers"
                :key="account.id"
                class="pr-row__accounts-tooltip-row"
              >
                <PRismAvatar
                  :login="account.login"
                  :avatar-url="account.avatar_url"
                  size="sm"
                  :tooltip="null"
                />
                <span class="pr-row__accounts-tooltip-label">
                  {{ account.label || account.login }}
                </span>
              </li>
            </ul>
          </div>
        </template>
      </PRismTooltip>
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

    <PRismTooltip :text="timeCellTooltip" :as-child="true">
      <div :class="['pr-row__time', isStale && 'pr-row__time--stale']">
        <PRismRelativeTime
          :value="pullRequest.updated_at"
          class="pr-row__time-value"
          :disable-tooltip="true"
        />
        <span :class="['pr-row__time-since', sinceTone]">{{ sinceLabel }}</span>
      </div>
    </PRismTooltip>

    <PRismTooltip text="Open on Unravel" :as-child="true">
      <button
        type="button"
        class="pr-row__unravel"
        aria-label="Open on Unravel"
        @click="openOnUnravel"
        @keydown.stop
      >
        <svg
          width="14"
          height="14"
          viewBox="287 261 441 447"
          fill="currentColor"
          aria-hidden="true"
        >
          <g transform="translate(0 1024) scale(0.1 -0.1)" fill="currentColor" stroke="none">
            <path d="M4755 7599 c-251 -37 -444 -98 -680 -214 -105 -51 -215 -115 -288 -165 -128 -90 -305 -253 -415 -383 -249 -293 -436 -707 -483 -1067 -17 -130 -17 -551 -1 -665 52 -349 215 -725 439 -1009 78 -98 231 -254 336 -342 518 -434 1260 -593 1959 -418 693 173 1263 680 1511 1344 44 118 102 344 122 479 20 131 20 424 0 574 -50 380 -215 752 -481 1087 -331 415 -869 713 -1419 785 -163 21 -435 18 -600 -6z m465 -350 c55 -6 113 -15 128 -19 l27 -8 -27 -1 c-57 -2 -236 -42 -343 -77 -239 -77 -437 -196 -606 -364 -239 -236 -372 -481 -440 -810 -29 -140 -31 -425 -5 -575 43 -239 152 -466 320 -664 216 -255 493 -416 846 -492 141 -31 424 -33 574 -5 525 97 943 433 1162 936 20 47 38 86 39 88 8 9 -9 -134 -26 -220 -46 -232 -175 -504 -334 -703 -293 -368 -677 -600 -1135 -688 -98 -19 -149 -22 -345 -22 -198 0 -247 3 -350 23 -312 60 -642 220 -877 427 -245 214 -429 491 -528 795 -49 147 -67 246 -79 434 -23 333 35 639 173 922 186 381 482 675 861 855 295 140 650 202 965 168z m764 -434 c268 -48 487 -284 515 -557 33 -333 -163 -622 -479 -705 -87 -23 -253 -22 -340 1 -168 45 -330 178 -407 332 -126 253 -79 552 116 744 162 160 371 225 595 185z" />
          </g>
        </svg>
      </button>
    </PRismTooltip>

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
            <svg
              class="pr-row__menu-item-icon"
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
              <circle cx="8" cy="8" r="3.25" />
            </svg>
            <span class="pr-row__menu-item-label">Mark unread</span>
          </DropdownMenuItem>
          <DropdownMenuItem
            class="pr-row__menu-item"
            @select="onCopyLink"
          >
            <svg
              class="pr-row__menu-item-icon"
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
              <path d="M6.75 9.25a2.5 2.5 0 003.54 0l2.25-2.25a2.5 2.5 0 10-3.54-3.54l-.75.75" />
              <path d="M9.25 6.75a2.5 2.5 0 00-3.54 0L3.46 9a2.5 2.5 0 103.54 3.54l.75-.75" />
            </svg>
            <span class="pr-row__menu-item-label">Copy link</span>
          </DropdownMenuItem>
          <DropdownMenuItem
            v-if="canArchive && !isArchiveView"
            class="pr-row__menu-item pr-row__menu-item--stacked"
            @select="onArchive"
          >
            <svg
              class="pr-row__menu-item-icon"
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
              <rect x="2" y="3" width="12" height="3" rx="0.75" />
              <path d="M3 6v6.5a1 1 0 001 1h8a1 1 0 001-1V6" />
              <path d="M6.5 9h3" />
            </svg>
            <span class="pr-row__menu-item-label">Archive</span>
            <span class="pr-row__menu-item-hint">
              Hides from PRism only - the PR on GitHub is unchanged.
            </span>
          </DropdownMenuItem>
          <DropdownMenuItem
            v-if="canArchive && isArchiveView"
            class="pr-row__menu-item pr-row__menu-item--stacked"
            @select="onUnarchive"
          >
            <svg
              class="pr-row__menu-item-icon"
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
              <rect x="2" y="3" width="12" height="3" rx="0.75" />
              <path d="M3 6v6.5a1 1 0 001 1h8a1 1 0 001-1V6" />
              <path d="M8 12V7.75M6 9.5L8 7.5l2 2" />
            </svg>
            <span class="pr-row__menu-item-label">Unarchive</span>
            <span class="pr-row__menu-item-hint">
              Restores to PRism's default views. No GitHub action.
            </span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>
  </article>
</template>

<style scoped>
.pr-row {
  position: relative;
  /* Columns: [select] [attention dot] [my-review icon] [#num] [title] [accounts] */
  /* [threads] [reviewers] [ci] [time] [unravel] [github] [kebab] */
  display: grid;
  grid-template-columns: 20px 10px 22px 54px 1fr 64px 144px 180px 80px 80px 24px 24px 28px;
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

/* Keyboard-targeted row. The inset outline reads as a focus ring without
 * stealing space from the row's grid, so the highlight doesn't shift the
 * adjacent rows. The browser's `:focus-visible` ring stacks on top when
 * the underlying element is tab-focused, so click + hotkey share the cue. */
.pr-row--focused {
  outline: 2px solid var(--focus-ring);
  outline-offset: -2px;
  background: var(--bg-2);
}

/* Bulk-select checkbox cell (#331). Hidden by default; reveals on row hover
 * or once any row in the list is selected (sticky once the user starts a
 * multi-select). The cell keeps its slot in the grid either way so rows
 * don't reflow when the checkbox appears - only opacity changes. */
.pr-row__select {
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0;
  transition: opacity 0.12s;
}

.pr-row:hover .pr-row__select,
.pr-row--selection-active .pr-row__select,
.pr-row--selected .pr-row__select {
  opacity: 1;
}

.pr-row--selected {
  background: var(--accent-bg);
}

.pr-row--selected:hover {
  background: var(--accent-bg);
}

/* Leftmost attention dot - the single attention affordance, bound to the
 * `needs_attention` roll-up (ADR 0031). Sits in its own grid cell so rows
 * align identically; only its background switches between transparent and the
 * accent token. Centred inside the 10px column. */
.pr-row__dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: transparent;
  justify-self: center;
}

.pr-row[data-needs-attention="true"] .pr-row__dot {
  background: var(--accent-strong);
}

/* `.pr-row__state` shape + `.my-review--*` colour variants live in
 * `assets/styles/pr-status.css` so the dashboard legend renders the same
 * swatch without re-declaring the palette. */

.pr-row__num {
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
  padding-left: var(--s-2);
}

.pr-row__title-col {
  min-width: 0;
  container-type: inline-size;
  container-name: pr-row-title;
}

/* Narrow rows: drop the branch chip + its separator + the "N files" suffix.
 * The combined-tooltip on the +/- cluster still surfaces the file count, and
 * the author login keeps its ellipsis. Threshold tuned to the breakpoint
 * where the avatar + branch icon start crowding the +/- numbers. */
@container pr-row-title (max-width: 300px) {
  .pr-row__branch,
  .pr-row__branch + .pr-row__sep {
    display: none;
  }
  .pr-row__lines-files {
    display: none;
  }
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

/* Account marker. Sits in its own grid column right before the threads bar
 * in unified scope, hinting at which account(s) saw the PR. Right-aligned
 * within the column with a small gutter so the marker's right edge meets
 * the threads bar's left edge consistently across every row, regardless of
 * title length or density. Single-relation rows render at reduced opacity
 * so the marker reads as a scope hint without competing with the title or
 * the reviewer stack. */
.pr-row__accounts-col {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  min-width: 0;
  padding-right: var(--s-2);
}

.pr-row__accounts {
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

.pr-row__accounts-tooltip {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-width: 240px;
}

.pr-row__accounts-tooltip-header {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  letter-spacing: 0.3px;
  text-transform: uppercase;
  color: var(--text-faint);
}

.pr-row__accounts-tooltip-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.pr-row__accounts-tooltip-row {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--fs-12);
  color: var(--text);
}

.pr-row__accounts-tooltip-label {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Unread rows lean on the title weight as the primary signal; the left-edge
 * dot is the secondary confirmation. */
.pr-row--unread .pr-row__title {
  font-weight: 600;
}

.pr-row__meta-row {
  display: flex;
  align-items: center;
  flex-wrap: nowrap;
  gap: 6px;
  margin-top: 2px;
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  color: var(--text-faint);
  min-width: 0;
  overflow: hidden;
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
  flex: 0 1 auto;
}

.pr-row__branch > svg {
  flex-shrink: 0;
}

.pr-row__branch-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.pr-row__sep {
  color: var(--text-disabled);
  flex-shrink: 0;
}

.pr-row__author {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--text-mute);
  min-width: 0;
  flex: 0 1 auto;
  white-space: nowrap;
}

.pr-row__author-avatar {
  width: 14px;
  height: 14px;
  font-size: 7px;
  flex-shrink: 0;
}

.pr-row__author-login {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.pr-row__lines {
  display: inline-flex;
  gap: 4px;
  flex-shrink: 0;
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
  white-space: nowrap;
}

.pr-row__time-since {
  color: var(--text-faint);
  font-size: var(--fs-9);
  letter-spacing: 0.3px;
  display: block;
  margin-top: 1px;
  white-space: nowrap;
}

/* Tone modifiers track the same colour language as the row strip + ThreadsBar:
 * success for approved, danger for the various failure flavours, warning for
 * stale. Default (no modifier) stays muted. */
.pr-row__time-since--success { color: var(--success); }
.pr-row__time-since--danger  { color: var(--danger); }
.pr-row__time-since--warning { color: var(--warning); }

.pr-row__time--stale .pr-row__time-value {
  color: var(--warning);
}

/* External-link icon buttons (GitHub + Unravel) share the same chrome:
 * 24x24 hit area, transparent background, muted by default, brighten on
 * row hover, fill on button hover / focus. */
.pr-row__github,
.pr-row__unravel {
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

.pr-row:hover .pr-row__github,
.pr-row:hover .pr-row__unravel {
  color: var(--text-mute);
}

.pr-row__github:hover,
.pr-row__unravel:hover {
  background: var(--bg-3);
  color: var(--text);
}

.pr-row__github:focus-visible,
.pr-row__unravel:focus-visible {
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

/* Flat items: icon and label in a single row. Stacked items: icon sits in
 * column 1 aligned to the label row, hint spans column 2 so the secondary
 * copy reads under the label rather than the icon. Same column gutter in
 * both layouts so the icon column lines up vertically across entries. */
.pr-row__menu-item {
  display: grid;
  grid-template-columns: 14px 1fr;
  align-items: center;
  column-gap: 8px;
  height: 28px;
  padding: 0 10px;
  font-size: var(--fs-13);
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

.pr-row__menu-item-icon {
  color: var(--text-faint);
}

.pr-row__menu-item[data-highlighted] .pr-row__menu-item-icon {
  color: inherit;
}

.pr-row__menu-item--stacked {
  grid-template-rows: auto auto;
  align-items: start;
  row-gap: 2px;
  height: auto;
  padding: 6px 10px;
}

.pr-row__menu-item--stacked .pr-row__menu-item-icon {
  /* Optical nudge so the 14px svg's stroke centre lines up with the label
   * cap-height instead of sitting flush against the row's top edge. */
  margin-top: 1px;
}

.pr-row__menu-item-label {
  font-size: var(--fs-13);
  color: inherit;
  line-height: 1.2;
}

.pr-row__menu-item--stacked .pr-row__menu-item-hint {
  grid-column: 2;
}

.pr-row__menu-item-hint {
  font-size: var(--fs-11);
  color: var(--text-mute);
  line-height: 1.35;
}

.pr-row__menu-item--stacked[data-highlighted] .pr-row__menu-item-hint {
  color: var(--text);
}

/* Combined +/- / files tooltip. Lives in the unscoped block because Reka's
 * TooltipPortal teleports the content node to <body> and scoped data-v-*
 * selectors don't follow across the portal. Two-column grid mirrors the
 * reviewer-stack breakdown: signed count column right-aligns, labels read
 * flush left, tabular numerals so the digits stack. */
.pr-row__lines-tooltip {
  display: grid;
  grid-template-columns: auto 1fr;
  align-items: center;
  column-gap: 10px;
  row-gap: 4px;
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  font-variant-numeric: tabular-nums;
  min-width: 140px;
}

.pr-row__lines-tooltip-count {
  justify-self: end;
  color: var(--text-strong);
}

.pr-row__lines-tooltip-count--add {
  color: var(--success);
}

.pr-row__lines-tooltip-count--del {
  color: var(--danger);
}

.pr-row__lines-tooltip-label {
  color: var(--text-mute);
}

/* Two-line signal tooltip (attention dot + my-review-state glyph): a terse
 * label over the one-sentence explanation pulled from `signalCopy.ts`. In the
 * unscoped block because Reka's TooltipPortal teleports the content to
 * <body>, where scoped data-v-* selectors don't follow. */
.pr-row__signal-tip {
  display: flex;
  flex-direction: column;
  gap: 2px;
  max-width: 220px;
}

.pr-row__signal-tip-label {
  font-size: var(--fs-12);
  font-weight: 600;
  color: var(--text-strong);
}

.pr-row__signal-tip-desc {
  font-size: var(--fs-11);
  color: var(--text-mute);
  line-height: 1.4;
}
</style>
