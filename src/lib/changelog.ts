/**
 * Pure helpers backing the in-app "What's new" dialog (ADR 0025).
 *
 * The dialog ships with the bundled `CHANGELOG.md` content imported via
 * Vite's `?raw` query, parses it into per-version entries, and slices off
 * the sections strictly newer than the user's last-seen cursor. The
 * concatenated body is rendered through `PRismMarkdown`.
 *
 * The parser handles the Keep-a-Changelog 1.1.0 shape that
 * `scripts/stamp-changelog.ts` produces:
 *   `## [Unreleased]`             -> in-flight section. Dropped by default
 *                                    (auto-open `sectionsSince` needs a
 *                                    semver to compare against the cursor);
 *                                    included when the caller opts in via
 *                                    `{ includeUnreleased: true }` for the
 *                                    manual "View changelog" surface (#377).
 *   `## [X.Y.Z] - YYYY-MM-DD`     -> released section, retained.
 * Anything before the first `## ` heading (preamble / front-matter) is
 * ignored. Trailing reference-link definitions (`[X.Y.Z]: https://...`) are
 * left in the last entry's body where they sit in the source; they render
 * as markdown link references but harmlessly so.
 */

/** Sentinel `version` value the manual-open path uses for the Unreleased
 *  entry (when `includeUnreleased: true`). Consumers that compose headings
 *  per entry (notably `WhatsNewDialog`) special-case this string to render
 *  "## Unreleased" instead of "## vUnreleased - ". */
export const UNRELEASED_VERSION = "Unreleased";

export interface ChangelogEntry {
  /** Semver string as it appears between the `[` and `]`, e.g. `0.4.0`.
   *  Equal to [`UNRELEASED_VERSION`] for the Unreleased entry when the
   *  caller opted in via `{ includeUnreleased: true }`. */
  readonly version: string;
  /** Date as it appears after the ` - ` separator, e.g. `2026-05-23`.
   *  Empty string for the Unreleased entry (no date). */
  readonly date: string;
  /** Markdown body between this heading and the next `## ` heading, trimmed. */
  readonly body: string;
}

export interface ParseChangelogOptions {
  /** Include the `## [Unreleased]` block as an entry whose `version` is
   *  [`UNRELEASED_VERSION`] and whose `date` is empty. Off by default so
   *  the auto-open `sectionsSince` path keeps comparing only semver
   *  entries; the manual "View changelog" surface (#377) opts in. An empty
   *  Unreleased body is skipped so a stale heading doesn't render alone
   *  immediately after a release stamp resets the section. */
  includeUnreleased?: boolean;
}

const VERSION_HEADING = /^## \[(\d+\.\d+\.\d+(?:[-+][^\]]+)?)\] - (.+)$/;
const UNRELEASED_HEADING = /^## \[Unreleased\]/i;

/**
 * Split a Keep-a-Changelog markdown file into per-version entries. Entries
 * are returned in source order (newest first, matching how the file is
 * maintained). Pass `{ includeUnreleased: true }` to retain the
 * `## [Unreleased]` block as the first entry; default behaviour drops it
 * because the auto-open slice needs semver-comparable versions. Malformed
 * `## ` headings are skipped.
 */
export function parseChangelog(
  raw: string,
  options?: ParseChangelogOptions,
): ChangelogEntry[] {
  const lines = raw.split(/\r?\n/);
  const entries: ChangelogEntry[] = [];
  const includeUnreleased = options?.includeUnreleased === true;

  type Cursor = { version: string; date: string; start: number };
  let cursor: Cursor | null = null;

  const flush = (endExclusive: number): void => {
    if (cursor === null) return;
    const bodyLines = lines.slice(cursor.start, endExclusive);
    const body = stripEmptySubsections(bodyLines.join("\n").trim());
    // Skip empty Unreleased entries (stamp-changelog leaves a bare heading
    // immediately after a release promotion). A heading with no body would
    // render as a dead section in the manual dialog.
    if (cursor.version === UNRELEASED_VERSION && body === "") return;
    entries.push({
      version: cursor.version,
      date: cursor.date,
      body,
    });
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i] ?? "";

    if (UNRELEASED_HEADING.test(line)) {
      flush(i);
      cursor = includeUnreleased
        ? { version: UNRELEASED_VERSION, date: "", start: i + 1 }
        : null;
      continue;
    }

    const match = line.match(VERSION_HEADING);
    if (match !== null) {
      flush(i);
      const version = match[1] ?? "";
      const date = match[2]?.trim() ?? "";
      cursor = { version, date, start: i + 1 };
      continue;
    }

    // `## ` headings that aren't `[X.Y.Z]` or `[Unreleased]` close the
    // current entry to avoid swallowing unrelated sections. Top-level
    // `# `, `### ` (section subheadings inside a body), and prose lines
    // are left as body content.
    if (line.startsWith("## ") && cursor !== null) {
      flush(i);
      cursor = null;
    }
  }

  flush(lines.length);
  return entries;
}

