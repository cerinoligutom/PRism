<script setup lang="ts">
import { computed } from "vue";
import {
  DialogClose,
  DialogContent,
  DialogOverlay,
  DialogPortal,
  DialogRoot,
  DialogTitle,
} from "reka-ui";
import { openUrl } from "@tauri-apps/plugin-opener";
import { marked } from "marked";

import PRismButton from "@/components/ui/PRismButton.vue";
import PRismMarkdown from "@/components/ui/PRismMarkdown.vue";
import { UNRELEASED_VERSION, type ChangelogEntry } from "@/lib/changelog";

/**
 * In-app "What's new" dialog (ADR 0025).
 *
 * Surfaces a per-version changelog slice on launch after the user has
 * crossed at least one version boundary. The host (App-level launch hook)
 * supplies the pre-sliced entries; the dialog opens itself when the slice
 * is non-empty and emits `dismissed` on every close path (Got it CTA,
 * Escape, backdrop click) so the host can advance the last-seen cursor.
 *
 * `PRismMarkdown` expects HTML; the bundled markdown is converted with
 * `marked` first. The HTML feeds back through the same DOMPurify + Shiki
 * pipeline as comment bodies, so there's no second sanitiser to audit.
 */

interface Props {
  /** When non-empty, the dialog renders open. Owned by the host. */
  entries: readonly ChangelogEntry[];
  /** Running app version; titles the dialog and the GitHub release link. */
  currentVersion: string;
  /**
   * Override the dialog title (issue #375). Manual opens from the About
   * panel pass "Changelog" because the dialog shows the full file, not a
   * version slice. `undefined` keeps the auto-open default
   * (`What's new in vX.Y.Z`).
   */
  title?: string;
  /**
   * Override the GitHub link the footer button opens (#377). Manual opens
   * pass the releases-index URL because pre-release builds may not have a
   * `/releases/tag/vX.Y.Z` page for the running version. `undefined` keeps
   * the auto-open default of the tag-specific release page.
   */
  releaseUrl?: string;
}

const props = defineProps<Props>();
const emit = defineEmits<{ dismissed: [] }>();

// Two-way bind to Reka's `DialogRoot.open`. The dialog is "open" whenever
// the host hands us a non-empty slice; closing flips the model and emits
// `dismissed`, which the host handles by writing `last_seen_version`.
const open = computed<boolean>({
  get: () => props.entries.length > 0,
  set: (next) => {
    if (!next) emit("dismissed");
  },
});

const dialogTitle = computed<string>(
  () => props.title ?? `What's new in v${props.currentVersion}`,
);

const concatenatedHtml = computed<string>(() => {
  if (props.entries.length === 0) return "";
  // Compose: each entry gets a `## v<version> - <date>` heading followed
  // by its body. The body itself uses `### Added / Changed / ...`
  // subheadings; the heading level shift keeps the dialog title (a `<h2>`
  // in Reka's DialogTitle) typographically distinct from the per-version
  // headings inside the scroll surface.
  //
  // The Unreleased sentinel (#377) renders as plain `## Unreleased` — no
  // `v` prefix, no ` - date` suffix — because it isn't a semver entry.
  //
  // Marked options: GFM on (lists, tables, autolinks for bare URLs); no
  // `breaks` so a single `\n` stays a soft break and blank lines mark
  // paragraph boundaries, matching what `CHANGELOG.md` uses.
  const composed = props.entries
    .map((entry) => {
      const heading =
        entry.version === UNRELEASED_VERSION
          ? `## Unreleased`
          : `## v${entry.version} - ${entry.date}`;
      return `${heading}\n\n${entry.body}`;
    })
    .join("\n\n");
  const rendered = marked.parse(composed, {
    gfm: true,
    breaks: false,
    async: false,
  });
  return typeof rendered === "string" ? rendered : "";
});

const footerHref = computed<string>(
  () =>
    props.releaseUrl ??
    `https://github.com/cerinoligutom/PRism/releases/tag/v${props.currentVersion}`,
);

async function openReleasePage(): Promise<void> {
  try {
    await openUrl(footerHref.value);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("failed to open release page", err);
  }
}
</script>

<template>
  <DialogRoot v-model:open="open">
    <DialogPortal>
      <DialogOverlay class="whats-new__overlay" />
      <DialogContent class="whats-new">
        <header class="whats-new__header">
          <DialogTitle class="whats-new__title">{{ dialogTitle }}</DialogTitle>
        </header>

        <div class="whats-new__body">
          <PRismMarkdown :html="concatenatedHtml" class="whats-new__changelog" />
        </div>

        <footer class="whats-new__foot">
          <PRismButton type="button" @click="openReleasePage">
            Open full changelog on GitHub
          </PRismButton>
          <DialogClose as-child>
            <PRismButton type="button" variant="primary">Got it</PRismButton>
          </DialogClose>
        </footer>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>

<!--
  Styles are global (no `scoped`) because Reka's `DialogPortal` teleports
  the dialog content to `document.body`, and Vue's scoped `data-v-*`
  attribute selectors don't reliably follow it across the portal. The BEM
  class names (`whats-new__*`) are unique enough not to need scoping. Same
  constraint and mitigation as `PRismTooltip`.
-->
<style>
.whats-new__overlay {
  position: fixed;
  inset: 0;
  background: rgb(0 0 0 / 0.5);
  /* Same z-index ceiling as ReauthDialog so the surface always wins focus. */
  z-index: 80;
  animation: whats-new-fade-in 0.14s ease-out;
}

.whats-new__overlay[data-state="closed"] {
  animation: whats-new-fade-out 0.14s ease-in;
}

.whats-new {
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  width: min(540px, calc(100vw - 32px));
  max-height: 70vh;
  background: var(--bg-1);
  border: 1px solid var(--border-1);
  border-radius: var(--r-3);
  box-shadow: var(--shadow-3);
  z-index: 90;
  display: flex;
  flex-direction: column;
  animation: whats-new-pop-in 0.16s ease-out;
}

.whats-new[data-state="closed"] {
  animation: whats-new-pop-out 0.12s ease-in;
}

.whats-new__header {
  padding: var(--s-5) var(--s-5) var(--s-3);
  border-bottom: 1px solid var(--border-1);
}

.whats-new__title {
  margin: 0;
  font-size: var(--fs-14);
  font-weight: 600;
  color: var(--text-strong);
  letter-spacing: -0.2px;
}

.whats-new__body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: var(--s-4) var(--s-5);
}

.whats-new__foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-2);
  padding: var(--s-3) var(--s-5) var(--s-5);
  border-top: 1px solid var(--border-1);
}

@keyframes whats-new-fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}

@keyframes whats-new-fade-out {
  from { opacity: 1; }
  to { opacity: 0; }
}

@keyframes whats-new-pop-in {
  from {
    opacity: 0;
    transform: translate(-50%, calc(-50% + 6px));
  }
  to {
    opacity: 1;
    transform: translate(-50%, -50%);
  }
}

@keyframes whats-new-pop-out {
  from {
    opacity: 1;
    transform: translate(-50%, -50%);
  }
  to {
    opacity: 0;
    transform: translate(-50%, calc(-50% + 6px));
  }
}
</style>
