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
 *   `## [Unreleased]`             -> in-flight section, dropped on slice.
 *   `## [X.Y.Z] - YYYY-MM-DD`     -> released section, retained.
 * Anything before the first `## ` heading (preamble / front-matter) is
 * ignored. Trailing reference-link definitions (`[X.Y.Z]: https://...`) are
 * left in the last entry's body where they sit in the source; they render
 * as markdown link references but harmlessly so.
 */

export interface ChangelogEntry {
  /** Semver string as it appears between the `[` and `]`, e.g. `0.4.0`. */
  readonly version: string;
  /** Date as it appears after the ` - ` separator, e.g. `2026-05-23`. */
  readonly date: string;
  /** Markdown body between this heading and the next `## ` heading, trimmed. */
  readonly body: string;
}

const VERSION_HEADING = /^## \[(\d+\.\d+\.\d+(?:[-+][^\]]+)?)\] - (.+)$/;
const UNRELEASED_HEADING = /^## \[Unreleased\]/i;

/**
 * Split a Keep-a-Changelog markdown file into per-version entries. Entries
 * are returned in source order (newest first, matching how the file is
 * maintained); the `[Unreleased]` block is dropped because it doesn't carry
 * a version to compare against. Malformed `## ` headings are skipped.
 */
export function parseChangelog(raw: string): ChangelogEntry[] {
  const lines = raw.split(/\r?\n/);
  const entries: ChangelogEntry[] = [];

  type Cursor = { version: string; date: string; start: number };
  let cursor: Cursor | null = null;

  const flush = (endExclusive: number): void => {
    if (cursor === null) return;
    const bodyLines = lines.slice(cursor.start, endExclusive);
    entries.push({
      version: cursor.version,
      date: cursor.date,
      body: bodyLines.join("\n").trim(),
    });
  };

  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i] ?? "";

    if (UNRELEASED_HEADING.test(line)) {
      flush(i);
      cursor = null;
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
