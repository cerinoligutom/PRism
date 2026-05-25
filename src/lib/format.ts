/**
 * Shared formatting helpers for relative timestamps, durations, and the
 * deterministic avatar palette / initials used across the dashboard row and
 * the conversation surface.
 */

const SECOND = 1;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;

/** Em-dash placeholder used when a numeric stat is null. */
export const EM_DASH = "—";

/** Now in unix seconds. Pulled out so tests / fixtures can swap the clock. */
export function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

/** Seconds since the given unix timestamp, clamped to zero. */
export function secondsSince(unixSeconds: number): number {
  return Math.max(0, nowSeconds() - unixSeconds);
}

/**
 * Render a duration in seconds as a short relative label: `45s`, `12m`,
 * `3h 4m`, `2d 6h`, `8d`. Past a week the hour remainder is dropped — it's
 * noise at that granularity and risks overflowing narrow time columns.
 */
export function formatDuration(seconds: number): string {
  if (seconds < MINUTE) return `${seconds}s`;
  if (seconds < HOUR) return `${Math.floor(seconds / MINUTE)}m`;
  if (seconds < DAY) {
    const hours = Math.floor(seconds / HOUR);
    const remainder = Math.floor((seconds - hours * HOUR) / MINUTE);
    return remainder > 0 ? `${hours}h ${remainder}m` : `${hours}h`;
  }
  const days = Math.floor(seconds / DAY);
  if (seconds >= WEEK) return `${days}d`;
  const remainder = Math.floor((seconds - days * DAY) / HOUR);
  return remainder > 0 ? `${days}d ${remainder}h` : `${days}d`;
}

/**
 * Two-piece variant used by the stat tiles (`2d 4h` is rendered as `2d` +
 * subtle `4h`). Returns `{ value: string, sub: string | null }`; `sub` is
 * null when the duration doesn't have a second unit (e.g. `45s`).
 */
export function formatDurationParts(seconds: number): {
  readonly value: string;
  readonly sub: string | null;
} {
  if (seconds < MINUTE) return { value: `${seconds}s`, sub: null };
  if (seconds < HOUR) return { value: `${Math.floor(seconds / MINUTE)}m`, sub: null };
  if (seconds < DAY) {
    const hours = Math.floor(seconds / HOUR);
    const remainder = Math.floor((seconds - hours * HOUR) / MINUTE);
    return {
      value: `${hours}h`,
      sub: remainder > 0 ? `${remainder}m` : null,
    };
  }
  const days = Math.floor(seconds / DAY);
  if (seconds >= WEEK) return { value: `${days}d`, sub: null };
  const remainder = Math.floor((seconds - days * DAY) / HOUR);
  return {
    value: `${days}d`,
    sub: remainder > 0 ? `${remainder}h` : null,
  };
}

/**
 * "Now" / "12s ago" / "3h ago" / "2d ago" for thread / review timestamps.
 *
 * Pass `nowS` to drive the label off a reactive clock (e.g. `useNowSeconds`)
 * so the rendered string updates as time passes. Omitting it falls back to
 * `Date.now()` for callers that only need a one-shot format.
 */
export function formatRelativeAgo(
  unixSeconds: number | null,
  nowS?: number,
): string {
  if (unixSeconds === null) return EM_DASH;
  const reference = nowS ?? nowSeconds();
  const elapsed = Math.max(0, reference - unixSeconds);
  if (elapsed < 10) return "now";
  return `${formatDuration(elapsed)} ago`;
}

/** Login -> two-letter initials. Mirrors `ReviewerStack.initials`. */
export function initials(login: string): string {
  if (login.length === 0) return "?";
  const cleaned = login.replace(/^[-_]+|[-_]+$/g, "");
  const parts = cleaned.split(/[-_]+/).filter((p) => p.length > 0);
  if (parts.length === 0) return login.slice(0, 2).toUpperCase();
  if (parts.length === 1) return (parts[0] ?? "").slice(0, 2).toUpperCase();
  const first = (parts[0] ?? "").slice(0, 1);
  const last = (parts[parts.length - 1] ?? "").slice(0, 1);
  return `${first}${last}`.toUpperCase();
}

/**
 * Deterministic avatar palette slot for a login. Returns one of the eight
 * `av-N` CSS classes from `primitives.css`.
 */
export function avatarSeed(login: string): string {
  let hash = 0;
  for (let i = 0; i < login.length; i += 1) {
    hash = (hash * 31 + login.charCodeAt(i)) | 0;
  }
  const slot = (Math.abs(hash) % 8) + 1;
  return `av-${slot}`;
}
