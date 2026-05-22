<script setup lang="ts">
import AppShell from "@/components/AppShell.vue";
import { useKeyboardShortcuts } from "@/composables/useKeyboardShortcuts";
import { useNotificationRouter } from "@/composables/useNotificationRouter";

useKeyboardShortcuts();
useNotificationRouter();
</script>

<template>
  <AppShell />
</template>
