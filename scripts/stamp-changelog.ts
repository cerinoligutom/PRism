// Promote the [Unreleased] block in CHANGELOG.md to a versioned block
// stamped with today's UTC date, and seed a fresh empty [Unreleased]
// above it.
//
// Usage:
//   pnpm stamp-changelog --version 0.2.0
//
// Run via `node --experimental-strip-types` (set by the pnpm script).
// Exit codes:
//   0 - success
//   1 - drift / conflict (CHANGELOG missing [Unreleased], or the target
//       version is already stamped)
//   2 - invalid CLI arguments

import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const CHANGELOG = resolve(ROOT, "CHANGELOG.md");

// Bare SemVer matching scripts/bump-version.ts. Pre-release / build
// metadata stay out until a real rc.N tag forces the conversation.
const SEMVER_RE = /^\d+\.\d+\.\d+$/;

// The empty block we insert above the freshly-stamped version. Kept here
// so the layout stays diff-stable with the seed in CHANGELOG.md.
const UNRELEASED_BLOCK = `## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

### Security
`;

function fail(msg: string, code: 1 | 2): never {
  console.error(`stamp-changelog: ${msg}`);
  process.exit(code);
}

function parse_args(argv: readonly string[]): string {
  const tokens = argv.slice(2);
  const idx = tokens.indexOf("--version");
  if (idx === -1 || idx === tokens.length - 1) {
    fail("usage: stamp-changelog.ts --version <X.Y.Z>", 2);
  }
  const target = tokens[idx + 1];
  if (!SEMVER_RE.test(target)) {
    fail(`invalid version "${target}": expected X.Y.Z`, 2);
  }
  return target;
}

function today_utc(): string {
  const now = new Date();
  const y = now.getUTCFullYear();
  const m = String(now.getUTCMonth() + 1).padStart(2, "0");
  const d = String(now.getUTCDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

async function stamp(target: string): Promise<never> {
  const txt = await readFile(CHANGELOG, "utf8");

  // Idempotency guard: refuse to stamp a version that already exists as
  // a versioned heading.
  const already = new RegExp(
    `^## \\[${target.replace(/\./g, "\\.")}\\]`,
    "m",
  );
  if (already.test(txt)) {
    fail(`version ${target} already stamped in CHANGELOG.md`, 1);
  }

  // Anchored to a line start. The leading `## [Unreleased]` heading is
  // the only one we touch; later versioned blocks stay untouched. The
  // capture also eats the trailing newline so we can splice a fresh
  // empty block on top without doubling up blank lines.
  const unreleased = /^## \[Unreleased\]\n/m;
  if (!unreleased.test(txt)) {
    fail("CHANGELOG.md has no [Unreleased] heading to promote", 1);
  }

  const stamped = `## [${target}] - ${today_utc()}\n`;
  const replaced = txt.replace(unreleased, `${UNRELEASED_BLOCK}\n${stamped}`);

  await writeFile(CHANGELOG, replaced, "utf8");
  console.log(`stamp-changelog: promoted [Unreleased] -> [${target}] - ${today_utc()}`);
  process.exit(0);
}

const target = parse_args(process.argv);
await stamp(target);
