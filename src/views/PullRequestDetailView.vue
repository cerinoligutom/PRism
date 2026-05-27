<script setup lang="ts">
import { computed, onMounted } from "vue";
import { useRoute, useRouter } from "vue-router";

import PullRequestConversation from "@/components/conversation/PullRequestConversation.vue";
import PullRequestExternalLinks from "@/components/PullRequestExternalLinks.vue";
import {
  useDashboardStore,
  type DashboardView as DashboardViewName,
} from "@/stores/dashboard";

interface Props {
  pullRequestId: number;
}

const props = defineProps<Props>();

const route = useRoute();
const router = useRouter();
const dashboard = useDashboardStore();

const VIEW_LABELS: Record<DashboardViewName, string> = {
  authored: "Authored",
  assigned: "Assigned",
  watching: "Watching",
  tracked: "Tracked",
  archive: "Archive",
};

function isView(value: unknown): value is DashboardViewName {
  return (
    value === "authored" ||
    value === "assigned" ||
    value === "watching" ||
    value === "tracked" ||
    value === "archive"
  );
}

const viewParam = computed<DashboardViewName>(() => {
  const raw = route.params.view;
  return isView(raw) ? raw : "authored";
});

const viewLabel = computed<string>(() => VIEW_LABELS[viewParam.value]);

const row = computed(
  () =>
    dashboard.pullRequests.find((pr) => pr.id === props.pullRequestId) ?? null,
);

const headerCrumb = computed<string>(() => {
  const r = row.value;
  if (r === null) return `${viewLabel.value} / #${props.pullRequestId}`;
  return `${viewLabel.value} / ${r.repo.owner}/${r.repo.name} / #${r.number}`;
});

const headerTitle = computed<string>(() => row.value?.title ?? "");

function goBack(): void {
  // Prefer history.back() when the user navigated here from inside the app,
  // so the dashboard's scroll position survives. The fallback hits the
  // matching named dashboard route for cold loads.
  if (window.history.length > 1) {
    router.back();
    return;
  }
  void router.push({ name: `dashboard.${viewParam.value}` });
}

onMounted(async () => {
  // Cold-load guard: the conversation tabs (and the drawer's header) read the
  // dashboard row by id; without this the status-timeline tab falls back to
  // the M3-E "Timeline unavailable" placeholder until the user navigates back
  // and forward. Loading here makes deep-link entry produce the same view as
  // an in-app navigation.
  //
  // We also align `dashboard.view` to the route's `:view` param so the
  // resulting `pullRequests` list contains the row whose id we're rendering.
  // `setView` is a no-op when the view already matches and triggers a single
  // load otherwise; the explicit `load()` covers the same-view branch.
  if (dashboard.view !== viewParam.value) {
    await dashboard.setView(viewParam.value);
  } else if (dashboard.pullRequests.length === 0) {
    await dashboard.load();
  }
});
</script>

<template>
  <section class="pr-detail">
    <header class="pr-detail__header">
      <button
        type="button"
        class="btn btn-ghost btn-sm pr-detail__back"
        aria-label="Back to dashboard"
        @click="goBack"
      >
        <svg
          width="12"
          height="12"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linecap="round"
        >
          <path d="M10 3L5 8l5 5" />
        </svg>
        <span>Back</span>
      </button>
      <div class="pr-detail__crumb-block">
        <span class="pr-detail__crumb mono">{{ headerCrumb }}</span>
        <h1 v-if="headerTitle !== ''" class="pr-detail__title">{{ headerTitle }}</h1>
      </div>
      <PullRequestExternalLinks
        v-if="row !== null"
        :owner="row.repo.owner"
        :repo="row.repo.name"
        :number="row.number"
        :url="row.url"
      />
    </header>
    <div class="pr-detail__body">
      <PullRequestConversation :pull-request-id="pullRequestId" />
    </div>
  </section>
</template>

<style scoped>
.pr-detail {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--bg-1);
}

.pr-detail__header {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  padding: var(--s-4) var(--s-6);
  border-bottom: 1px solid var(--border-1);
  background: var(--bg-1);
}

.pr-detail__back {
  flex-shrink: 0;
}

.pr-detail__crumb-block {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
  flex: 1;
}

.pr-detail__crumb {
  font-size: var(--fs-11);
  color: var(--text-faint);
  letter-spacing: 0.4px;
  text-transform: uppercase;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.pr-detail__title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.3px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pr-detail__body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.pr-detail__body > * {
  flex: 1;
  min-height: 0;
}
</style>
