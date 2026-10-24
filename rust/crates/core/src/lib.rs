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

pub mod relocate;
pub use relocate::{Relocation, Relocations};

pub mod time;

pub mod sample;
pub use sample::{audit_project, audit_project_with, collect_samples, CollectReport};

pub mod error;

pub mod legacy;
pub use error::ClavusResult;

/// The name a note is signed with when the user does not give one.
///
/// The CLI and the TUI must sign a note the same way, or a
/// collaborator sees the same person as two different authors. The
/// environment is asked first, then the host name, and only then a
/// word that says nothing about who wrote it.
pub fn default_author() -> String {
    for key in ["CLAVUS_AUTHOR", "USER", "LOGNAME"] {
        if let Ok(name) = std::env::var(key) {
            let name = name.trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    // A machine with no user name — a container, a cron job, CI — still
    // needs a name to sign a note with. The host name says who is
    // speaking; the word "cli" said nothing at all and looked like a
    // person's name in a list of people.
    if let Ok(host) = std::env::var("HOSTNAME") {
        let host = host.trim().to_string();
        if !host.is_empty() {
            return host;
        }
    }
    "unknown".to_string()
}

/// Truncate `text` to at most `len` characters, for display.
///
/// Slicing a string with `[..8]` panics when the string is shorter, and
/// ids and hashes arrive from remote peers — so a hostile peer could
/// crash the client with a one-character id. This never panics.
pub fn short(text: &str, len: usize) -> String {
    text.chars().take(len).collect()
}
