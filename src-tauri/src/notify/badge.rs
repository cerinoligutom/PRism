//! Cross-platform unread-PR badge (ADR 0017 decision 3, plus the post-v1
//! Windows + Linux follow-up in issue #330).
//!
//! Two surfaces:
//!
//! * [`update_badge`] - the cross-platform entry point. Reads the count via
//!   [`count_needs_attention_global`] and dispatches to the platform-specific
//!   `apply_badge`: macOS dock badge, Windows taskbar overlay icon, Linux
//!   Unity launcher D-Bus signal. Targets without a known affordance no-op.
//! * [`BadgeSink`] / [`AppHandleBadge`] - the trait + production impl the sync
//!   worker holds inside `WorkerContext`. Mirrors `EmitSink` and
//!   `ReauthNotifier` so unit tests can capture refresh calls without booting
//!   Tauri.
//!
//! The count source is the `needs_attention` roll-up (ADR 0031): every open,
//! unarchived PR with at least one relation whose `needs_attention = 1`
//! contributes one. The badge equals the row roll-up by construction - it is
//! the same per-`(account, pr)` boolean the PR-row attention dot and the
//! sidebar per-view dots read, so the three surfaces can never disagree
//! (resolving ADR 0031's symptom 5). It is account-agnostic: multi-account
//! viewers see one number, and a PR visible from two accounts that needs
//! attention on either counts once (`DISTINCT pr.id`).
//!
//! Why the roll-up, not the unread predicate: under ADR 0031 attention is a
//! conversation-unit watermark model, not a blunt "PR changed since I last
//! opened it" flag. The roll-up clears only on a deliberate mark-seen or a
//! genuine reply, and a role obligation (requested reviewer / CHANGES_REQUESTED
//! on your PR) keeps it lit even after you've read it - the case the unread
//! predicate got wrong. Binding the badge to the roll-up is what lets the dock,
//! the row dot, and the sidebar dots track one definition.
//!
//! Trigger surfaces:
//!
//! * The sync worker calls [`BadgeSink::refresh`] once per cycle, after the
//!   auto-archive sweep, so per-account fan-out and the archive sweep both
//!   feed into the same post-cycle update.
//! * The triage write commands (`mark_pr_read`, `mark_pr_unread`,
//!   `mark_pr_archived`, `mark_pr_unarchived`, `mark_view_read`) and the
//!   conversation hydrator's auto-mark-on-open call [`refresh_from_db`]
//!   after their commit so the dock reflects the change without waiting
//!   for the next sync tick.

use rusqlite::Connection;
use tauri::{AppHandle, Runtime};

use crate::db::DbHandle;

/// Hard cap for the on-badge number. macOS renders inside a small circle;
/// anything past three digits is illegible. Counts beyond the cap clamp to
/// 999 so the badge keeps a fixed-width number rather than growing arbitrarily.
/// The Windows tile renderer applies a tighter "99+" ceiling because the
/// 16x16 taskbar overlay only fits two glyphs.
pub(crate) const BADGE_MAX: i64 = 999;

/// Push `count` onto the OS badge surface for the current platform. `count == 0`
/// clears the badge. Targets without a badge affordance no-op.
///
/// Counts above [`BADGE_MAX`] are clamped to 999. Each platform's `apply_badge`
/// downstream formats from there (macOS leaves it to the dock, Windows folds
/// counts past 99 to a "99+" tile, Linux passes the raw integer to the
/// launcher receiver, which formats it).
///
/// Failures inside the Tauri call or the D-Bus emit are logged and swallowed -
/// the badge is a convenience signal and never blocks the sync loop or a
/// triage command.
pub fn update_badge<R: Runtime>(app: &AppHandle<R>, count: i64) {
    apply_badge(app, count.clamp(0, BADGE_MAX));
}

