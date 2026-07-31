//! Off-site recovery primitives.
//!
//! Configuration is non-secret SQLite state, credentials are Keychain-only,
//! and every production object key is derived beneath Kosh's fixed prefix.
//! Media uploads run as optional durable background work; relational
//! replication runs only for an enabled configuration under the supervised
//! Litestream runtime.

pub(crate) mod checkpoint;
pub(crate) mod credentials;
pub(crate) mod domain;
pub mod litestream;
pub(crate) mod litestream_runtime;
pub(crate) mod media_reconciler;
pub(crate) mod object_store;
pub(crate) mod owner;
pub(crate) mod probe;
pub(crate) mod restore;
pub(crate) mod settings;
pub(crate) mod writer_identity;
