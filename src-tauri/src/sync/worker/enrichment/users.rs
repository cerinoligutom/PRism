//! Per-cycle avatar caching (ADR 0013). Walks every `(login, avatar_url)`
//! pair the detail + events payload surfaced and UPSERTs them into `users`.

use rusqlite::params;

use super::super::unix_now;

/// Collect every `(login, avatar_url)` pair surfaced by this cycle's payload
/// and UPSERT them into `users`. Only entries with a populated `avatar_url`
/// are written: we never store NULLs, so a partial payload (e.g. an older
/// fixture or a comment-edit response that drops the avatar field) can't
/// blank a row a previous cycle populated.
///
/// Dedup happens via the SQL UPSERT itself; collecting into a HashMap first
/// would also work but every login on a typical PR (author + reviewers +
/// thread/issue comment heads + review submitters + timeline actors) hits a
/// small bound, so the cycle-time win isn't worth the extra allocation.
pub(super) fn write_user_avatars(
    tx: &rusqlite::Transaction<'_>,
    detail: Option<&crate::github::graphql::PullRequestDetail>,
    events: Option<&[crate::sync::status_timeline::TimelineEvent]>,
) -> Result<(), rusqlite::Error> {
    use crate::github::graphql::RequestedReviewer;

    let now = unix_now();
    let upsert = |login: &str, avatar_url: &Option<String>| -> Result<(), rusqlite::Error> {
        let Some(url) = avatar_url.as_deref() else {
            return Ok(());
        };
        if login.is_empty() || url.is_empty() {
            return Ok(());
        }
        tx.execute(
            "INSERT INTO users (login, avatar_url, last_seen_at)
                VALUES (?1, ?2, ?3)
             ON CONFLICT(login) DO UPDATE SET
                avatar_url = excluded.avatar_url,
                last_seen_at = excluded.last_seen_at",
            params![login, url, now],
        )?;
        Ok(())
    };

    if let Some(d) = detail {
        if let Some(author) = d.author.as_ref() {
            upsert(&author.login, &author.avatar_url)?;
        }
        if let Some(rr) = d.review_requests.as_ref() {
            for entry in &rr.nodes {
                // Team reviewers have no avatar URL on the User branch; the
                // `Team` and `Other` variants skip cleanly.
                if let Some(RequestedReviewer::User { login, avatar_url }) =
                    entry.requested_reviewer.as_ref()
                {
                    upsert(login, avatar_url)?;
                }
            }
        }
        for thread in &d.review_threads.nodes {
            for comment in &thread.comments.nodes {
                if let Some(actor) = comment.author.as_ref() {
                    upsert(&actor.login, &actor.avatar_url)?;
                }
            }
        }
        if let Some(reviews) = d.reviews.as_ref() {
            for review in &reviews.nodes {
                if let Some(actor) = review.author.as_ref() {
                    upsert(&actor.login, &actor.avatar_url)?;
                }
            }
        }
    }

    if let Some(events) = events {
        for event in events {
            if let (Some(login), Some(_)) = (
                event.actor_login.as_deref(),
                event.actor_avatar_url.as_ref(),
            ) {
                upsert(login, &event.actor_avatar_url)?;
            }
        }
    }
    Ok(())
}