/**
 * Return changelog entries whose version is strictly newer than
 * `lastSeen` and at most `current`. Returns `[]` when `lastSeen` is `null`
 * (fresh-install path, dialog suppressed) or when the cursor is already at
 * or past the current binary.
 *
 * Versions are compared by SemVer major.minor.patch only; pre-release / build
 * suffixes are ignored for ordering, which matches the cadence of PRism's
 * release process and avoids pulling in a `semver` dep for the few versions
 * the bundled file ever sees.
 */
export function sectionsSince(
  entries: readonly ChangelogEntry[],
  lastSeen: string | null,
  current: string,
): ChangelogEntry[] {
  if (lastSeen === null) return [];
  if (compareSemver(lastSeen, current) >= 0) return [];

  return entries.filter((entry) => {
    return (
      compareSemver(entry.version, lastSeen) > 0 &&
      compareSemver(entry.version, current) <= 0
    );
  });
}

/**
 * Drop `### Subsection` blocks whose body is empty (#377). Keep-a-Changelog
 * sections are formed with the six fixed subheadings (Added / Changed /
 * Deprecated / Removed / Fixed / Security); `stamp-changelog.ts` re-seeds
 * every subheading on a fresh `[Unreleased]` block even when most stay
 * unused, so the rendered dialog would otherwise show bare `Deprecated /
 * Removed / Security` headings with nothing below them. This walks the body
 * line-by-line, groups by `### `, and skips groups whose collected lines
 * trim to the empty string. Lines outside any `### ` group (rare — usually
 * just a stray trailing newline) pass through unchanged.
 */
function stripEmptySubsections(body: string): string {
  if (body === "") return body;
  const lines = body.split(/\r?\n/);
  const out: string[] = [];
  let pendingHeader: string | null = null;
  let pendingLines: string[] = [];

  const flushPending = (): void => {
    if (pendingHeader === null) return;
    const content = pendingLines.join("\n").trim();
    if (content.length > 0) {
      if (out.length > 0) out.push("");
      out.push(pendingHeader);
      out.push("");
      out.push(content);
    }
    pendingHeader = null;
    pendingLines = [];
  };

  for (const line of lines) {
    if (line.startsWith("### ")) {
      flushPending();
      pendingHeader = line;
      continue;
    }
    if (pendingHeader !== null) {
      pendingLines.push(line);
    } else if (line.trim() !== "") {
      // Pre-subheading body content (rare: a paragraph before any `### `).
      // Pass through so it isn't lost.
      out.push(line);
    }
  }
  flushPending();
  return out.join("\n").trim();
}

/**
 * Three-integer SemVer compare. Returns a negative number when `a < b`,
 * zero when equal, positive when `a > b`. Unparseable segments are treated
 * as `0` so a malformed entry sorts low rather than crashing the dialog.
 */
function compareSemver(a: string, b: string): number {
  const pa = parseSegments(a);
  const pb = parseSegments(b);
  for (let i = 0; i < 3; i += 1) {
    const av = pa[i] ?? 0;
    const bv = pb[i] ?? 0;
    if (av !== bv) return av - bv;
  }
  return 0;
}

function parseSegments(version: string): readonly number[] {
  const core = version.split(/[-+]/)[0] ?? version;
  return core.split(".").map((seg) => {
    const n = Number.parseInt(seg, 10);
    return Number.isFinite(n) ? n : 0;
  });
}
