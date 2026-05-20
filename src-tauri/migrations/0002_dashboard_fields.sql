-- v2 schema: dashboard data fields. See docs/contracts/dashboard-data.md and
-- ADR 0009.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

-- Per-PR enrichments visible on the dashboard row.
ALTER TABLE pull_requests ADD COLUMN mergeable         TEXT;
ALTER TABLE pull_requests ADD COLUMN review_decision   TEXT;
ALTER TABLE pull_requests ADD COLUMN additions         INTEGER;
ALTER TABLE pull_requests ADD COLUMN deletions         INTEGER;
ALTER TABLE pull_requests ADD COLUMN changed_files     INTEGER;

-- CI rollup. Pre-aggregated by sync rather than re-counted at query time.
ALTER TABLE pull_requests ADD COLUMN ci_state          TEXT;
ALTER TABLE pull_requests ADD COLUMN ci_total          INTEGER;
ALTER TABLE pull_requests ADD COLUMN ci_passing        INTEGER;

-- Per-repo Team-view opt-in. Driven by Settings -> Repositories.
ALTER TABLE repos ADD COLUMN is_team_tracked INTEGER NOT NULL DEFAULT 0;

-- Reviewers requested but not yet submitted. Distinct from `reviews`
-- (submitted reviews).
CREATE TABLE requested_reviewers (
    id                  INTEGER PRIMARY KEY,
    pull_request_id     INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    login               TEXT    NOT NULL,
    reviewer_type       TEXT    NOT NULL,
    UNIQUE (pull_request_id, reviewer_type, login)
);

CREATE INDEX idx_requested_reviewers_pr
    ON requested_reviewers (pull_request_id);

-- Viewer relations. Rebuilt each sync cycle from Search-API results. One row
-- per (account, PR) where the account has any relationship to the PR; rows
-- whose `last_seen_at` predates the current cycle start are pruned.
CREATE TABLE pull_request_viewer_relations (
    account_id              INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    pull_request_id         INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    is_authored             INTEGER NOT NULL DEFAULT 0,
    is_review_requested     INTEGER NOT NULL DEFAULT 0,
    is_involved             INTEGER NOT NULL DEFAULT 0,
    last_seen_at            INTEGER NOT NULL,
    PRIMARY KEY (account_id, pull_request_id)
);

CREATE INDEX idx_pull_request_viewer_relations_account_authored
    ON pull_request_viewer_relations (account_id, pull_request_id)
    WHERE is_authored = 1;

CREATE INDEX idx_pull_request_viewer_relations_account_review_requested
    ON pull_request_viewer_relations (account_id, pull_request_id)
    WHERE is_review_requested = 1;

CREATE INDEX idx_pull_request_viewer_relations_account_involved
    ON pull_request_viewer_relations (account_id, pull_request_id)
    WHERE is_involved = 1;
