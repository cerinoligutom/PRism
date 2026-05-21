<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";
import DOMPurify, { type Config as DOMPurifyConfig } from "dompurify";
import {
  bundledLanguages,
  getSingletonHighlighter,
  type BundledLanguage,
  type Highlighter,
} from "shiki";

/**
 * Renders GitHub's pre-rendered comment HTML safely and consistently.
 *
 * Three branches:
 *   1. `html` populated -> DOMPurify-sanitise + render via `v-html`, then
 *      lazy-highlight fenced code blocks via Shiki against the
 *      `github-light` / `github-dark` dual-theme palette so the same DOM
 *      tracks the app theme via CSS variables.
 *   2. `html` empty + `fallback` populated -> pre-wrap the plain text. Matches
 *      the legacy `.thread-comment__text` / `.review-card__text` behaviour for
 *      rows persisted before ADR 0014's `body_html` columns landed.
 *   3. Both empty -> render the `empty` slot (default: italic muted "No
 *      content." line).
 *
 * Anchor clicks are intercepted via event delegation and routed through
 * Tauri's `openUrl`; relative URLs resolve against `https://github.com` so
 * mentions / refs land on the right page. See ADR 0014 and issue #138.
 */

interface Props {
  /** GitHub's `bodyHTML`. Null / empty triggers the fallback branch. */
  html?: string | null;
  /** Plain-text body, rendered with `white-space: pre-wrap` when `html`
   *  is null / empty. Lets legacy rows degrade gracefully. */
  fallback?: string | null;
}

const props = withDefaults(defineProps<Props>(), {
  html: null,
  fallback: null,
});

const HTML_EMPTY = (value: string | null | undefined): boolean =>
  value === null || value === undefined || value.trim() === "";

const hasHtml = computed<boolean>(() => !HTML_EMPTY(props.html));
const hasFallback = computed<boolean>(() => !HTML_EMPTY(props.fallback));

// DOMPurify config. Allow the GitHub-specific custom element + task-list
// inputs; block scripts / styles / iframes / event handlers. The default
// allow-list covers most of what `bodyHTML` produces; adding `g-emoji`
// (and the few attrs it carries) brings emoji shortcodes through.
const SANITISE_CONFIG: DOMPurifyConfig = {
  ADD_TAGS: ["g-emoji"],
  ADD_ATTR: [
    // GitHub emoji custom-element attrs
    "alias",
    "fallback-src",
    "tone",
    // Standard markdown attrs DOMPurify allows by default, listed for clarity
    "class",
    "href",
    "src",
    "alt",
    "title",
    "align",
    "width",
    "height",
    // Task-list checkboxes ship `disabled` + `checked`; DOMPurify allows
    // them on `input` by default but list explicitly so config drift is
    // obvious.
    "disabled",
    "checked",
    "type",
  ],
  FORBID_TAGS: ["script", "style", "iframe"],
  // `RETURN_TRUSTED_TYPE: false` keeps the return value a string so the
  // `v-html` binding can swap it in directly.
};

const sanitised = computed<string>(() => {
  if (!hasHtml.value) return "";
  // DOMPurify v3 returns `string | TrustedHTML | ...` depending on the
  // RETURN_TRUSTED_TYPE flag. Default (`false`) returns a plain string;
  // narrow via the explicit return type rather than chasing the union
  // each call site.
  const out = DOMPurify.sanitize(props.html ?? "", SANITISE_CONFIG);
  return typeof out === "string" ? out : String(out);
});

// Container element ref used for: (1) anchor click delegation, (2) walking
// `<pre><code>` blocks to highlight with Shiki, (3) listening for the
// theme-change event so the rendered Shiki CSS variable palette updates.
const rootEl = ref<HTMLDivElement | null>(null);

// Track which languages have been requested so we don't redundantly load
// the grammar from the bundled-language registry.
const loadedLangs = new Set<BundledLanguage>();
let highlighter: Highlighter | null = null;

async function ensureHighlighter(): Promise<Highlighter> {
  if (highlighter !== null) return highlighter;
  highlighter = await getSingletonHighlighter({
    themes: ["github-light", "github-dark"],
    langs: [],
  });
  return highlighter;
}

function normaliseLang(raw: string): BundledLanguage | null {
  // GitHub emits `language-foo` on the inner `<code>` element. Map to a
  // bundled language key; return null when unknown so we skip highlight.
  const key = raw.trim().toLowerCase().replace(/^language-/, "");
  if (key === "") return null;
  return key in bundledLanguages ? (key as BundledLanguage) : null;
}

