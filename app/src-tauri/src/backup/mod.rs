//! Off-site recovery primitives.
//!
//! Chunk 29a deliberately exposes only the pinned Litestream binary,
//! configuration, and control contracts. No backup service is constructed or
//! enabled until the persistence and ownership slices land.

pub mod litestream;
