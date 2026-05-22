<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  bundledLanguages,
  getSingletonHighlighter,
  type BundledLanguage,
  type Highlighter,
} from "shiki";

/**
 * Renders the unified-diff hunk GitHub attaches to inline review comments.
 *
 * The hunk lives on `review_threads.diff_hunk`, hydrated lazily by
 * `fetch_pr_conversation` from the head comment's `diffHunk`. The renderer:
 *  - Parses the `@@ -A,B +C,D @@` header to derive old / new line numbers.
 *  - Splits the body into rows tagged `addition`, `deletion`, `context`,
 *    `header`, or `noNewline`.
 *  - Highlights each row's code via Shiki against the dual-theme palette
 *    (`github-light` / `github-dark`) so the same DOM tracks the app theme
 *    via the existing `--shiki-light` / `--shiki-dark` CSS variables that
 *    `markdown.css` already wires up.
 *  - Picks the Shiki grammar from the thread's `path` extension; unknown
 *    extensions fall back to plain text (no highlighting, layout intact).
 *
 * Out of scope per issue #162:
 *  - Highlighting which line within the hunk the thread is about (the line
 *    range chip on the thread row already conveys this).
 *  - Inline reply UX (read-only v1).
 *  - Full-file context (only the diff-hunk GitHub gives us).
 */

interface Props {
  /** Unified-diff hunk text, e.g.
   *  `@@ -1,3 +1,4 @@\n  fn main() {\n-    println!("hi");\n+    println!("hello");`. */
  hunk: string;
  /** Thread file path, used to infer the Shiki grammar. `null` when GitHub
   *  doesn't carry one for the thread (rare; defaults to no highlight). */
  path?: string | null;
}

const props = withDefaults(defineProps<Props>(), {
  path: null,
});

type DiffRowKind = "header" | "addition" | "deletion" | "context" | "noNewline";

interface DiffRow {
  readonly kind: DiffRowKind;
  /** Old-file line number; null for additions and headers. */
  readonly oldLine: number | null;
  /** New-file line number; null for deletions and headers. */
  readonly newLine: number | null;
  /** The row's content without the leading prefix character. Headers keep
   *  their `@@ ... @@` prefix; `noNewline` rows keep the `\` marker text. */
  readonly text: string;
  /** Pre-rendered HTML once Shiki has highlighted the row; `null` when the
   *  block falls back to plain text (no grammar matched or highlight failed). */
  highlighted: string | null;
}

const HUNK_HEADER_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/**
 * Convert the unified-diff body into a row list with derived line numbers.
 * Per the unified-diff spec the very first line is the `@@ -X,Y +A,B @@`
 * header; remaining lines start with one of:
 *   - `+` addition (new-file only)
 *   - `-` deletion (old-file only)
 *   - ` ` context (both files)
 *   - `\` "No newline at end of file" marker (between either block)
 * Multi-hunk strings are unusual for an inline review comment but the
 * parser handles them by resetting the line counters on each header it
 * encounters.
 */
function parseHunk(raw: string): DiffRow[] {
  const lines = raw.split("\n");
  const rows: DiffRow[] = [];
  let oldLine = 0;
  let newLine = 0;
  for (const line of lines) {
    if (line.length === 0) continue;
    const headerMatch = HUNK_HEADER_RE.exec(line);
    if (headerMatch !== null) {
      // Capture groups 1 and 3 are required in the regex (`(\d+)`), so the
      // match always carries them; narrow via `?? "0"` to satisfy the TS
      // signature without a non-null assertion.
      oldLine = Number.parseInt(headerMatch[1] ?? "0", 10);
      newLine = Number.parseInt(headerMatch[3] ?? "0", 10);
      rows.push({
        kind: "header",
        oldLine: null,
        newLine: null,
        text: line,
        highlighted: null,
      });
      continue;
    }
    const prefix = line.charAt(0);
    const body = line.slice(1);
    if (prefix === "+") {
      rows.push({
        kind: "addition",
        oldLine: null,
        newLine,
        text: body,
        highlighted: null,
      });
      newLine += 1;
      continue;
    }
    if (prefix === "-") {
      rows.push({
        kind: "deletion",
        oldLine,
        newLine: null,
        text: body,
        highlighted: null,
      });
      oldLine += 1;
      continue;
    }
    if (prefix === " ") {
      rows.push({
        kind: "context",
        oldLine,
        newLine,
        text: body,
        highlighted: null,
      });
      oldLine += 1;
      newLine += 1;
      continue;
    }
    if (prefix === "\\") {
      // `\ No newline at end of file`. Track but don't advance counters.
      rows.push({
        kind: "noNewline",
        oldLine: null,
        newLine: null,
        text: line,
        highlighted: null,
      });
    }
  }
  return rows;
}

