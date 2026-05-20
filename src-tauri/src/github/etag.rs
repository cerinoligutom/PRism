//! ETag store for conditional GitHub requests.
//!
//! REST endpoints get a literal `ETag` header that we replay on the next
//! request as `If-None-Match`; a 304 response saves a rate-limit unit. The
//! GraphQL endpoint does not honour `If-None-Match`, but we cache the response
//! body keyed by `{account}:gql:{query_hash}` so the sync worker can short-circuit
//! identical queries inside a short window. See ADR 0006.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::SystemTime;

/// Cached conditional-request metadata.
#[derive(Debug, Clone)]
pub struct EtagEntry {
    pub etag: String,
    pub last_seen_at: SystemTime,
    /// SHA-256 of the last-observed body. Used by GraphQL caching where the
    /// server does not echo an ETag.
    pub body_sha256: Option<[u8; 32]>,
}

impl EtagEntry {
    pub fn new(etag: impl Into<String>) -> Self {
        Self {
            etag: etag.into(),
            last_seen_at: SystemTime::now(),
            body_sha256: None,
        }
    }

    pub fn with_body_sha256(mut self, sha256: [u8; 32]) -> Self {
        self.body_sha256 = Some(sha256);
        self
    }
}

/// Persistence interface for ETag entries.
///
/// Issue #9 provides the SQLite-backed implementation. This crate ships
/// [`InMemoryEtagStore`] for tests.
pub trait EtagStore: Send + Sync {
    fn get(&self, key: &str) -> Option<EtagEntry>;
    fn put(&self, key: &str, entry: EtagEntry);
}

/// In-memory `EtagStore` for tests.
#[derive(Debug, Default)]
pub struct InMemoryEtagStore {
    inner: Mutex<HashMap<String, EtagEntry>>,
}

impl InMemoryEtagStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test helper: number of stored entries.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("etag store poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl EtagStore for InMemoryEtagStore {
    fn get(&self, key: &str) -> Option<EtagEntry> {
        self.inner
            .lock()
            .expect("etag store poisoned")
            .get(key)
            .cloned()
    }

    fn put(&self, key: &str, entry: EtagEntry) {
        self.inner
            .lock()
            .expect("etag store poisoned")
            .insert(key.to_string(), entry);
    }
}

/// Build the cache key for a REST endpoint.
pub fn rest_key(account_id: u64, method: &str, path: &str) -> String {
    format!("{account_id}:{method}:{path}")
}

/// Build the cache key for a GraphQL query.
///
/// `query_hash` is typically the hex SHA-256 of the canonicalised query body
/// plus its variables.
pub fn graphql_key(account_id: u64, query_hash: &str) -> String {
    format!("{account_id}:gql:{query_hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_in_memory_store() {
        let store = InMemoryEtagStore::new();
        assert!(store.is_empty());

        let entry = EtagEntry::new(r#"W/"abc123""#).with_body_sha256([7u8; 32]);
        store.put("0:GET:/repos/o/r/pulls/1", entry.clone());

        let got = store.get("0:GET:/repos/o/r/pulls/1").unwrap();
        assert_eq!(got.etag, entry.etag);
        assert_eq!(got.body_sha256, Some([7u8; 32]));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn missing_key_returns_none() {
        let store = InMemoryEtagStore::new();
        assert!(store.get("nope").is_none());
    }

    #[test]
    fn key_formatters_use_documented_layout() {
        assert_eq!(rest_key(7, "GET", "/repos/o/r"), "7:GET:/repos/o/r");
        assert_eq!(graphql_key(7, "deadbeef"), "7:gql:deadbeef");
    }
}
