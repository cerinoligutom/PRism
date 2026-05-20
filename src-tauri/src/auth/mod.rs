//! Authentication: PAT entry, OS keychain storage, and account lifecycle.
//!
//! See ADR-0005 (PAT-only stored in OS keychain) and the GitHub-client
//! contract (`docs/contracts/github-client.md`). The token is never copied
//! into any long-lived struct; it is fetched fresh from the keychain on every
//! request via the `TokenSource` trait.
//!
//! When PR #11 (GraphQL client + shared HTTP infra) merges, the canonical
//! `TokenSource` trait + `AccountHandle` will live in `github::auth`. Until
//! then the trait is defined here in `token_source` and reconciled on rebase.

pub mod commands;
pub mod keychain;
pub mod store;
pub mod token_source;
pub mod validation;

pub use commands::AuthCommandError;
pub use store::{Account, AccountId, AccountStore};
pub use token_source::{AuthError, KeychainTokenSource, TokenSource};
