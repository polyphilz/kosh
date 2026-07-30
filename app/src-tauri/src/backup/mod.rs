//! Off-site recovery primitives.
//!
//! These primitives remain dormant until the supervised runtime slice lands.
//! Configuration is non-secret SQLite state, credentials are Keychain-only,
//! and every production object key is derived beneath Kosh's fixed prefix.

pub(crate) mod credentials;
pub(crate) mod domain;
pub mod litestream;
pub(crate) mod object_store;
pub(crate) mod probe;
