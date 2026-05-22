<script setup lang="ts">
import AppShell from "@/components/AppShell.vue";
import PRismToastViewport from "@/components/ui/PRismToastViewport.vue";
import { useKeyboardShortcuts } from "@/composables/useKeyboardShortcuts";
import { useNotificationRouter } from "@/composables/useNotificationRouter";

useKeyboardShortcuts();
useNotificationRouter();
</script>

<template>
  <AppShell />
  <PRismToastViewport />
</template>