/// Wrap the `Manager::get_webview_window` + `set_badge_count` call inside the
/// cfg gate so non-macOS builds don't carry the syscall.
#[cfg(target_os = "macos")]
fn apply_badge<R: Runtime>(app: &AppHandle<R>, count: i64) {
    use tauri::Manager;
    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("badge: main webview window missing, skipping update");
        return;
    };
    // `Some(0)` and `None` both clear the badge per Tauri's docs; pass `None`
    // explicitly so the intent is legible in stack traces.
    let payload = if count > 0 { Some(count) } else { None };
    if let Err(err) = window.set_badge_count(payload) {
        tracing::warn!(%err, "badge: set_badge_count failed");
    }
}

#[cfg(target_os = "windows")]
fn apply_badge<R: Runtime>(app: &AppHandle<R>, count: i64) {
    use tauri::Manager;
    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("badge: main webview window missing, skipping update");
        return;
    };
    let image = if count > 0 {
        Some(windows::render_overlay_tile(count))
    } else {
        None
    };
    if let Err(err) = window.set_overlay_icon(image) {
        tracing::warn!(%err, "badge: set_overlay_icon failed");
    }
}

#[cfg(target_os = "linux")]
fn apply_badge<R: Runtime>(_app: &AppHandle<R>, count: i64) {
    // Best-effort. No D-Bus session, no Unity launcher consumer, or a Linux
    // desktop that ignores the signal all land on the silent no-op path -
    // ADR 0017 decision 3's "documented gap" stays honest. We still attempt
    // the emit on every refresh; the failure surface is a single trace log.
    if let Err(err) = linux::emit_launcher_update(count) {
        tracing::debug!(%err, "badge: linux launcher emit failed (likely no D-Bus session)");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn apply_badge<R: Runtime>(_app: &AppHandle<R>, _count: i64) {
    // Other targets (iOS / Android / BSD) have no v1 badge story.
}

/// Read the global count from `db` and push it onto the dock. The Tauri
/// command surface (`mark_pr_read`, `mark_pr_unread`, `mark_pr_archived`,
/// `mark_pr_unarchived`, `mark_view_read`, and the conversation hydrator's
/// auto-mark-on-open) call this once per write so the dock reflects the
/// change without waiting for the next sync cycle (ADR 0017 decision 3).
///
/// Errors at every step log and continue - the badge is a convenience signal
/// that should never propagate a failure into a triage command's return path.
pub fn refresh_from_db<R: Runtime>(app: &AppHandle<R>, db: &DbHandle) {
    let count = match db.lock() {
        Ok(conn) => count_needs_attention_global(&conn).unwrap_or_else(|err| {
            tracing::error!(%err, "badge: count_needs_attention_global failed");
            0
        }),
        Err(err) => {
            tracing::error!(%err, "badge: db poisoned");
            return;
        }
    };
    update_badge(app, count);
}

/// Count the global needs-attention total used by the dock badge (ADR 0031).
///
/// DISTINCT over `pull_requests.id` so a PR that needs attention on two
/// accounts contributes one - the badge counts PRs (analogous to Slack
/// channels), not relation rows. Excludes archived rows (ADR 0018 decision 5)
/// and closed / merged PRs (only open work contributes to the attention
/// signal).
///
/// The predicate is the roll-up boolean `rel.needs_attention = 1`, the same
/// per-`(account, pr)` value the PR-row attention dot and the sidebar per-view
/// dots ([`crate::triage::query::count_sidebar_attention`]) read. The sync
/// worker recomputes that column each cycle via the shared
/// [`needs_attention_case_expr`](crate::triage::query) builder, so the badge,
/// the row, and the sidebar agree by construction.
pub fn count_needs_attention_global(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(DISTINCT pr.id)
           FROM pull_requests pr
           JOIN pull_request_viewer_relations rel
             ON rel.pull_request_id = pr.id
          WHERE rel.archived_at IS NULL
            AND pr.state = 'open'
            AND rel.needs_attention = 1",
        [],
        |row| row.get::<_, i64>(0),
    )
}

/// Fire-and-forget surface the sync worker calls into. Mirrors `EmitSink` /
/// `ReauthNotifier`: no `Result`, no async, no boxed futures - a failed
/// badge update logs and continues so the loop never stalls.
pub trait BadgeSink: Send + Sync {
    /// Recompute the global count and push it onto the dock.
    fn refresh(&self);
}

/// Production [`BadgeSink`] wired to a Tauri `AppHandle` + shared
/// [`DbHandle`]. Hands the count straight to [`update_badge`].
pub struct AppHandleBadge<R: Runtime> {
    handle: AppHandle<R>,
    db: DbHandle,
}

impl<R: Runtime> AppHandleBadge<R> {
    pub fn new(handle: AppHandle<R>, db: DbHandle) -> Self {
        Self { handle, db }
    }
}

impl<R: Runtime> BadgeSink for AppHandleBadge<R> {
    fn refresh(&self) {
        refresh_from_db(&self.handle, &self.db);
    }
}

#[cfg(target_os = "windows")]
mod windows {
    //! 16x16 RGBA overlay tile rendered at runtime for the Windows taskbar.
    //!
    //! Tauri's `set_overlay_icon` takes an [`Image`] (RGBA, top-to-bottom,
    //! row-major). The Windows taskbar paints the overlay at 16x16 over the
    //! app's existing taskbar icon. We compose the tile in three passes:
    //!
    //! 1. A filled accent-coloured circle inscribed in the 16x16 square,
    //!    matching the macOS dock badge's visual weight.
    //! 2. A label - one or two digits, or "9+" for counts >= 100 - centred
    //!    inside the circle. Glyphs are 3x5 bitmaps from `GLYPH_3X5`,
    //!    blitted as 1:1 pixels in white.
    //! 3. Edge antialias is skipped intentionally - at 16x16 a hand-drawn
    //!    aliased circle reads as crisp on Windows' nearest-neighbour
    //!    scaling. Adding subpixel blending here would require a font /
    //!    raster crate for a single use site.
    //!
    //! The whole tile is heap-allocated per call (16 * 16 * 4 = 1024 bytes);
    //! `set_overlay_icon` happens at most a few times per minute (sync cycle
    //! + triage writes), so caching the tiles per count would save
    //! microseconds for no perceptible win.
    use tauri::image::Image;

    const TILE: u32 = 16;
    /// Accent colour: PRism's `--accent` token, baked to sRGB. Tile rendering
    /// can't reach the runtime token store, and the overlay sits outside the
    /// webview's CSS context anyway.
    const ACCENT: [u8; 4] = [0xE5, 0x4C, 0x4C, 0xFF];
    const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

    /// Compact 3x5 bitmap glyphs for the digits 0-9 plus '+'. Each entry is
    /// five rows of three bits; bit-2 (mask 0b100) is the leftmost column.
    /// Designed for legibility at native size; no antialias needed.
    const GLYPH_3X5: [(char, [u8; 5]); 11] = [
        ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
        ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
        ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
        ('3', [0b111, 0b001, 0b111, 0b001, 0b111]),
        ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
        ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
        ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
        ('7', [0b111, 0b001, 0b010, 0b010, 0b010]),
        ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
        ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
        ('+', [0b000, 0b010, 0b111, 0b010, 0b000]),
    ];

    /// Render the overlay tile for `count`. `count` is the clamped value from
    /// [`super::update_badge`]; callers never reach this with `count == 0`.
    pub(super) fn render_overlay_tile(count: i64) -> Image<'static> {
        // The 16x16 overlay only fits two glyphs; counts past 99 fold to "9+".
        // Mirrors the Slack / Discord taskbar convention at this tile size.
        let label = format_label(count);
        let mut rgba = vec![0u8; (TILE * TILE * 4) as usize];
        draw_circle(&mut rgba, ACCENT);
        draw_label(&mut rgba, &label, WHITE);
        Image::new_owned(rgba, TILE, TILE)
    }

    fn format_label(count: i64) -> String {
        if count >= 100 {
            "9+".to_string()
        } else {
            count.to_string()
        }
    }

    /// Filled circle, midpoint-style. Radius 7.5 around (7.5, 7.5) yields an
    /// inscribed disc that fills the tile with a one-pixel transparent margin.
    fn draw_circle(rgba: &mut [u8], colour: [u8; 4]) {
        let radius_sq = 7.5_f32 * 7.5_f32;
        for y in 0..TILE {
            for x in 0..TILE {
                let dx = x as f32 - 7.5;
                let dy = y as f32 - 7.5;
                if dx * dx + dy * dy <= radius_sq {
                    write_pixel(rgba, x, y, colour);
                }
            }
        }
    }

    /// Blit each glyph in `label` one pixel apart, horizontally centred.
    /// Label height is 5px; vertical centre puts the top row at y = 5.
    fn draw_label(rgba: &mut [u8], label: &str, colour: [u8; 4]) {
        let glyph_count = label.chars().count() as u32;
        // 3px per glyph + 1px spacing between glyphs.
        let width = glyph_count * 3 + glyph_count.saturating_sub(1);
        let start_x = (TILE.saturating_sub(width)) / 2;
        let start_y = (TILE - 5) / 2;
        let mut pen_x = start_x;
        for ch in label.chars() {
            if let Some(rows) = glyph_rows(ch) {
                blit_glyph(rgba, pen_x, start_y, rows, colour);
            }
            pen_x += 4; // 3px glyph + 1px spacing
        }
    }

    fn glyph_rows(ch: char) -> Option<&'static [u8; 5]> {
        GLYPH_3X5
            .iter()
            .find_map(|(c, rows)| if *c == ch { Some(rows) } else { None })
    }

    fn blit_glyph(rgba: &mut [u8], origin_x: u32, origin_y: u32, rows: &[u8; 5], colour: [u8; 4]) {
        for (row_idx, row) in rows.iter().enumerate() {
            for col_idx in 0..3u32 {
                let mask = 0b100u8 >> col_idx;
                if row & mask != 0 {
                    write_pixel(rgba, origin_x + col_idx, origin_y + row_idx as u32, colour);
                }
            }
        }
    }

    fn write_pixel(rgba: &mut [u8], x: u32, y: u32, colour: [u8; 4]) {
        if x >= TILE || y >= TILE {
            return;
        }
        let idx = ((y * TILE + x) * 4) as usize;
        rgba[idx..idx + 4].copy_from_slice(&colour);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::notify::badge::BADGE_MAX;

        #[test]
        fn format_label_under_hundred_renders_decimal() {
            assert_eq!(format_label(1), "1");
            assert_eq!(format_label(42), "42");
            assert_eq!(format_label(99), "99");
        }

        #[test]
        fn format_label_clamps_at_hundred_to_nine_plus() {
            assert_eq!(format_label(100), "9+");
            assert_eq!(format_label(500), "9+");
            assert_eq!(format_label(BADGE_MAX), "9+");
        }

        #[test]
        fn render_overlay_tile_produces_16x16_rgba() {
            let image = render_overlay_tile(5);
            assert_eq!(image.width(), 16);
            assert_eq!(image.height(), 16);
            assert_eq!(image.rgba().len(), 16 * 16 * 4);
        }

        #[test]
        fn render_overlay_tile_paints_circle_centre() {
            // The centre pixel sits inside the inscribed circle and is the
            // accent colour, not transparent.
            let image = render_overlay_tile(1);
            let rgba = image.rgba();
            let centre_idx = ((8 * 16 + 8) * 4) as usize;
            assert_eq!(&rgba[centre_idx..centre_idx + 4], &ACCENT);
        }

        #[test]
        fn render_overlay_tile_leaves_corners_transparent() {
            // The 16x16 tile is an inscribed circle; the (0, 0) corner sits
            // outside the radius and must remain RGBA-zero.
            let image = render_overlay_tile(1);
            let rgba = image.rgba();
            assert_eq!(&rgba[0..4], &[0u8, 0, 0, 0]);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    //! Unity LauncherEntry D-Bus signal emission.
    //!
    //! The session-bus signal `com.canonical.Unity.LauncherEntry.Update` is
    //! the cross-DE convention for app-icon counts: Unity, KDE Plasma's task
    //! manager, and a handful of GNOME Shell extensions consume it.
    //! Environments without a listener silently drop the signal.
    //!
    //! Object path: `/com/canonical/unity/launcherentry/<crc32(uri)>`.
    //! Launcher receivers compute the same CRC32 over the desktop entry URI
    //! they're tracking and only route signals matching that path. We feed
    //! the URI built from the bundle identifier in `tauri.conf.json`; if the
    //! Linux bundle config changes the desktop file's basename, update
    //! [`DESKTOP_ENTRY_URI`] in lockstep.
    //!
    //! Signal body: `(sa{sv})` per the spec - URI string plus an a{sv} of
    //! properties. We emit `count` (i64) and `count-visible` (bool); the
    //! receiver hides the badge when `count-visible` is false.
    use std::collections::HashMap;
    use zbus::zvariant::Value;

    /// Application URI used by D-Bus launcher receivers to match the signal
    /// against a running app. Derived from `tauri.conf.json`'s `identifier`
    /// (com.cerinoligutom.prism) - if the Linux bundle ever ships a different
    /// `.desktop` basename, update this constant to match.
    const DESKTOP_ENTRY_URI: &str = "application://com.cerinoligutom.prism.desktop";
    const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
    const SIGNAL: &str = "Update";

    /// Emit the launcher Update signal for `count`. Errors propagate so the
    /// caller can log them at debug level; the contract with `apply_badge` is
    /// best-effort, no panics, no retries.
    pub(super) fn emit_launcher_update(count: i64) -> zbus::Result<()> {
        let connection = zbus::blocking::Connection::session()?;
        let path = launcher_object_path(DESKTOP_ENTRY_URI);
        let mut props: HashMap<&str, Value<'_>> = HashMap::new();
        props.insert("count", Value::I64(count));
        props.insert("count-visible", Value::Bool(count > 0));
        connection.emit_signal(
            None::<&str>,
            path.as_str(),
            INTERFACE,
            SIGNAL,
            &(DESKTOP_ENTRY_URI, props),
        )
    }

    /// `/com/canonical/unity/launcherentry/<crc32(uri)>` - the per-app object
    /// path receivers filter on. Computed via `crc32fast` to match the
    /// Electron / GTK conventions widely in the wild.
    fn launcher_object_path(uri: &str) -> String {
        let id = crc32fast::hash(uri.as_bytes());
        format!("/com/canonical/unity/launcherentry/{id}")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn launcher_object_path_is_stable_for_known_uri() {
            // Hand-computed crc32 of "application://com.cerinoligutom.prism.desktop"
            // pins the expected receiver path; if this changes silently the
            // launcher will stop matching our signal.
            let path = launcher_object_path(DESKTOP_ENTRY_URI);
            assert!(
                path.starts_with("/com/canonical/unity/launcherentry/"),
                "path must live under the Unity prefix, got {path}"
            );
            let suffix = path
                .strip_prefix("/com/canonical/unity/launcherentry/")
                .unwrap();
            assert!(
                suffix.parse::<u32>().is_ok(),
                "suffix must be a u32, got {suffix}"
            );
        }

        #[test]
        fn launcher_object_path_differs_per_uri() {
            assert_ne!(
                launcher_object_path("application://a.desktop"),
                launcher_object_path("application://b.desktop")
            );
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the needs-attention count source (ADR 0031). The
    //! Tauri-bound write path is exercised at app run time; here we verify the
    //! badge SQL equals the row roll-up - `rel.needs_attention = 1` over open,
    //! unarchived PRs, DISTINCT by `pr.id`. We seed the boolean directly rather
    //! than re-deriving it through the roll-up builder; that derivation is
    //! covered in `triage::query` tests. The badge's only job is to count the
    //! flagged rows the row dot and the sidebar dots already read.
    use super::*;
    use rusqlite::Connection;

    fn fresh_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::migrate::run(&mut conn).expect("migrations");
        conn
    }

    /// Seed an account / repo / PR / relation row with the supplied shape.
    /// `needs_attention` is the roll-up boolean the badge counts; `archived`
    /// and `state` exercise the open + unarchived gates. A row contributes to
    /// the badge iff `needs_attention = 1 AND state = 'open' AND NOT archived`.
    fn seed_pr(conn: &Connection, pr_id: i64, state: &str, archived: bool, needs_attention: bool) {
        conn.execute_batch(
            "INSERT OR IGNORE INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT OR IGNORE INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');",
        )
        .unwrap();
        let archived_sql = if archived {
            "strftime('%s','now')"
        } else {
            "NULL"
        };
        let attention = i64::from(needs_attention);
        conn.execute_batch(&format!(
            "INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES ({pr_id}, 10, {pr_id}, 't', '{state}', 0, 'bob',
                        0, 1000, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at,
                 needs_attention, archived_at)
                VALUES (1, {pr_id}, 0, {attention}, {archived_sql});"
        ))
        .unwrap();
    }

    #[test]
    fn count_needs_attention_global_returns_zero_on_empty_table() {
        let conn = fresh_conn();
        assert_eq!(count_needs_attention_global(&conn).unwrap(), 0);
    }

    #[test]
    fn count_needs_attention_global_counts_flagged_open_prs() {
        // A relation with `needs_attention = 1` on an open, unarchived PR is
        // exactly what the row attention dot lights, so the badge counts it.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "open", false, true);
        assert_eq!(count_needs_attention_global(&conn).unwrap(), 1);
    }

    #[test]
    fn count_needs_attention_global_excludes_unflagged_prs() {
        // `needs_attention = 0` means no lit unit and no role obligation; the
        // badge must not count a PR the row doesn't flag.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "open", false, false);
        assert_eq!(count_needs_attention_global(&conn).unwrap(), 0);
    }

    #[test]
    fn count_needs_attention_global_excludes_archived_rows() {
        // ADR 0018 decision 5: archived rows do not contribute to any active
        // count, even while `needs_attention` stays 1 on disk.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "open", true, true);
        assert_eq!(count_needs_attention_global(&conn).unwrap(), 0);
    }

    #[test]
    fn count_needs_attention_global_excludes_closed_and_merged_prs() {
        // Only open work feeds the attention signal; a closed/merged PR keeps
        // its flag on disk but drops from the badge.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "closed", false, true);
        seed_pr(&conn, 101, "merged", false, true);
        seed_pr(&conn, 102, "open", false, true);
        assert_eq!(count_needs_attention_global(&conn).unwrap(), 1);
    }

    #[test]
    fn count_needs_attention_global_counts_only_flagged_among_many() {
        // A mix of flagged and unflagged open PRs: only the flagged ones count.
        let conn = fresh_conn();
        seed_pr(&conn, 100, "open", false, true);
        seed_pr(&conn, 101, "open", false, false);
        seed_pr(&conn, 102, "open", false, true);
        assert_eq!(count_needs_attention_global(&conn).unwrap(), 2);
    }

    #[test]
    fn count_needs_attention_global_distincts_across_accounts() {
        // A PR that needs attention on two accounts contributes one - DISTINCT
        // over `pr.id`, matching the row indicator (one dot per PR, not per
        // relation owner).
        let conn = fresh_conn();
        conn.execute_batch(
            "INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (1, 'a', 'github.com', 'alice', 0);
             INSERT INTO accounts (id, label, host, login, created_at)
                VALUES (2, 'b', 'github.com', 'bob', 0);
             INSERT INTO repos (id, account_id, owner, name, visibility)
                VALUES (10, 1, 'owner', 'repo', 'public');
             INSERT INTO pull_requests
                (id, repo_id, number, title, state, is_draft, author_login,
                 created_at, updated_at, base_ref, head_ref)
                VALUES (100, 10, 1, 't', 'open', 0, 'bob',
                        0, 1000, 'main', 'feat');
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at, needs_attention)
                VALUES (1, 100, 0, 1);
             INSERT INTO pull_request_viewer_relations
                (account_id, pull_request_id, relation_observed_at, needs_attention)
                VALUES (2, 100, 0, 1);",
        )
        .unwrap();
        assert_eq!(
            count_needs_attention_global(&conn).unwrap(),
            1,
            "two accounts needing attention on the same PR count as one"
        );
    }
}