// Common path extension -> Shiki bundled language name. Kept as a small
// allow-list rather than an exhaustive map; unknown extensions fall back to
// plain text rendering (highlight skipped, layout intact). The keys lean on
// the bundled-language identifiers Shiki accepts directly.
const EXTENSION_TO_LANG: Readonly<Record<string, BundledLanguage>> = {
  ts: "typescript",
  tsx: "tsx",
  js: "javascript",
  jsx: "jsx",
  vue: "vue",
  rs: "rust",
  py: "python",
  rb: "ruby",
  go: "go",
  java: "java",
  kt: "kotlin",
  swift: "swift",
  c: "c",
  h: "c",
  cc: "cpp",
  cpp: "cpp",
  hpp: "cpp",
  cs: "csharp",
  php: "php",
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  fish: "fish",
  json: "json",
  jsonc: "jsonc",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  sql: "sql",
  css: "css",
  scss: "scss",
  html: "html",
  xml: "xml",
  md: "markdown",
  mdx: "mdx",
  graphql: "graphql",
  gql: "graphql",
  dockerfile: "dockerfile",
};

function detectLang(path: string | null): BundledLanguage | null {
  if (path === null || path.length === 0) return null;
  const segments = path.split(/[\\/]/);
  const filename = segments[segments.length - 1] ?? "";
  if (filename.toLowerCase() === "dockerfile") return "dockerfile";
  const lastDot = filename.lastIndexOf(".");
  if (lastDot <= 0) return null;
  const ext = filename.slice(lastDot + 1).toLowerCase();
  const candidate = EXTENSION_TO_LANG[ext];
  if (candidate === undefined) return null;
  return candidate in bundledLanguages ? candidate : null;
}

const lang = computed<BundledLanguage | null>(() => detectLang(props.path));

const rows = ref<readonly DiffRow[]>(parseHunk(props.hunk));

let highlighter: Highlighter | null = null;
const loadedLangs = new Set<BundledLanguage>();

async function ensureHighlighter(): Promise<Highlighter> {
  if (highlighter !== null) return highlighter;
  highlighter = await getSingletonHighlighter({
    themes: ["github-light", "github-dark"],
    langs: [],
  });
  return highlighter;
}

/** Extract the `<code>` inner HTML from Shiki's `<pre><code>` wrapper so the
 * row layout (two-column gutter + content) stays under our control. Shiki
 * emits a single-line `<pre class="shiki"><code>...</code></pre>` for a
 * single-line `codeToHtml` call; we pull the inner content via a tiny DOM
 * parse, which sidesteps brittle regex on highlighter output. Returns null
 * when the parse can't find the expected structure. */
function extractInnerCode(shikiHtml: string): string | null {
  const doc = new DOMParser().parseFromString(shikiHtml, "text/html");
  const code = doc.querySelector("pre.shiki code");
  if (code === null) return null;
  // Shiki wraps each code line in `<span class="line">...</span>`. For a
  // single-line input there's exactly one such span; lifting its innerHTML
  // gives us just the highlighted tokens without the wrapping shell.
  const line = code.querySelector("span.line");
  if (line !== null) return line.innerHTML;
  return code.innerHTML;
}

async function highlightRows(): Promise<void> {
  if (lang.value === null) {
    // No language match - leave every row's `highlighted` as null and the
    // template falls back to plain text rendering.
    return;
  }
  const hl = await ensureHighlighter();
  if (!loadedLangs.has(lang.value)) {
    try {
      await hl.loadLanguage(lang.value);
      loadedLangs.add(lang.value);
    } catch {
      return;
    }
  }
  // Highlight each non-header row's text in isolation. Per-row highlighting
  // matches the row backgrounds GitHub uses; whole-block highlighting would
  // need a second pass to re-segment the output back into rows.
  const next: DiffRow[] = rows.value.map((row) => {
    if (row.kind === "header" || row.kind === "noNewline") return { ...row };
    if (row.text.length === 0) return { ...row };
    try {
      const html = hl.codeToHtml(row.text, {
        lang: lang.value as BundledLanguage,
        themes: { light: "github-light", dark: "github-dark" },
        defaultColor: false,
      });
      const inner = extractInnerCode(html);
      return { ...row, highlighted: inner };
    } catch {
      return { ...row };
    }
  });
  rows.value = next;
}

watch(
  () => props.hunk,
  (next) => {
    rows.value = parseHunk(next);
    void highlightRows();
  },
  { immediate: true, flush: "post" },
);

