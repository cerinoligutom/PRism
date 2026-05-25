//! Activity-feed event helpers used by the cycle to record phase, per-PR,
//! and cycle-level events. Each emits through the [`super::WorkerContext`]'s
//! `ActivityBuffer` and `EmitSink`, mirroring the wire shape the status-bar
//! diagnostics panel consumes.

use std::time::Duration;

use crate::auth::store::Account;
use crate::sync::activity::{
    record as record_activity, ActivityEventBuilder, ActivityKind, ActivityLevel, SyncPhaseLabel,
};

use super::WorkerContext;

/// User-facing phase label. Mirrors the chip ticker's wording in
/// `phaseLabelFor` (src/stores/syncActivity.ts) so the panel row and the
/// chip describe the same frame identically. The internal `as_str()`
/// equivalent stays lowercase for wire/log compatibility.
fn phase_display_label(phase: SyncPhaseLabel) -> &'static str {
    match phase {
        SyncPhaseLabel::Discovery => "Discovering",
        SyncPhaseLabel::Enrichment => "Fetching detail",
        SyncPhaseLabel::Pruning => "Pruning",
    }
}

/// Past-tense phase label used by the `... complete - {summary}` row.
fn phase_completed_label(phase: SyncPhaseLabel) -> &'static str {
    match phase {
        SyncPhaseLabel::Discovery => "Discovery",
        SyncPhaseLabel::Enrichment => "Detail fetch",
        SyncPhaseLabel::Pruning => "Pruning",
    }
}

pub(super) fn emit_activity_cycle_started(ctx: &WorkerContext, account: &Account) {
    let message = format!("Sync started for {}", account.login);
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::CycleStarted,
            message,
        )
        .build(),
    );
}

pub(super) fn emit_activity_phase_started(
    ctx: &WorkerContext,
    account: &Account,
    phase: SyncPhaseLabel,
) {
    let message = match phase {
        SyncPhaseLabel::Discovery => format!("Discovering for {}", account.login),
        SyncPhaseLabel::Enrichment => "Fetching detail".to_string(),
        SyncPhaseLabel::Pruning => "Pruning".to_string(),
    };
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PhaseStarted { phase },
            message,
        )
        .build(),
    );
}

pub(super) fn emit_activity_phase_progress(
    ctx: &WorkerContext,
    account: &Account,
    phase: SyncPhaseLabel,
    current: u32,
    total: u32,
) {
    let label = phase_display_label(phase);
    let message = if total > 0 {
        format!("{label} ({current}/{total})")
    } else {
        format!("{label} ({current})")
    };
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PhaseProgress {
                phase,
                current,
                total,
            },
            message,
        )
        .build(),
    );
}

pub(super) fn emit_activity_pr_fetched(
    ctx: &WorkerContext,
    account: &Account,
    owner: &str,
    name: &str,
    number: i64,
    url: &str,
) {
    let message = format!("Fetched detail for {owner}/{name}#{number}");
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PrFetched {
                number,
                owner: owner.to_string(),
                name: name.to_string(),
                url: url.to_string(),
            },
            message,
        )
        .build(),
    );
}

pub(super) fn emit_activity_pr_skipped_no_change(
    ctx: &WorkerContext,
    account: &Account,
    owner: &str,
    name: &str,
    number: i64,
    url: &str,
) {
    let message = format!("Skipped {owner}/{name}#{number} (no change)");
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PrSkippedNoChange {
                number,
                owner: owner.to_string(),
                name: name.to_string(),
                url: url.to_string(),
            },
            message,
        )
        .build(),
    );
}

pub(super) fn emit_activity_phase_completed(
    ctx: &WorkerContext,
    account: &Account,
    phase: SyncPhaseLabel,
    summary: impl Into<String>,
) {
    emit_activity_phase_completed_with_skips(ctx, account, phase, summary, 0);
}

pub(super) fn emit_activity_phase_completed_with_skips(
    ctx: &WorkerContext,
    account: &Account,
    phase: SyncPhaseLabel,
    summary: impl Into<String>,
    cache_skips: u32,
) {
    let summary = summary.into();
    let message = format!("{} complete - {}", phase_completed_label(phase), summary);
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::PhaseCompleted {
                phase,
                summary,
                cache_skips,
            },
            message,
        )
        .build(),
    );
}

pub(super) fn emit_activity_cycle_completed(
    ctx: &WorkerContext,
    account: &Account,
    prs_visited: u32,
    summary: impl Into<String>,
) {
    let summary = summary.into();
    let message = format!("Sync complete for {} - {}", account.login, summary);
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Info,
            Some(account.id),
            ActivityKind::CycleCompleted {
                prs_visited,
                summary,
            },
            message,
        )
        .build(),
    );
}

pub(in crate::sync::worker) fn emit_activity_cycle_failed(
    ctx: &WorkerContext,
    account: &Account,
    error_kind: &str,
    error_message: &str,
) {
    let truncated = super::super::short_error_message(error_message);
    let message = format!("Sync failed ({error_kind}): {truncated}");
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Error,
            Some(account.id),
            ActivityKind::CycleFailed {
                error_message: truncated,
                error_kind: error_kind.to_string(),
            },
            message,
        )
        .build(),
    );
}

pub(super) fn emit_activity_rate_pause(
    ctx: &WorkerContext,
    account: &Account,
    reset_in: Option<Duration>,
    pct: u8,
) {
    let reset_in_seconds = reset_in.map(|d| d.as_secs()).unwrap_or(0);
    let message = if reset_in_seconds > 0 {
        format!(
            "Paused {} - API budget at {}%, resumes in {}s",
            account.login, pct, reset_in_seconds
        )
    } else {
        format!("Paused {} - API budget at {}%", account.login, pct)
    };
    record_activity(
        &ctx.activity,
        ctx.emit.as_ref(),
        ActivityEventBuilder::new(
            ActivityLevel::Warn,
            Some(account.id),
            ActivityKind::RateLimitPause { reset_in_seconds },
            message,
        )
        .build(),
    );
}
