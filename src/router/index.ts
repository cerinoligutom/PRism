import { createRouter, createWebHistory } from "vue-router";

import DashboardView from "@/views/DashboardView.vue";
import type { DashboardView as DashboardViewName } from "@/stores/dashboard";

// The four dashboard views share the same component and differ only by the
// `view` route meta. The store reads `to.meta.dashboardView` on each navigation
// and reloads.
export const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/",
      redirect: { name: "dashboard.authored" },
    },
    {
      path: "/dashboard/authored",
      name: "dashboard.authored",
      component: DashboardView,
      meta: { dashboardView: "authored" satisfies DashboardViewName },
    },
    {
      path: "/dashboard/assigned",
      name: "dashboard.assigned",
      component: DashboardView,
      meta: { dashboardView: "assigned" satisfies DashboardViewName },
    },
    {
      path: "/dashboard/watching",
      name: "dashboard.watching",
      component: DashboardView,
      meta: { dashboardView: "watching" satisfies DashboardViewName },
    },
    {
      path: "/dashboard/team",
      name: "dashboard.team",
      component: DashboardView,
      meta: { dashboardView: "team" satisfies DashboardViewName },
    },
    {
      // Detail-surface route host for `prDetailSurface = 'route'`. The view
      // param keeps the back-button breadcrumb honest about which list the
      // user came from; the id resolves the PR.
      path: "/dashboard/:view/pr/:id",
      name: "pr-detail",
      component: () => import("@/views/PullRequestDetailView.vue"),
      props: (route) => ({ pullRequestId: Number(route.params.id) }),
    },
    {
      path: "/onboarding",
      name: "onboarding",
      component: () => import("@/views/OnboardingView.vue"),
    },
    {
      path: "/settings",
      name: "settings",
      component: () => import("@/views/SettingsView.vue"),
      children: [
        {
          path: "",
          redirect: { name: "settings.accounts" },
        },
        {
          path: "accounts",
          name: "settings.accounts",
          component: () => import("@/views/settings/AccountsPanel.vue"),
        },
        {
          path: "repositories",
          name: "settings.repositories",
          component: () => import("@/views/settings/RepositoriesSettings.vue"),
        },
        {
          path: "appearance",
          name: "settings.appearance",
          component: () => import("@/views/settings/AppearanceSettings.vue"),
        },
      ],
    },
  ],
});
