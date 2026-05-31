import { createRouter, createWebHistory } from "vue-router";

import DashboardView from "@/views/DashboardView.vue";
import type { DashboardView as DashboardViewName } from "@/stores/dashboard";

/**
 * Map a `DashboardView` name to its route name. The `assigned` view is served
 * under the `dashboard.review-requested` route so the URL slug matches the
 * sidebar / title vocabulary (see the `/dashboard/review-requested` route
 * below); every other view shares its name with the route suffix. Callers in
 * the notification, deep-link, and inbox paths use this when they need to
 * push to a dashboard route by view name rather than by string literal.
 */
export function dashboardRouteName(view: DashboardViewName): string {
  return view === "assigned"
    ? "dashboard.review-requested"
    : `dashboard.${view}`;
}

// The four dashboard views share the same component and differ only by the
// `view` route meta. The store reads `to.meta.dashboardView` on each navigation
// and reloads.
export const router = createRouter({
  history: createWebHistory(),
  // Honour a route hash so the legend deep-links land on the matching section
  // of the "How signals work" page (#436). `scroll-margin-top` on the targets
  // keeps the heading clear of the scroll-container edge. Hash-less navigations
  // return `false` to preserve the prior no-op behaviour - the views manage
  // their own scroll containers, so forcing the window to the top would change
  // existing routes.
  scrollBehavior(to) {
    if (to.hash) {
      return { el: to.hash, behavior: "smooth" };
    }
    return false;
  },
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
      // Issue #378: persistent notifications inbox. Distinct from the four
      // PR views above - this surface mirrors dispatched OS toasts rather
      // than reading the PR table directly, so it uses its own view
      // component instead of `DashboardView`.
      path: "/dashboard/notifications",
      name: "dashboard.notifications",
      component: () => import("@/views/NotificationsView.vue"),
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
      // Issue #436: the "How signals work" reference page. Fixture-driven and
      // always reachable - it calls no auth- or sync-gated store, so it
      // renders before any account is connected. The dashboard and
      // conversation legends deep-link into its section anchors.
      path: "/signals",
      name: "signals",
      component: () => import("@/views/HowSignalsWorkView.vue"),
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
