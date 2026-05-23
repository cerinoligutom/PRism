<script setup lang="ts">
import { computed, ref } from "vue";

import PRismButton from "@/components/ui/PRismButton.vue";
import { useAppMetadata } from "@/composables/useAppMetadata";
import { useAccountsStore } from "@/stores/accounts";

const { metadata } = useAppMetadata();
const accounts = useAccountsStore();

const versionLabel = computed<string>(() =>
  metadata.value === null ? "" : `v${metadata.value.version}`,
);

const buildLabel = computed<string>(() =>
  metadata.value === null ? "" : `Build ${metadata.value.commit_sha}`,
);

const builtAtLabel = computed<string>(() => metadata.value?.build_date ?? "");

const platformLabel = computed<string>(() =>
  metadata.value === null ? "" : `${metadata.value.os} ${metadata.value.arch}`,
);

const isDevBuild = computed<boolean>(() => metadata.value?.profile !== "release");

const hostsLabel = computed<string>(() => {
  const hosts = Array.from(new Set(accounts.accounts.map((a) => a.host)));
  if (hosts.length === 0) return "none connected";
  return hosts.join(", ");
});

/**
 * Diagnostics payload pasted into bug reports. Plain text rather than markdown
 * because it lands in GitHub issue templates, Discord, email - all of which
 * render plain text cleanly and markdown inconsistently.
 */
const diagnosticsText = computed<string>(() => {
  const m = metadata.value;
  if (m === null) return "";
  const lines = [
    `PRism v${m.version} (Build ${m.commit_sha})`,
    `Built: ${m.build_date} (${m.profile})`,
    `Platform: ${m.os} ${m.arch}`,
    `GitHub hosts: ${hostsLabel.value}`,
  ];
  return lines.join("\n");
});

type CopyState = "idle" | "copied" | "error";
const copyState = ref<CopyState>("idle");
let copyResetTimer: ReturnType<typeof setTimeout> | null = null;

async function copyDiagnostics(): Promise<void> {
  if (diagnosticsText.value === "") return;
  try {
    await navigator.clipboard.writeText(diagnosticsText.value);
    copyState.value = "copied";
  } catch (err) {
    console.warn("copy diagnostics failed:", err);
    copyState.value = "error";
  }
  if (copyResetTimer !== null) clearTimeout(copyResetTimer);
  copyResetTimer = setTimeout(() => {
    copyState.value = "idle";
  }, 1800);
}

const copyButtonLabel = computed<string>(() => {
  switch (copyState.value) {
    case "copied":
      return "Copied!";
    case "error":
      return "Copy failed - select & copy below";
    case "idle":
    default:
      return "Copy diagnostics";
  }
});
</script>

<template>
  <div class="about-panel">
    <header class="about-panel__header">
      <h1 class="about-panel__title">About PRism</h1>
    </header>

    <section class="about-panel__section">
      <div class="about-panel__section-head">
        <h3 class="about-panel__section-title">Build</h3>
        <span class="about-panel__section-desc">
          Useful when filing a bug - copy and paste the diagnostics block below.
        </span>
      </div>

      <div class="about-panel__row-list">
        <div class="set-row">
          <div>
            <div class="set-row__name">Version</div>
            <div class="set-row__desc">SemVer release, synced across the three build files.</div>
          </div>
          <code class="about-panel__value">{{ versionLabel || "..." }}</code>
        </div>

        <div class="set-row">
          <div>
            <div class="set-row__name">Build</div>
            <div class="set-row__desc">First 6 chars of the build commit.</div>
          </div>
          <code class="about-panel__value">{{ buildLabel || "..." }}</code>
        </div>

        <div class="set-row">
          <div>
            <div class="set-row__name">Built</div>
            <div class="set-row__desc">UTC timestamp baked at compile time.</div>
          </div>
          <code class="about-panel__value">{{ builtAtLabel || "..." }}</code>
        </div>

        <div class="set-row">
          <div>
            <div class="set-row__name">Platform</div>
            <div class="set-row__desc">Host OS and CPU architecture.</div>
          </div>
          <code class="about-panel__value">{{ platformLabel || "..." }}</code>
        </div>

        <div v-if="isDevBuild" class="set-row">
          <div>
            <div class="set-row__name">Profile</div>
            <div class="set-row__desc">Cargo build profile. Only shown for non-release builds.</div>
          </div>
          <code class="about-panel__value about-panel__value--dev">{{ metadata?.profile ?? "..." }}</code>
        </div>
      </div>
    </section>

    <section class="about-panel__section">
      <div class="about-panel__section-head">
        <h3 class="about-panel__section-title">Diagnostics</h3>
        <span class="about-panel__section-desc">
          Paste this block into a bug report. No PATs, no PR data - only build identifiers and connected hosts.
        </span>
      </div>

      <pre class="about-panel__diagnostics" aria-label="Diagnostics payload">{{ diagnosticsText || "Loading..." }}</pre>

      <div class="about-panel__actions">
        <PRismButton
          variant="primary"
          :disabled="metadata === null"
          @click="copyDiagnostics"
        >
          {{ copyButtonLabel }}
        </PRismButton>
      </div>
    </section>
  </div>
</template>

<style scoped>
.about-panel__header {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  margin-bottom: var(--s-6);
}

.about-panel__title {
  margin: 0;
  font-size: var(--fs-24);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.5px;
}

.about-panel__section {
  margin-bottom: var(--s-7);
}

.about-panel__section-head {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border-1);
  margin-bottom: var(--s-4);
}

.about-panel__section-title {
  margin: 0;
  font-size: var(--fs-16);
  font-weight: 600;
  color: var(--text-strong);
}

.about-panel__section-desc {
  font-size: var(--fs-12);
  color: var(--text-mute);
}

.about-panel__row-list {
  display: flex;
  flex-direction: column;
  gap: 1px;
  background: var(--border-1);
  border-radius: var(--r-3);
  overflow: hidden;
}

/* Inline value chip in the right-hand column of each row. Monospace so
 * SHA / date / version values stay aligned with the diagnostics block. */
.about-panel__value {
  font-family: var(--font-mono);
  font-size: var(--fs-12);
  color: var(--text-strong);
  background: var(--bg-3);
  padding: 4px 10px;
  border-radius: var(--r-2);
  border: 1px solid var(--border-1);
}

.about-panel__value--dev {
  color: var(--text-warning, var(--text-strong));
  border-color: var(--border-2);
}

.about-panel__diagnostics {
  font-family: var(--font-mono);
  font-size: var(--fs-12);
  color: var(--text);
  background: var(--bg-3);
  border: 1px solid var(--border-1);
  border-radius: var(--r-3);
  padding: 12px 14px;
  margin: 0;
  white-space: pre-wrap;
  user-select: text;
}

.about-panel__actions {
  margin-top: var(--s-3);
  display: flex;
  justify-content: flex-end;
}
</style>
