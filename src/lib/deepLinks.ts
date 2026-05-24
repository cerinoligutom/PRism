/**
 * Pure helpers for the `prism://` custom URL scheme (issue #339).
 *
 * The deep-link composable (`src/composables/useDeepLinkRouter.ts`) takes a
 * raw URL string emitted by `tauri-plugin-deep-link`'s `onOpenUrl` / drained
 * from `getCurrent()`, parses it through `parsePrismDeepLink`, and routes
 * the result.
 *
 * URL shape (v1):
 *   `prism://pr/<owner>/<repo>/<number>[?host=<host>]`
 *
 * `<host>` defaults to `github.com` (ADR 0016: v1 only validates github.com;
 * GHES hosts are wired but unvalidated). Anything that doesn't match the
 * `pr/...` shape returns `null` so the caller can drop the URL silently.
 *
 * Parsing rules:
 *  - The scheme must be exactly `prism:` (case-insensitive).
 *  - The path must start with `pr/` and contain exactly three more segments:
 *    owner, repo, number. Extra trailing segments invalidate the URL.
 *  - `number` must be a positive integer.
 *  - `host` defaults to `github.com`. The query parser accepts the first
 *    `host=` occurrence (later duplicates are ignored).
 */

export interface PrCoordinates {
  readonly host: string;
  readonly owner: string;
  readonly repo: string;
  readonly number: number;
}

export type DeepLinkTarget = { readonly kind: "pr"; readonly coords: PrCoordinates };

const DEFAULT_HOST = "github.com";
const PR_SCHEME = "prism:";

/**
 * Parse a raw deep-link URL into a typed target. Returns `null` when the URL
 * doesn't match a supported shape; the caller drops the URL silently in that
 * case rather than throwing - external links shouldn't crash the app.
 */
export function parsePrismDeepLink(raw: string): DeepLinkTarget | null {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol.toLowerCase() !== PR_SCHEME) return null;

  // The host segment lives in `url.host` for `prism://pr/...` URLs because
  // the URL parser interprets the first authority-style token as the host.
  // We reassemble path + host into a single token sequence so the matcher
  // doesn't care which "side" of the `//` carries the verb.
  const hostToken = url.host;
  const pathTokens = url.pathname.split("/").filter((segment) => segment.length > 0);
  const tokens = hostToken.length > 0 ? [hostToken, ...pathTokens] : pathTokens;

  if (tokens.length !== 4 || tokens[0]?.toLowerCase() !== "pr") return null;
  const owner = decodeSegment(tokens[1]);
  const repo = decodeSegment(tokens[2]);
  const number = parsePositiveInt(tokens[3]);
  if (owner === null || repo === null || number === null) return null;

  const queryHost = url.searchParams.get("host");
  const host = normaliseHost(queryHost) ?? DEFAULT_HOST;

  return { kind: "pr", coords: { host, owner, repo, number } };
}

/**
 * Compose the canonical GitHub web URL for a PR coordinate set. The deep-link
 * composable uses this when the PR isn't cached locally so the user still
 * lands somewhere useful (acceptance criterion 4).
 */
export function githubPrUrl(coords: PrCoordinates): string {
  return `https://${coords.host}/${encodeURIComponent(coords.owner)}/${encodeURIComponent(coords.repo)}/pull/${coords.number}`;
}

function decodeSegment(value: string | undefined): string | null {
  if (value === undefined || value.length === 0) return null;
  try {
    const decoded = decodeURIComponent(value);
    if (decoded.length === 0) return null;
    return decoded;
  } catch {
    return null;
  }
}

function parsePositiveInt(value: string | undefined): number | null {
  if (value === undefined) return null;
  if (!/^[0-9]+$/.test(value)) return null;
  const n = Number(value);
  if (!Number.isInteger(n) || n <= 0) return null;
  return n;
}

function normaliseHost(value: string | null): string | null {
  if (value === null) return null;
  const trimmed = value.trim().toLowerCase();
  if (trimmed.length === 0) return null;
  // Reject anything that contains a path / protocol / port. The host is just
  // a DNS-style hostname for the dashboard lookup; a richer authority shape
  // belongs to the post-v1 work in the issue's "out of scope" section.
  if (!/^[a-z0-9.-]+$/.test(trimmed)) return null;
  return trimmed;
}
