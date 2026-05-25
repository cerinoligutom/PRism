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
      // URL renamed to "/dashboard/review-requested" so the address-bar slug
      // matches the sidebar / title vocabulary. Internal `dashboardView` meta
      // stays `assigned` to avoid a wider rename across the Tauri serde
      // surface and Rust enum variant.
      path: "/dashboard/review-requested",
      name: "dashboard.review-requested",
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
      path: "/dashboard/tracked",
      name: "dashboard.tracked",
      component: DashboardView,
      meta: { dashboardView: "tracked" satisfies DashboardViewName },
    },
    {
      // ADR 0018: archive bucket. Inverts the default-view archive predicate
      // server-side; the frontend reuses `DashboardView.vue` with the
      // view-specific copy + chip-rail suppression keyed on the same meta.
      path: "/dashboard/archive",
      name: "dashboard.archive",
      component: DashboardView,
      meta: { dashboardView: "archive" satisfies DashboardViewName },
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
          redirect: { name: "settings.appearance" },
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
        {
          path: "notifications",
          name: "settings.notifications",
          component: () => import("@/views/settings/NotificationsSettings.vue"),
        },
        {
          path: "sync",
          name: "settings.sync",
          component: () => import("@/views/settings/SyncSettings.vue"),
        },
        {
          path: "updates",
          name: "settings.updates",
          component: () => import("@/views/settings/UpdatesSettings.vue"),
        },
        {
          path: "about",
          name: "settings.about",
          component: () => import("@/views/settings/AboutPanel.vue"),
        },
      ],
    },
  ],
});
