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
  metadata.value === null ? "" : `${metadata.value.os} · ${metadata.value.arch}`,
);

const isDevBuild = computed<boolean>(() => metadata.value?.profile !== "release");

const hostsLabel = computed<string>(() => {
  const hosts = Array.from(new Set(accounts.accounts.map((a) => a.host)));
  if (hosts.length === 0) return "none connected";
  return hosts.join(", ");
});

/**
 * Diagnostics payload pasted into bug reports. Plain text rather than markdown
 * because it lands in GitHub issue templates, Discord, email — all of which
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
      return "Copy failed — select & copy below";
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

    <!-- Hero identity card. Brand mark + the canonical identifiers a bug
         report needs (version, commit, date, platform). The dev-profile
         badge only appears for non-release builds so release users aren't
         shown an irrelevant pill. -->
    <section class="about-card">
      <span class="about-card__mark" aria-hidden="true">
        <svg viewBox="0 0 32 32" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round">
          <line x1="2" y1="16" x2="9.5" y2="16" opacity="0.55" />
          <path d="M16 4 L28 26 L4 26 Z" />
          <line x1="20.5" y1="17.5" x2="30" y2="11" stroke="oklch(0.72 0.18 25)" />
          <line x1="21" y1="19" x2="30" y2="16" stroke="oklch(0.78 0.15 80)" />
          <line x1="21.5" y1="20.5" x2="30" y2="21" stroke="oklch(0.74 0.16 145)" />
          <line x1="22" y1="22" x2="29" y2="26" stroke="oklch(0.72 0.14 320)" />
        </svg>
      </span>

      <div class="about-card__text">
        <div class="about-card__name-row">
          <span class="about-card__name">PRism</span>
          <code class="about-card__version">{{ versionLabel || "v—" }}</code>
          <span v-if="isDevBuild && metadata !== null" class="about-card__profile">{{ metadata.profile }}</span>
        </div>
        <div class="about-card__meta">
          <code>{{ buildLabel || "Build —" }}</code>
          <template v-if="builtAtLabel !== ''">
            <span aria-hidden="true">·</span>
            <code>{{ builtAtLabel }}</code>
          </template>
        </div>
        <div class="about-card__meta">
          <code>{{ platformLabel || "platform —" }}</code>
        </div>
      </div>
    </section>

    <section class="about-panel__section">
      <div class="about-panel__section-head">
        <h3 class="about-panel__section-title">Diagnostics</h3>
        <span class="about-panel__section-desc">
          Paste this block into a bug report. No PATs, no PR data — only build identifiers and connected hosts.
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

/* ────── about-card BEM block (build-identity hero) ──────
 * Mark + identity stack laid out horizontally; the brand mark is the same
 * SVG used in the sidebar at a larger size. */
.about-card {
  display: flex;
  align-items: center;
  gap: var(--s-5);
  padding: 20px 22px;
  background: var(--bg-2);
  border: 1px solid var(--border-1);
  border-radius: var(--r-3);
  margin-bottom: var(--s-7);
}

.about-card__mark {
  flex-shrink: 0;
  width: 56px;
  height: 56px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-strong);
}

.about-card__mark svg {
  width: 100%;
  height: 100%;
}

.about-card__text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.about-card__name-row {
  display: flex;
  align-items: baseline;
  gap: var(--s-3);
  flex-wrap: wrap;
}

.about-card__name {
  font-size: var(--fs-20);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.3px;
}

.about-card__version {
  font-family: var(--font-mono);
  font-size: var(--fs-13);
  color: var(--text);
  background: var(--bg-3);
  padding: 2px 8px;
  border-radius: var(--r-2);
  border: 1px solid var(--border-1);
  font-variant-numeric: tabular-nums;
}

/* Dev-profile pill: tinted with the accent so it stands out enough to
 * remind the user this isn't a release build. */
.about-card__profile {
  font-family: var(--font-mono);
  font-size: var(--fs-10);
  letter-spacing: 0.5px;
  text-transform: uppercase;
  color: var(--accent);
  background: var(--accent-bg, color-mix(in oklch, var(--accent) 14%, transparent));
  border: 1px solid color-mix(in oklch, var(--accent) 35%, transparent);
  padding: 2px 8px;
  border-radius: var(--r-2);
}

.about-card__meta {
  display: flex;
  align-items: center;
  gap: 6px;
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  color: var(--text-faint);
  font-variant-numeric: tabular-nums;
}

.about-card__meta code {
  font-family: inherit;
  background: transparent;
  padding: 0;
  border: 0;
  color: inherit;
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
