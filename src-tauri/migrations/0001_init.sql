-- v1 schema for PRism local cache. See PRD section 7.2 and ADR 0003.
--
-- Migrations are forward-only and never edited in place. Add new migrations
-- as `NNNN_description.sql` rather than amending this file.

CREATE TABLE accounts (
    id          INTEGER PRIMARY KEY,
    label       TEXT    NOT NULL,
    host        TEXT    NOT NULL,
    login       TEXT    NOT NULL,
    scopes      TEXT    NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    UNIQUE (host, login)
);

CREATE TABLE repos (
    id          INTEGER PRIMARY KEY,
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    owner       TEXT    NOT NULL,
    name        TEXT    NOT NULL,
    visibility  TEXT    NOT NULL,
    etag        TEXT,
    UNIQUE (account_id, owner, name)
);

CREATE TABLE pull_requests (
    id                                  INTEGER PRIMARY KEY,
    repo_id                             INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    number                              INTEGER NOT NULL,
    title                               TEXT    NOT NULL,
    state                               TEXT    NOT NULL,
    draft                               INTEGER NOT NULL DEFAULT 0,
    author_login                        TEXT    NOT NULL,
    created_at                          INTEGER NOT NULL,
    updated_at                          INTEGER NOT NULL,
    latest_status_change_at             INTEGER,
    latest_status_change_event_type     TEXT,
    base_ref                            TEXT    NOT NULL,
    head_ref                            TEXT    NOT NULL,
    UNIQUE (repo_id, number)
);

CREATE TABLE reviews (
    id                  INTEGER PRIMARY KEY,
    pull_request_id     INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    reviewer_login      TEXT    NOT NULL,
    state               TEXT    NOT NULL,
    submitted_at        INTEGER,
    body                TEXT
);

CREATE TABLE review_threads (
    id                  INTEGER PRIMARY KEY,
    pull_request_id     INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    is_resolved         INTEGER NOT NULL DEFAULT 0,
    original_line       INTEGER,
    path                TEXT
);

CREATE TABLE review_comments (
    id                  INTEGER PRIMARY KEY,
    review_thread_id    INTEGER NOT NULL REFERENCES review_threads(id) ON DELETE CASCADE,
    author_login        TEXT    NOT NULL,
    body                TEXT    NOT NULL,
    created_at          INTEGER NOT NULL
);

CREATE TABLE issue_comments (
    id                  INTEGER PRIMARY KEY,
    pull_request_id     INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    author_login        TEXT    NOT NULL,
    body                TEXT    NOT NULL,
    created_at          INTEGER NOT NULL
);

CREATE TABLE timeline_events (
    id                  INTEGER PRIMARY KEY,
    pull_request_id     INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    event_type          TEXT    NOT NULL,
    actor_login         TEXT,
    created_at          INTEGER NOT NULL,
    payload             TEXT    NOT NULL DEFAULT '{}'
);

CREATE TABLE check_runs (
    id                  INTEGER PRIMARY KEY,
    pull_request_id     INTEGER NOT NULL REFERENCES pull_requests(id) ON DELETE CASCADE,
    name                TEXT    NOT NULL,
    status              TEXT    NOT NULL,
    conclusion          TEXT,
    html_url            TEXT,
    completed_at        INTEGER
);

-- ETag store for conditional GitHub requests. See docs/contracts/github-client.md.
-- `body_sha256` is a 32-byte BLOB; NULL until the first successful body hash.
CREATE TABLE etags (
    key             TEXT    PRIMARY KEY,
    etag            TEXT    NOT NULL,
    last_seen_at    INTEGER NOT NULL,
    body_sha256     BLOB
);

-- Dashboard "open PRs in repo X".
CREATE INDEX idx_pull_requests_repo_state
    ON pull_requests (repo_id, state);

-- "Authored by me" view.
CREATE INDEX idx_pull_requests_author_state
    ON pull_requests (author_login, state);

-- Sort PRs by most-recent status change.
CREATE INDEX idx_pull_requests_latest_status_change_at
    ON pull_requests (latest_status_change_at DESC);

-- Unresolved-thread counts per PR.
CREATE INDEX idx_review_threads_pr_resolved
    ON review_threads (pull_request_id, is_resolved);
