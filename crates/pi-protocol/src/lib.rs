//! Rust port of `packages/protocol` CBOR and framing.
//!
//! Behaviour, limits and error messages match the TypeScript implementation byte for byte, so either
//! side can be swapped in without observable change.

pub mod cbor;
pub mod framing;
