<script setup lang="ts">
import AppShell from "@/components/AppShell.vue";
import { useKeyboardShortcuts } from "@/composables/useKeyboardShortcuts";

useKeyboardShortcuts();
</script>

<template>
  <AppShell />
</template>
