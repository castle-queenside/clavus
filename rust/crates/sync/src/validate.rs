//! Validation of data received from a remote peer.
//!
//! Everything a relay sends is untrusted. A snapshot's hash becomes a
//! file name, a snapshot's parent becomes a ref name, and a cue's id
//! becomes a file name — so a hostile or broken peer must not be able to
//! make the client write outside its data directory, or crash it with a
//! malformed identifier.
//!
//! These checks run at the client's trust boundary, before anything
//! received is stored or displayed.

use clavus_core::blob::BlobStore;
use clavus_core::cue::CueStore;
use clavus_core::types::{Cue, Snapshot};

/// Why a received object was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    /// A short description of the problem.
    pub reason: String,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

/// Check that a snapshot is safe to store and display.
///
/// The hash becomes a path, and the parent is used as a ref name, so
/// both have to be well-formed.
pub fn validate_snapshot(snapshot: &Snapshot) -> Result<(), Rejection> {
    if !BlobStore::is_valid_hash(&snapshot.hash) {
        return Err(Rejection {
            reason: format!("snapshot hash is not a valid digest: {:?}", snapshot.hash),
        });
    }
    if snapshot.hash.len() != 64 {
        return Err(Rejection {
            reason: format!("snapshot hash is not 64 characters: {:?}", snapshot.hash),
        });
    }
    if let Some(parent) = &snapshot.parent {
        if !parent.is_empty() && !BlobStore::is_valid_hash(parent) {
            return Err(Rejection {
                reason: format!("snapshot parent is not a valid digest: {:?}", parent),
            });
        }
    }
    Ok(())
}

/// Check that a cue is safe to store and display.
pub fn validate_cue(cue: &Cue) -> Result<(), Rejection> {
    if !CueStore::is_valid_cue_id(&cue.id) {
        return Err(Rejection {
            reason: format!("cue id is not a valid identifier: {:?}", cue.id),
        });
    }
    Ok(())
}

/// Filter a batch of snapshots, dropping the invalid ones.
pub fn filter_snapshots(snapshots: Vec<Snapshot>) -> Vec<Snapshot> {
    snapshots
        .into_iter()
        .filter(|s| {
            if let Err(e) = validate_snapshot(s) {
                tracing::warn!("dropping invalid snapshot from peer: {}", e);
                return false;
            }
            true
        })
        .collect()
}

/// Filter a batch of cues, dropping the invalid ones.
pub fn filter_cues(cues: Vec<Cue>) -> Vec<Cue> {
    cues.into_iter()
        .filter(|c| {
            if let Err(e) = validate_cue(c) {
                tracing::warn!("dropping invalid cue from peer: {}", e);
                return false;
            }
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(hash: &str, parent: Option<&str>) -> Snapshot {
        Snapshot {
            hash: hash.to_string(),
            parent: parent.map(|p| p.to_string()),
            timestamp: 0.0,
            message: "m".into(),
            track_count: 0,
            bpm: 120.0,
            tags: vec![],
            notes: String::new(),
            sample_hashes: vec![],
            sample_paths: Default::default(),
            project_path: "/tmp/p.als".into(),
        }
    }

    #[test]
    fn a_well_formed_snapshot_passes() {
        assert!(validate_snapshot(&snapshot(&"a".repeat(64), None)).is_ok());
        assert!(
            validate_snapshot(&snapshot(&"a".repeat(64), Some(&"b".repeat(64)))).is_ok()
        );
    }

    #[test]
    fn a_short_hash_is_rejected() {
        assert!(validate_snapshot(&snapshot("abc", None)).is_err());
        assert!(validate_snapshot(&snapshot("", None)).is_err());
    }

    #[test]
    fn a_traversing_parent_is_rejected() {
        assert!(validate_snapshot(&snapshot(&"a".repeat(64), Some("../../etc"))).is_err());
        assert!(validate_snapshot(&snapshot(&"a".repeat(64), Some("a/b"))).is_err());
    }

    #[test]
    fn a_non_hex_hash_is_rejected() {
        assert!(validate_snapshot(&snapshot(&"z".repeat(64), None)).is_err());
    }
}
