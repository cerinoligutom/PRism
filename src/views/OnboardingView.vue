<script setup lang="ts">
import PRismButton from "@/components/ui/PRismButton.vue";

// Full 3-step onboarding (welcome → PAT → org/repo select) lands with issue
// #10. This placeholder anchors the route and confirms the shell handles
// modal-style full-bleed views.
</script>

<template>
  <section class="onboarding-placeholder">
    <div class="onboarding-placeholder__card">
      <h2 class="onboarding-placeholder__title">Onboarding</h2>
      <p class="onboarding-placeholder__copy">
        The PAT-entry and org/repo selection flow lands with
        <a class="onboarding-placeholder__link" href="https://github.com/cerinoligutom/PRism/issues/10" target="_blank" rel="noreferrer">
          issue #10
        </a>.
      </p>
      <PRismButton to="/">Back to dashboard</PRismButton>
    </div>
  </section>
</template>

<style scoped>
.onboarding-placeholder {
  display: grid;
  place-items: center;
  height: 100%;
  padding: var(--s-7);
}

.onboarding-placeholder__card {
  max-width: 420px;
  text-align: center;
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  align-items: center;
}

.onboarding-placeholder__title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
}

.onboarding-placeholder__copy {
  margin: 0;
  font-size: var(--fs-13);
  color: var(--text-mute);
}

.onboarding-placeholder__link {
  color: var(--accent);
  text-decoration: none;
}
</style>
