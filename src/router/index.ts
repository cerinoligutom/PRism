import { createRouter, createWebHistory } from "vue-router";

import DashboardView from "@/views/DashboardView.vue";

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: "/",
      name: "dashboard",
      component: DashboardView,
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
          path: "accounts",
          name: "settings.accounts",
          component: () => import("@/views/settings/AccountsPanel.vue"),
        },
      ],
    },
  ],
});
