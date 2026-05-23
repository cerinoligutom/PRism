// Sync the SemVer across package.json, src-tauri/Cargo.toml, and
// src-tauri/tauri.conf.json. ADR-0022 documents tauri.conf.json as the
// canonical-reads source for the UI; the three files still have to match
// physically because each toolchain reads its own.
//
// Usage:
//   pnpm bump-version 0.2.0     -> writes 0.2.0 into all three files
//   pnpm check-version          -> CI guard; non-zero exit on drift
//
// Run via `node --experimental-strip-types` (set by the pnpm scripts).

import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");

const PACKAGE_JSON = resolve(ROOT, "package.json");
const CARGO_TOML = resolve(ROOT, "src-tauri/Cargo.toml");
const TAURI_CONF = resolve(ROOT, "src-tauri/tauri.conf.json");

// Bare SemVer for v1.x. Pre-release / build metadata stay out until a real
// rc.N tag forces the conversation; revise SEMVER_RE then.
const SEMVER_RE = /^\d+\.\d+\.\d+$/;

interface CliArgs {
  readonly target: string;
  readonly check: boolean;
}

function fail(msg: string): never {
  console.error(`bump-version: ${msg}`);
  process.exit(2);
}

function parseArgs(argv: readonly string[]): CliArgs {
  const tokens = argv.slice(2);
  const positional = tokens.filter((a) => !a.startsWith("--"));
  const flags = new Set(tokens.filter((a) => a.startsWith("--")));
  const check = flags.has("--check");
  const target = positional[0] ?? "";

  if (check && target !== "") {
    fail("--check takes no version argument");
  }
  if (!check && !SEMVER_RE.test(target)) {
    fail("usage: bump-version.ts <X.Y.Z>  |  bump-version.ts --check");
  }
  return { target, check };
}

async function readPackageJsonVersion(): Promise<string> {
  const txt = await readFile(PACKAGE_JSON, "utf8");
  const json = JSON.parse(txt) as { version?: unknown };
  if (typeof json.version !== "string") {
    fail("package.json: version not a string");
  }
  return json.version;
}

async function writePackageJsonVersion(version: string): Promise<boolean> {
  // String-replace so key order + trailing newline + formatting stay
  // byte-stable. JSON.stringify would re-flow the whole file.
  const txt = await readFile(PACKAGE_JSON, "utf8");
  const replaced = txt.replace(
    /(^\s*"version"\s*:\s*")[^"]+(")/m,
    (_match, p1: string, p2: string) => `${p1}${version}${p2}`,
  );
  if (replaced === txt) return false;
  await writeFile(PACKAGE_JSON, replaced, "utf8");
  return true;
}

async function readCargoVersion(): Promise<string> {
  const txt = await readFile(CARGO_TOML, "utf8");
  // Anchored to a line start so the [package] version doesn't pick up
  // dependency `version = "..."` lines.
  const match = txt.match(/^version\s*=\s*"([^"]+)"\s*$/m);
  if (match === null) fail("Cargo.toml: no top-level version key");
  return match[1];
}

async function writeCargoVersion(version: string): Promise<boolean> {
  const txt = await readFile(CARGO_TOML, "utf8");
  const replaced = txt.replace(
    /^(version\s*=\s*")[^"]+(")/m,
    (_match, p1: string, p2: string) => `${p1}${version}${p2}`,
  );
  if (replaced === txt) return false;
  await writeFile(CARGO_TOML, replaced, "utf8");
  return true;
}

async function readTauriConfVersion(): Promise<string> {
  const txt = await readFile(TAURI_CONF, "utf8");
  const json = JSON.parse(txt) as { version?: unknown };
  if (typeof json.version !== "string") {
    fail("tauri.conf.json: version not a string");
  }
  return json.version;
}

async function writeTauriConfVersion(version: string): Promise<boolean> {
  const txt = await readFile(TAURI_CONF, "utf8");
  const replaced = txt.replace(
    /(^\s*"version"\s*:\s*")[^"]+(")/m,
    (_match, p1: string, p2: string) => `${p1}${version}${p2}`,
  );
  if (replaced === txt) return false;
  await writeFile(TAURI_CONF, replaced, "utf8");
  return true;
}

async function runCheck(): Promise<never> {
  const [pkg, cargo, tauri] = await Promise.all([
    readPackageJsonVersion(),
    readCargoVersion(),
    readTauriConfVersion(),
  ]);
  if (pkg === cargo && cargo === tauri) {
    console.log(`bump-version: all three at v${pkg}`);
    process.exit(0);
  }
  console.error("bump-version: version drift detected");
  console.error(`  package.json:           v${pkg}`);
  console.error(`  src-tauri/Cargo.toml:   v${cargo}`);
  console.error(`  src-tauri/tauri.conf:   v${tauri}`);
  process.exit(1);
}

async function runSet(target: string): Promise<never> {
  const [pkgChanged, cargoChanged, tauriChanged] = await Promise.all([
    writePackageJsonVersion(target),
    writeCargoVersion(target),
    writeTauriConfVersion(target),
  ]);

  const changes: string[] = [];
  if (pkgChanged) changes.push("package.json");
  if (cargoChanged) changes.push("src-tauri/Cargo.toml");
  if (tauriChanged) changes.push("src-tauri/tauri.conf.json");

  if (changes.length === 0) {
    console.log(`bump-version: already at v${target}`);
    process.exit(0);
  }
  console.log(`bump-version: wrote v${target} -> ${changes.join(", ")}`);
  process.exit(0);
}

const { target, check } = parseArgs(process.argv);
if (check) {
  await runCheck();
} else {
  await runSet(target);
}