async function highlightBlocks(root: HTMLElement): Promise<void> {
  const blocks = root.querySelectorAll<HTMLElement>("pre > code[class*='language-']");
  if (blocks.length === 0) return;
  const hl = await ensureHighlighter();
  for (const code of Array.from(blocks)) {
    // The `language-foo` class is the source of truth; fenced code blocks
    // without a language attribute don't enter this loop.
    const langClass = Array.from(code.classList).find((c) => c.startsWith("language-"));
    if (langClass === undefined) continue;
    const lang = normaliseLang(langClass);
    if (lang === null) continue;
    if (!loadedLangs.has(lang)) {
      try {
        await hl.loadLanguage(lang);
        loadedLangs.add(lang);
      } catch {
        // Loading a bundled grammar can still fail (network-isolated
        // sandbox, etc.); leave the block as plain code rather than
        // surface the failure to the user.
        continue;
      }
    }
    const source = code.textContent ?? "";
    if (source === "") continue;
    try {
      const html = hl.codeToHtml(source, {
        lang,
        themes: { light: "github-light", dark: "github-dark" },
        defaultColor: false,
      });
      // Replace the entire `<pre>` so the Shiki wrapper takes over the
      // surrounding markup; the design-system styles target `.shiki` to
      // keep the surface flush with the surrounding card.
      const pre = code.parentElement;
      if (pre !== null && pre.tagName === "PRE") {
        pre.outerHTML = html;
      } else {
        code.outerHTML = html;
      }
    } catch {
      // Same defensive posture: skip on highlight failure rather than
      // disrupting the rest of the comment.
      continue;
    }
  }
}

watch(
  [sanitised, rootEl],
  async ([currentHtml, el]) => {
    if (el === null) return;
    if (currentHtml === "") return;
    await highlightBlocks(el);
  },
  { immediate: true, flush: "post" },
);

// Anchor delegation. Tauri webviews don't follow same-origin links via the
// browser, and even when they do, opening on the host's default browser is
// the desired behaviour for github.com / mention / ref links.
async function onClick(event: MouseEvent): Promise<void> {
  const target = event.target;
  if (!(target instanceof Element)) return;
  const anchor = target.closest("a");
  if (anchor === null) return;
  const href = anchor.getAttribute("href");
  if (href === null || href === "") return;
  event.preventDefault();
  const resolved = resolveHref(href);
  if (resolved === null) return;
  try {
    await openUrl(resolved);
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn("failed to open markdown link", err);
  }
}

function resolveHref(href: string): string | null {
  if (href.startsWith("#")) return null; // intra-document anchors
  if (
    href.startsWith("http://") ||
    href.startsWith("https://") ||
    href.startsWith("mailto:")
  ) {
    return href;
  }
  if (href.startsWith("/")) {
    return `https://github.com${href}`;
  }
  // Relative paths in GitHub comment HTML are typically `./...` or `../...`
  // referencing repo paths. Treat them as repo-relative against github.com;
  // not perfect for fork-cross-references but matches the v1 contract.
  return null;
}

onBeforeUnmount(() => {
  // The singleton highlighter is shared across mounts, so nothing per-instance
  // to dispose here. Anchor handler is registered via the template, no manual
  // teardown needed.
});
</script>

<template>
  <div
    v-if="hasHtml"
    ref="rootEl"
    class="prism-markdown"
    @click="onClick"
    v-html="sanitised"
  />
  <div
    v-else-if="hasFallback"
    class="prism-markdown prism-markdown--fallback-only"
  >
    <span class="prism-markdown__fallback">{{ fallback }}</span>
  </div>
  <div v-else class="prism-markdown prism-markdown--empty">
    <slot name="empty">
      <span class="prism-markdown__empty">No content.</span>
    </slot>
  </div>
</template>

<style scoped>
/* Layout-level hooks only — the typography / colour rules live in the
 * global `markdown.css` so a future call site that wants to scope the
 * primitive elsewhere can opt in without duplicating styles. */
.prism-markdown--fallback-only,
.prism-markdown--empty {
  /* Match `.thread-comment__text` / `.review-card__text` margin so a swap
   * doesn't shift the surrounding layout. */
  margin: 2px 0 0;
}
</style>