watch(
  () => props.path,
  () => {
    void highlightRows();
  },
  { flush: "post" },
);

function rowAriaLabel(row: DiffRow): string {
  switch (row.kind) {
    case "addition":
      return `added line ${row.newLine ?? ""}`;
    case "deletion":
      return `removed line ${row.oldLine ?? ""}`;
    case "context":
      return `context line ${row.newLine ?? ""}`;
    case "header":
      return "diff hunk header";
    case "noNewline":
      return row.text;
  }
}
</script>

<template>
  <div
    class="diff-hunk-block"
    role="figure"
    aria-label="Diff hunk for this thread"
  >
    <div
      v-for="(row, idx) in rows"
      :key="idx"
      :class="['diff-hunk-block__row', `diff-hunk-block__row--${row.kind}`]"
      :aria-label="rowAriaLabel(row)"
    >
      <span class="diff-hunk-block__gutter diff-hunk-block__gutter--old" aria-hidden="true">
        <template v-if="row.oldLine !== null">{{ row.oldLine }}</template>
      </span>
      <span class="diff-hunk-block__gutter diff-hunk-block__gutter--new" aria-hidden="true">
        <template v-if="row.newLine !== null">{{ row.newLine }}</template>
      </span>
      <span
        class="diff-hunk-block__sign"
        aria-hidden="true"
      >{{ row.kind === 'addition' ? '+' : row.kind === 'deletion' ? '-' : ' ' }}</span><code
        v-if="row.highlighted !== null"
        class="diff-hunk-block__code"
        v-html="row.highlighted"
      /><code
        v-else
        class="diff-hunk-block__code"
      >{{ row.text }}</code>
    </div>
  </div>
</template>

<style scoped>
.diff-hunk-block {
  margin: var(--s-2) 0 var(--s-3);
  padding: 0;
  border: 1px solid var(--border-1);
  border-radius: var(--r-2);
  background: var(--bg-2);
  overflow: hidden;
  font-family: var(--font-mono);
  font-size: var(--fs-11);
  line-height: 1.5;
  color: var(--text);
}

.diff-hunk-block__row {
  display: grid;
  grid-template-columns: 36px 36px 14px 1fr;
  align-items: baseline;
  column-gap: 0;
  padding: 0;
  white-space: pre;
}

.diff-hunk-block__row--header {
  background: var(--bg-3);
  color: var(--text-mute);
  padding: 2px var(--s-2);
  /* The header collapses to a single muted strip - no gutter columns. */
  display: block;
}

.diff-hunk-block__row--addition {
  background: oklch(from var(--success) l c h / 0.14);
}

.diff-hunk-block__row--deletion {
  background: oklch(from var(--danger) l c h / 0.14);
}

.diff-hunk-block__row--context {
  background: transparent;
}

.diff-hunk-block__row--noNewline {
  background: var(--bg-3);
  color: var(--text-mute);
  font-style: italic;
  display: block;
  padding: 2px var(--s-2);
}

.diff-hunk-block__gutter {
  padding: 0 var(--s-2) 0 0;
  text-align: right;
  color: var(--text-faint);
  user-select: none;
  font-variant-numeric: tabular-nums;
}

.diff-hunk-block__gutter--old {
  border-right: 1px solid var(--border-1);
}

.diff-hunk-block__sign {
  text-align: center;
  color: var(--text-mute);
  user-select: none;
}

.diff-hunk-block__row--addition .diff-hunk-block__sign {
  color: var(--success);
}

.diff-hunk-block__row--deletion .diff-hunk-block__sign {
  color: var(--danger);
}

.diff-hunk-block__code {
  padding-right: var(--s-2);
  overflow-x: auto;
  word-break: normal;
  color: var(--text);
  /* Preserve significant whitespace in the source line (indentation,
   * runs of spaces) without wrapping. Browsers render `<code>` with
   * `white-space: normal` by default which collapses tab + double-space. */
  white-space: pre;
  font-family: inherit;
  background: transparent;
}
</style>

<!-- Shiki emits tokens with inline `--shiki-light` / `--shiki-dark` CSS
     variables (per `defaultColor: false`). The theme switch needs a rule
     that selects `<html data-theme="dark">` directly, which Vue's scoped
     attribute can't reach. Mirrors the pattern `markdown.css` uses for
     `.prism-markdown .shiki span`. -->
<style>
.diff-hunk-block__code span {
  color: var(--shiki-light);
  background: transparent;
}

:root[data-theme="dark"] .diff-hunk-block__code span {
  color: var(--shiki-dark);
}
</style>
