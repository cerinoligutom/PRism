<script setup lang="ts">
import { computed } from "vue";
import {
  DialogContent,
  DialogDescription,
  DialogOverlay,
  DialogPortal,
  DialogRoot,
  DialogTitle,
  VisuallyHidden,
} from "reka-ui";

import PullRequestConversation from "./PullRequestConversation.vue";
import { useDashboardStore } from "@/stores/dashboard";

interface Props {
  pullRequestId: number | null;
}

const props = defineProps<Props>();

const emit = defineEmits<{
  close: [];
}>();

// `DialogRoot`'s `open` is two-way bound through `v-model:open`. When the host
// passes `null` we report closed; when the user dismisses (Esc, overlay click,
// close button) Reka emits `update:open` with `false` and we relay through
// `close` so the parent clears its own state.
const open = computed<boolean>({
  get: () => props.pullRequestId !== null,
  set: (next) => {
    if (!next) emit("close");
  },
});

const dashboard = useDashboardStore();

// Resolve the dashboard row for the header so the drawer carries the same
// breadcrumb the route host renders. The drawer is only mounted off the
// dashboard view, so this is always populated when `pullRequestId !== null`.
const row = computed(() =>
  props.pullRequestId === null
    ? null
    : dashboard.pullRequests.find((pr) => pr.id === props.pullRequestId) ?? null,
);

const headerLine = computed<string>(() => {
  const r = row.value;
  if (r === null) return "Pull request";
  return `${r.repo.owner}/${r.repo.name} #${r.number}`;
});

const titleLine = computed<string>(() => row.value?.title ?? "Pull request");
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="pr-drawer__overlay" />
      <DialogContent class="pr-drawer">
        <VisuallyHidden>
          <DialogDescription>
            Conversation, reviews, and timeline for the selected pull request.
          </DialogDescription>
        </VisuallyHidden>
        <header class="pr-drawer__header">
          <div class="pr-drawer__header-text">
            <span class="pr-drawer__crumb mono">{{ headerLine }}</span>
            <DialogTitle class="pr-drawer__title">{{ titleLine }}</DialogTitle>
          </div>
          <button
            type="button"
            class="btn btn-icon btn-ghost"
            aria-label="Close pull request drawer"
            @click="emit('close')"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="1.6"
              stroke-linecap="round"
            >
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </button>
        </header>
        <div class="pr-drawer__body">
          <PullRequestConversation
            v-if="pullRequestId !== null"
            :pull-request-id="pullRequestId"
          />
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<style scoped>
.pr-drawer__overlay {
  position: fixed;
  inset: 0;
  background: rgb(0 0 0 / 0.45);
  /* Tooltips sit at z-index 50 (PRismTooltip); the modal drawer must layer
     above them while open. */
  z-index: 60;
  animation: pr-drawer-fade-in 0.16s ease-out;
}

.pr-drawer__overlay[data-state="closed"] {
  animation: pr-drawer-fade-out 0.16s ease-in;
}

.pr-drawer {
  position: fixed;
  top: 0;
  right: 0;
  bottom: 0;
  /* 80% of the dashboard content area (viewport minus the sidebar). The
     dimmed overlay covers the sidebar; the calc keeps a slice of the list
     visible on the left for context. Clamped to 100vw so narrow viewports
     still render. */
  width: min(calc((100vw - var(--sidebar-width)) * 0.8), 100vw);
  background: var(--bg-1);
  border-left: 1px solid var(--border-1);
  box-shadow: var(--shadow-3);
  z-index: 70;
  display: flex;
  flex-direction: column;
  min-height: 0;
  animation: pr-drawer-slide-in 0.2s ease-out;
}

.pr-drawer[data-state="closed"] {
  animation: pr-drawer-slide-out 0.18s ease-in;
}

.pr-drawer__header {
  display: flex;
  align-items: flex-start;
  gap: var(--s-3);
  padding: var(--s-4) var(--s-5);
  border-bottom: 1px solid var(--border-1);
  background: var(--bg-2);
}

.pr-drawer__header-text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.pr-drawer__crumb {
  font-size: var(--fs-11);
  color: var(--text-faint);
  letter-spacing: 0.4px;
  text-transform: uppercase;
}

.pr-drawer__title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.01em;
  line-height: var(--lh-tight);
  overflow: hidden;
  text-overflow: ellipsis;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
}

.pr-drawer__body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
}

.pr-drawer__body > * {
  flex: 1;
  min-height: 0;
}

@keyframes pr-drawer-slide-in {
  from {
    transform: translateX(100%);
  }
  to {
    transform: translateX(0);
  }
}

@keyframes pr-drawer-slide-out {
  from {
    transform: translateX(0);
  }
  to {
    transform: translateX(100%);
  }
}

@keyframes pr-drawer-fade-in {
  from {
    opacity: 0;
  }
  to {
    opacity: 1;
  }
}

@keyframes pr-drawer-fade-out {
  from {
    opacity: 1;
  }
  to {
    opacity: 0;
  }
}
</style>
