//! Snapshot engine — save, load, and traverse the snapshot chain.
//!
//! Each snapshot is stored as:
//!   - Raw project file bytes → blob store (identity = SHA256)
//!   - Parsed project JSON → blob store (content hash, for diff)
//!   - Snapshot metadata (.meta file) → alongside blob
//!
//! Parent chain: each snapshot points to its parent by hash.
//! Traversal: follow parent links until None (the root).

use crate::blob::BlobStore;
use crate::types::{ClavusError, Snapshot};
use std::path::Path;
use tracing::{debug, info, warn};

/// Manages snapshots for a single project.
#[derive(Debug)]
pub struct SnapshotStore {
    /// The underlying blob store.
    pub store: BlobStore,
    /// Name of the project this store manages.
    pub project_name: String,
}

impl SnapshotStore {
    /// Create a SnapshotStore for a project.
    pub fn new(store: BlobStore, project_name: &str) -> Self {
        Self {
            store,
            project_name: project_name.to_string(),
        }
    }

    /// Save a snapshot of a project file.
    ///
    /// 1. Read raw bytes → store as blob (THIS is the identity)
    /// 2. Store raw bytes as blob (for restore)
    /// 3. Compute content hash of serialized project
    /// 4. Write snapshot metadata
    ///
    /// If the raw file hash already exists (Ableton saved identical bytes),
    /// returns the existing snapshot without creating a duplicate.
    ///
    /// Returns `Err` if the file doesn't exist or can't be read.
    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        project_path: &Path,
        message: &str,
        parent: Option<&str>,
        track_count: usize,
        bpm: f64,
        sample_hashes: Vec<String>,
        sample_paths: std::collections::HashMap<String, String>,
    ) -> Result<Snapshot, ClavusError> {
        // 1. Read raw project file bytes
        let raw_bytes = std::fs::read(project_path)
            .map_err(|e| ClavusError::ParseError(format!("read {}: {}", project_path.display(), e)))?;

        // 2. Store raw bytes — identity = SHA256 of raw bytes
        let als_hash = BlobStore::sha256(&raw_bytes);
        self.store.put(&raw_bytes);
        debug!("Stored raw project blob: {}", &als_hash[..12]);

        // 3. Check for duplicate snapshot (same raw bytes = same hash)
        if let Some(existing) = self.store.load_meta(&als_hash) {
            info!(
                "Snapshot {} already exists for {} (unchanged file)",
                existing.short_hash(8),
                project_path.display()
            );
            return Ok(existing);
        }

        // 4. Create and write snapshot metadata
        let snapshot = Snapshot {
            hash: als_hash.clone(),
            content_hash: BlobStore::sha256(&raw_bytes), // same — raw is identity
            parent: parent.map(String::from),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
            message: message.to_string(),
            track_count,
            bpm,
            tags: vec![],
            notes: String::new(),
            sample_hashes,
            sample_paths,
            project_path: project_path.to_string_lossy().to_string(),
        };

        self.store
            .write_meta(&snapshot)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;

        info!(
            "Created snapshot {}: {}",
            snapshot.short_hash(8),
            message
        );

        // 5. Update HEAD ref
        self.store
            .update_ref("HEAD", &als_hash)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;

        Ok(snapshot)
    }

    /// Load a snapshot by hash (full or short).
    pub fn load(&self, hash: &str) -> Option<Snapshot> {
        self.store.load_meta(hash)
    }

    /// Get the current HEAD snapshot.
    pub fn head(&self) -> Option<Snapshot> {
        self.store
            .read_ref("HEAD")
            .and_then(|h| self.store.load_meta(&h))
    }

    /// Count how many snapshots are reachable from `head`.
    pub fn chain_length(&self, head: Option<&str>) -> usize {
        let mut current = head.and_then(|h| self.store.read_ref(h)).or_else(|| self.store.read_ref("HEAD"));
        let mut seen = std::collections::HashSet::new();
        let mut count = 0;

        while let Some(h) = current {
            if seen.contains(&h) {
                break; // cycle detected
            }
            seen.insert(h.clone());
            count += 1;
            current = self.store.load_meta(&h).and_then(|s| s.parent);
        }
        count
    }

    /// Walk the snapshot chain from `head` back to root.
    /// Returns snapshots in order: [newest, ..., oldest].
    pub fn walk_chain(&self, head: Option<&str>) -> Vec<Snapshot> {
        let mut current = head
            .and_then(|h| self.store.read_ref(h))
            .or_else(|| self.store.read_ref("HEAD"));
        let mut snapshots = Vec::new();
        let mut seen = std::collections::HashSet::new();

        while let Some(h) = current {
            if seen.contains(&h) {
                break;
            }
            seen.insert(h.clone());
            if let Some(snap) = self.store.load_meta(&h) {
                snapshots.push(snap.clone());
                current = snap.parent;
            } else {
                break;
            }
        }
        snapshots
    }

    /// Update HEAD for this project with validation.
    ///
    /// Rejects changes that would shrink the chain unless `source` is
    /// an explicit "restore" operation.
    pub fn set_head(&self, new_head: &str, source: &str) -> Result<(), ClavusError> {
        // Validate new_head exists
        if !self.store.has(new_head) && !self.store.load_meta(new_head).is_some() {
            return Err(ClavusError::SnapshotNotFound(new_head.to_string()));
        }

        // Prevent shrinking the chain unless explicitly restoring
        let allow_shrink = source == "restore" || source == "restore-cli";
        if let Some(current) = self.store.read_ref("HEAD") {
            let old_len = self.chain_length(Some(&current));
            let new_len = self.chain_length(Some(new_head));
            if !allow_shrink && new_len < old_len {
                warn!(
                    "Rejected HEAD change {}→{}: chain would shrink {}→{} (source={})",
                    &current[..8],
                    &new_head[..8],
                    old_len,
                    new_len,
                    source
                );
                return Err(ClavusError::IndexCorrupted(format!(
                    "Rejecting chain shrink {}→{} from {}",
                    old_len, new_len, source
                )));
            }
        }

        self.store
            .update_ref("HEAD", new_head)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;

        info!("HEAD {}→{} (source={})", self.store.read_ref("HEAD").unwrap_or_default(), new_head, source);
        Ok(())
    }
}
