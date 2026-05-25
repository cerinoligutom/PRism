-- v21 schema: persistent notifications inbox. Each row mirrors what the OS
-- toast pipeline (ADR 0017) dispatched, so a user who missed the transient
-- toast can recover from the in-app inbox at /dashboard/notifications.
--
-- The row carries a self-contained snapshot (`owner`, `repo`, `pr_number`,
-- `pr_node_id`, `pr_title`) so an entry stays meaningful even after the
-- referenced PR is pruned from `pull_requests`. `pull_request_id` is a soft
-- link kept for the State-A "open the drawer if the PR is still cached"
-- click path; ON DELETE SET NULL preserves the row when the PR row is
-- pruned. `account_id` cascades because a removed account drops every
-- inbox row tied to it - the OS toast already fired in real time, the
-- inbox copy carries no value once the account is gone.
--
-- `pr_node_id` is nullable: the v1 sync surface does not currently
-- denormalise the GraphQL node id onto `pull_requests`, so the column is
-- here for the snapshot shape but stays unwritten until a future migration
-- supplies the source field. State-A lookup uses `(owner, repo, pr_number)`
-- via `pr_lookup_by_coordinates` so the click path doesn't depend on the
-- node id today.
--
-- Migrations are forward-only and never edited in place. Add new
-- migrations as `NNNN_description.sql` rather than amending this file.

CREATE TABLE notifications (
    id              INTEGER PRIMARY KEY,
    kind            TEXT    NOT NULL,
    account_id      INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    pull_request_id INTEGER REFERENCES pull_requests(id) ON DELETE SET NULL,
    owner           TEXT    NOT NULL,
    repo            TEXT    NOT NULL,
    pr_number       INTEGER NOT NULL,
    pr_node_id      TEXT,
    pr_title        TEXT    NOT NULL,
    title           TEXT    NOT NULL,
    body            TEXT,
    created_at      INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

-- The inbox list reads newest first; this index keeps the projection cheap
-- as the row count grows. Future cursor pagination (#379) uses the same
-- ordering with `WHERE id < before_id` so the index serves both shapes.
CREATE INDEX idx_notifications_created_at
    ON notifications (created_at DESC);
