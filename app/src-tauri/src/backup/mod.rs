//! Off-site recovery primitives.
//!
//! Configuration is non-secret SQLite state, credentials are Keychain-only,
//! and every production object key is derived beneath Kosh's fixed prefix.
//! Media uploads run as optional durable background work; relational
//! replication remains dormant until the supervised Litestream slice.

pub(crate) mod credentials;
pub(crate) mod domain;
pub mod litestream;
pub(crate) mod media_reconciler;
pub(crate) mod object_store;
pub(crate) mod probe;
