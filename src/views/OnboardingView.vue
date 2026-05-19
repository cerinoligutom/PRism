<script setup lang="ts">
import { RouterLink } from "vue-router";

// Full 3-step onboarding (welcome → PAT → org/repo select) lands with issue
// #10. This placeholder anchors the route and confirms the shell handles
// modal-style full-bleed views.
</script>

<template>
  <section class="placeholder">
    <div class="placeholder-card">
      <h2>Onboarding</h2>
      <p>
        The PAT-entry and org/repo selection flow lands with
        <a href="https://github.com/cerinoligutom/PRism/issues/10" target="_blank" rel="noreferrer">
          issue #10
        </a>.
      </p>
      <RouterLink to="/" class="btn">Back to dashboard</RouterLink>
    </div>
  </section>
</template>

<style scoped>
.placeholder {
  display: grid;
  place-items: center;
  height: 100%;
  padding: 32px;
}
.placeholder-card {
  max-width: 420px;
  text-align: center;
  display: flex;
  flex-direction: column;
  gap: 14px;
  align-items: center;
}
.placeholder-card h2 {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
  color: var(--text-strong);
}
.placeholder-card p {
  margin: 0;
  font-size: 13px;
  color: var(--text-mute);
}
.placeholder-card a {
  color: var(--accent);
  text-decoration: none;
}
</style>
