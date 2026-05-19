# GitHub client interface contract

This document is the shared interface contract between the two GitHub client implementations:

- **GraphQL client** — issue [#11](https://github.com/cerinoligutom/PRism/issues/11), implements the primary surface (PR detail with review-thread resolution state, timeline events sufficient for status reconstruction). Owns the shared HTTP/auth/rate-limit/ETag infrastructure.
- **REST client** — issue [#12](https://github.com/cerinoligutom/PRism/issues/12), implements the endpoints GraphQL doesn't cover well (notably the timeline events API for #14 status reconstruction). Consumes the shared infrastructure.

ADR [0006](../adr/0006-graphql-first-rest-fallback.md) is the policy decision behind the GraphQL-first stance; this file is the implementation contract that makes the two clients composable.

## Why this exists

Two parallel PRs landing in `src-tauri/src/github/` will conflict on `Cargo.toml`, `lib.rs` registration, and the shape of the shared HTTP layer if they each invent their own. The first agent (#11) builds the layer; the second agent (#12) plugs into it.

If you're implementing either issue, **read this end-to-end before writing code**. Anything ambiguous here is a spec bug — open a PR or comment on the issue to refine the contract rather than silently diverging.

## Module layout

```
src-tauri/
└── src/
    └── github/
        ├── mod.rs              # public surface — re-exports + factory
        ├── client.rs           # SHARED: GitHubClient struct + ctor
        ├── auth.rs             # SHARED: Auth header injection from keychain
        ├── rate_limit.rs       # SHARED: per-account rate-limit accounting
        ├── etag.rs             # SHARED: ETag store trait + SQLite impl
        ├── error.rs            # SHARED: GitHubError + From conversions
        ├── graphql/            # owned by #11
        │   ├── mod.rs
        │   ├── queries.rs
        │   └── ...
        └── rest/               # owned by #12
            ├── mod.rs
            ├── timeline.rs
            └── ...
```

Files marked SHARED land in PR #11 with the GraphQL client. PR #12 imports them and only adds files under `rest/`.

## Shared types

```rust
// src-tauri/src/github/client.rs

pub struct GitHubClient {
    inner: reqwest::Client,
    account: AccountHandle,
    rate: Arc<RateBudget>,
    etags: Arc<dyn EtagStore + Send + Sync>,
    base_url: Url,
}

impl GitHubClient {
    pub fn builder() -> GitHubClientBuilder { ... }

    pub async fn get_conditional(&self, path: &str) -> Result<Conditional<Bytes>, GitHubError>;
    pub async fn post_graphql<T>(&self, query: &str, vars: serde_json::Value)
        -> Result<T, GitHubError>
    where T: DeserializeOwned;

    pub fn rate(&self) -> &RateBudget { &self.rate }
}

pub enum Conditional<T> {
    NotModified,
    Modified { body: T, etag: Option<String> },
}
```

Both clients construct one `GitHubClient` per account. The HTTP layer (reqwest), auth header construction, base URL routing (`github.com` vs Enterprise hosts), and rate-limit accounting all flow through this single struct.

## Auth

```rust
// src-tauri/src/github/auth.rs

pub struct AccountHandle {
    pub id: AccountId,        // u64 or Uuid — opaque
    pub host: String,         // "github.com" or enterprise host
    pub label: String,        // user-visible label
}

pub trait TokenSource: Send + Sync {
    fn token(&self, account: &AccountHandle) -> Result<SecretString, AuthError>;
}
```

`#10` provides the concrete `KeychainTokenSource`. Both client implementations depend on the trait, not the concrete type, so they can be unit-tested with a fixture `TokenSource` that returns a fake PAT.

The token is fetched fresh on every request (cheap — keychain access is local). It is never copied into the `GitHubClient` struct or logged.

## Rate-limit accounting

```rust
// src-tauri/src/github/rate_limit.rs

pub struct RateBudget {
    /* internal: AtomicI64 for remaining, AtomicI64 for reset_at_epoch */
}

impl RateBudget {
    pub fn snapshot(&self) -> RateSnapshot;
    pub(crate) fn update_from_headers(&self, headers: &HeaderMap);
}

pub struct RateSnapshot {
    pub limit: i64,
    pub remaining: i64,
    pub used: i64,
    pub reset_at: SystemTime,
}
```

The GraphQL and REST endpoints share a single rate-limit budget per account on github.com (5000 req/hr for authenticated users). Every response touches `update_from_headers`. Sync workers can snapshot the budget before scheduling work and bail if `remaining` is below their threshold.

GitHub Enterprise rate limits are configurable per host — `RateBudget` only mirrors what the headers say; it does not assume the 5000/hr cap.

## ETag store

```rust
// src-tauri/src/github/etag.rs

pub trait EtagStore {
    fn get(&self, key: &str) -> Option<EtagEntry>;
    fn put(&self, key: &str, entry: EtagEntry);
}

pub struct EtagEntry {
    pub etag: String,
    pub last_seen_at: SystemTime,
    pub body_sha256: Option<[u8; 32]>,
}
```

`#9` provides the SQLite-backed concrete impl. `#11` defines the trait + an in-memory impl for tests. The key naming convention is `{account_id}:{method}:{path}` for REST and `{account_id}:gql:{query_hash}` for GraphQL.

## Errors

```rust
// src-tauri/src/github/error.rs

#[derive(thiserror::Error, Debug)]
pub enum GitHubError {
    #[error("network: {0}")] Network(#[from] reqwest::Error),
    #[error("auth: {0}")]    Auth(#[from] AuthError),
    #[error("rate limited; retry after {retry_after:?}")]
    RateLimited { retry_after: Option<Duration> },
    #[error("token expired or invalid")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("github 5xx: {status}")] Server { status: u16 },
    #[error("graphql errors: {0:?}")] Graphql(Vec<GraphqlError>),
    #[error("deserialise: {0}")]      Deserialize(#[from] serde_json::Error),
}
```

A 401 from any endpoint surfaces as `Unauthorized` — the sync worker maps this to an "expired token" UI state without needing endpoint-specific knowledge.

## Coordination notes for the parallel agents

- **PR #11 lands first.** Even if PR #12 is technically authored in parallel, it depends on the shared types compiling. The cleanest sequencing is: PR #11 → merge → rebase #12 → open #12.
- If #12 must run in parallel anyway, it stubs the shared types behind a feature flag and the rebase deletes the stub.
- Both PRs add deps to `Cargo.toml`. Agents should expect a merge conflict on the `[dependencies]` block and resolve it by union — never delete the other PR's lines.
- Neither client opens a long-lived task. The background sync worker (#13) drives polling cadence; the clients are stateless from the worker's perspective.

## Out of scope for v1

- OAuth or GitHub App auth (PAT only, ADR 0005).
- Webhook / streaming receivers (polling only, ADR 0004).
- Mutations — both clients are read-only in v1 (PRD §3).
