//! Clavus core library.
//!
//! Contains the shared data types, blob store, snapshot engine, cue store,
//! and sample audit system. All other Clavus crates depend on this one.

pub mod types;
pub use types::*;

pub mod blob;
pub use blob::BlobStore;

pub mod snapshot;
pub use snapshot::SnapshotStore;

pub mod cue;
pub use cue::{CueStore, ImportOutcome};

pub mod bitwig_archive;

pub mod als_refs;

pub mod project;

pub mod time;

pub mod sample;
pub use sample::{audit_project, collect_samples, CollectReport};

pub mod error;
pub use error::ClavusResult;

/// Truncate `text` to at most `len` characters, for display.
///
/// Slicing a string with `[..8]` panics when the string is shorter, and
/// ids and hashes arrive from remote peers — so a hostile peer could
/// crash the client with a one-character id. This never panics.
pub fn short(text: &str, len: usize) -> String {
    text.chars().take(len).collect()
}
