//! Snapshot engine — save, load, and traverse the snapshot chain.
//!
//! Each snapshot is stored as:
//!   - Raw project file bytes → blob store (identity = SHA256)
//!   - Snapshot metadata → the *project's* own record directory
//!
//! Parent chain: each snapshot points to its parent by hash.
//! Traversal: follow parent links until None (the root).
//!
//! A snapshot record belongs to a project, not to a piece of content. Two
//! projects that happen to contain the same bytes are two different
//! projects with two different histories, so each keeps its own records.

use crate::blob::BlobStore;
use crate::project::ProjectId;
use crate::types::{ClavusError, Snapshot};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// What happened when a snapshot was saved.
#[derive(Debug, Clone)]
pub enum SaveOutcome {
    /// A new snapshot was recorded and the project's HEAD moved to it.
    Created(Snapshot),
    /// The project's bytes are identical to a snapshot this project
    /// already has. Nothing was recorded and the message was not stored.
    Unchanged(Snapshot),
    /// The project file is the same as an existing snapshot, but the
    /// audio it points at is not.
    ///
    /// The snapshot's manifest describes audio that no longer exists,
    /// so the record is refreshed: the new message, the new manifest
    /// and the new audio are stored. The chain's node stays the same —
    /// the project file is the same file.
    Updated(Snapshot),
}

impl SaveOutcome {
    /// The snapshot this outcome refers to.
    pub fn snapshot(&self) -> &Snapshot {
        match self {
            SaveOutcome::Created(s) => s,
            SaveOutcome::Unchanged(s) => s,
            SaveOutcome::Updated(s) => s,
        }
    }

    /// Whether a new snapshot was actually stored.
    pub fn created(&self) -> bool {
        matches!(self, SaveOutcome::Created(_))
    }

    /// Whether the audio changed under an unchanged project file.
    pub fn audio_changed(&self) -> bool {
        matches!(self, SaveOutcome::Updated(_))
    }
}

/// What happened to the audio a snapshot recorded.
#[derive(Debug, Default, Clone)]
pub struct SampleRestore {
    /// The samples that were written back.
    pub restored: Vec<PathBuf>,
    /// The samples the snapshot recorded, but whose bytes are not here.
    pub missing: Vec<PathBuf>,
    /// The samples that were left alone: the user has a file there.
    pub kept: Vec<PathBuf>,
}

/// A save outcome reads like the snapshot it refers to, so callers that
/// only need the snapshot's fields do not have to unwrap it.
impl std::ops::Deref for SaveOutcome {
    type Target = Snapshot;

    fn deref(&self) -> &Self::Target {
        self.snapshot()
    }
}

/// Manages snapshots for a single project.
#[derive(Debug)]
pub struct SnapshotStore {
    /// The underlying blob store.
    pub store: BlobStore,
    /// The project's identity.
    pub project: ProjectId,
}

impl SnapshotStore {
    /// Create a SnapshotStore for a project.
    pub fn new(store: BlobStore, project: &ProjectId) -> Self {
        Self {
            store,
            project: project.clone(),
        }
    }

    /// Name of the project this store manages.
    pub fn project_name(&self) -> &str {
        &self.project.name
    }

    /// The ref that holds this project's HEAD.
    ///
    /// Every project needs its own HEAD. A single shared ref meant that
    /// whichever project was snapshotted last owned the chain, so
    /// `log`, `push` and `restore` for one project showed another's
    /// history.
    pub fn head_ref(&self) -> String {
        format!("{}/HEAD", self.project.key)
    }

    /// Read this project's HEAD.
    ///
    /// Falls back to the old shared `HEAD` ref, but only when that
    /// snapshot belongs to this project — otherwise another project's
    /// chain would leak in.
    pub fn head(&self) -> Option<Snapshot> {
        if let Some(h) = self.store.read_ref(&self.head_ref()) {
            if let Some(snap) = self.store.load_meta(&self.project.key, &h) {
                return Some(snap);
            }
        }

        // Legacy layout: one shared HEAD ref for every project.
        if let Some(h) = self.store.read_ref("HEAD") {
            if let Some(snap) = self.store.load_meta(&self.project.key, &h) {
                if Self::belongs_to(&snap, &self.project) {
                    return Some(snap);
                }
            }
        }
        None
    }

    /// The snapshot records that cannot be read.
    ///
    /// A record that exists on disk but cannot be parsed is a broken
    /// record, not a missing one. Reporting it as "no snapshots" would
    /// hide a damaged history behind a clean bill of health.
    pub fn corrupt_records(&self) -> Vec<String> {
        let mut out = Vec::new();
        for hash in self.store.list_meta_hashes(&self.project.key) {
            if self.store.load_meta(&self.project.key, &hash).is_none() {
                out.push(hash);
            }
        }
        out
    }

    /// Whether a snapshot was taken from the given project.
    ///
    /// The project folder and the file name must both match: a snapshot
    /// of `Album1/Track.als` is not a snapshot of `Album2/Track.als`,
    /// even though the two share a file name.
    pub fn belongs_to(snap: &Snapshot, project: &ProjectId) -> bool {
        let p = Path::new(&snap.project_path);
        let same_name = p
            .file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case(&project.file_name))
            .unwrap_or(false);
        if !same_name {
            return false;
        }
        match (
            p.parent().and_then(|d| d.file_name()),
            project.path.parent().and_then(|d| d.file_name()),
        ) {
            (Some(a), Some(b)) => a == b,
            // A bare file name has no folder to compare.
            _ => true,
        }
    }

    /// Save a snapshot of a project file.
    ///
    /// 1. Read raw bytes → store as blob (the content's identity)
    /// 2. Compute the content hash
    /// 3. Write this project's snapshot record
    /// 4. Update this project's HEAD ref
    ///
    /// When the project already has a snapshot with the same bytes,
    /// nothing is recorded and [`SaveOutcome::Unchanged`] is returned —
    /// the caller must say so rather than claim a new snapshot.
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
    ) -> Result<SaveOutcome, ClavusError> {
        // 1. Read raw project file bytes
        let raw_bytes = std::fs::read(project_path)
            .map_err(|e| ClavusError::ParseError(format!("read {}: {}", project_path.display(), e)))?;

        // 2. Store raw bytes — the content's identity = SHA256 of raw bytes
        let content_hash = BlobStore::sha256(&raw_bytes);
        self.store.put(&raw_bytes);
        debug!("Stored raw project blob: {}", &content_hash[..12]);

        // 2b. Store the audio the project points at.
        //
        // A snapshot of a set is not the set alone: the audio it plays
        // is part of what the snapshot is. The bytes are kept in the
        // blob store under their own hash, so a restore can put them
        // back where the project expects them.
        self.capture_samples(project_path, &sample_paths);

        // 3. Does this project already have a snapshot with these bytes?
        if let Some(existing) = self.store.load_meta(&self.project.key, &content_hash) {
            if existing.sample_paths == sample_paths {
                info!(
                    "Snapshot {} already exists for {} (unchanged file)",
                    existing.short_hash(8),
                    project_path.display()
                );
                return Ok(SaveOutcome::Unchanged(existing));
            }
            // The project file is the same, but the audio it points at
            // is not. The snapshot's manifest now describes audio that
            // no longer exists, so the record is refreshed: the new
            // message, the new manifest, the new audio. The node in the
            // chain stays where it is — it is the same file.
            let updated = Snapshot {
                hash: existing.hash.clone(),
                parent: existing.parent.clone(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0),
                message: message.to_string(),
                track_count,
                bpm,
                tags: existing.tags.clone(),
                notes: existing.notes.clone(),
                sample_hashes,
                sample_paths,
                project_path: existing.project_path.clone(),
                project_folder: existing.project_folder.clone(),
            };
            self.store
                .write_meta(&self.project.key, &updated)
                .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;
            info!(
                "Refreshed snapshot {} for {} (the audio changed)",
                updated.short_hash(8),
                project_path.display()
            );
            return Ok(SaveOutcome::Updated(updated));
        }

        // 4. Create and write this project's snapshot record
        let snapshot = Snapshot {
            hash: content_hash.clone(),
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
            // The project's folder, so a relay can tell two projects
            // that share a file name apart.
            project_folder: self.project.folder_path().to_string(),
        };

        self.store
            .write_meta(&self.project.key, &snapshot)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;

        info!(
            "Created snapshot {}: {}",
            snapshot.short_hash(8),
            message
        );

        // 5. Update this project's HEAD ref
        self.store
            .update_ref(&self.head_ref(), &content_hash)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;

        Ok(SaveOutcome::Created(snapshot))
    }

    /// Load a snapshot by hash (full or short).
    pub fn load(&self, hash: &str) -> Option<Snapshot> {
        self.store.load_meta(&self.project.key, hash)
    }

    /// Count how many snapshots are reachable from `head`.
    pub fn chain_length(&self, head: Option<&str>) -> usize {
        let mut current = head
            .and_then(|h| self.store.read_ref(h))
            .or_else(|| self.store.read_ref(&self.head_ref()));
        let mut seen = std::collections::HashSet::new();
        let mut count = 0;

        while let Some(h) = current {
            if seen.contains(&h) {
                break; // cycle detected
            }
            seen.insert(h.clone());
            count += 1;
            current = self
                .store
                .load_meta(&self.project.key, &h)
                .and_then(|s| s.parent);
        }
        count
    }

    /// Walk the snapshot chain from `head` back to root.
    /// Returns snapshots in order: [newest, ..., oldest].
    pub fn walk_chain(&self, head: Option<&str>) -> Vec<Snapshot> {
        // `head` may be a ref name (e.g. "HEAD") or a snapshot hash — both
        // are used by callers, so resolve either one.
        let mut current = head
            .and_then(|h| {
                self.store.read_ref(h).or_else(|| {
                    // A snapshot hash that exists is its own chain head.
                    if self.store.load_meta(&self.project.key, h).is_some() {
                        Some(h.to_string())
                    } else {
                        None
                    }
                })
            })
            .or_else(|| self.store.read_ref(&self.head_ref()));
        let mut snapshots = Vec::new();
        let mut seen = std::collections::HashSet::new();

        while let Some(h) = current {
            if seen.contains(&h) {
                break;
            }
            seen.insert(h.clone());
            if let Some(snap) = self.store.load_meta(&self.project.key, &h) {
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
    /// Rejects changes that would shrink the chain, because a shorter
    /// chain means snapshots the user can no longer reach.
    pub fn set_head(&self, new_head: &str, source: &str) -> Result<(), ClavusError> {
        // Validate new_head exists
        if !self.store.load_meta(&self.project.key, new_head).is_some() {
            return Err(ClavusError::SnapshotNotFound(new_head.to_string()));
        }

        // Prevent shrinking the chain.
        if let Some(current) = self.store.read_ref(&self.head_ref()) {
            let old_len = self.chain_length(Some(&current));
            let new_len = self.chain_length(Some(new_head));
            if new_len < old_len {
                warn!(
                    "Rejected HEAD change {}→{}: chain would shrink {}→{} (source={})",
                    crate::short(&current, 8),
                    crate::short(new_head, 8),
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
            .update_ref(&self.head_ref(), new_head)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;

        info!(
            "HEAD → {} (source={})",
            crate::short(new_head, 8),
            source
        );
        Ok(())
    }

    /// Check whether a snapshot hash exists locally.
    pub fn has(&self, hash: &str) -> bool {
        self.store.load_meta(&self.project.key, hash).is_some()
    }

    /// Resolve a full hash or a unique hash prefix to a full snapshot hash.
    ///
    /// Returns [`ClavusError::SnapshotNotFound`] when nothing matches, and an
    /// error when the prefix is ambiguous — picking one at random would
    /// restore the wrong version of someone's project.
    pub fn resolve(&self, hash_or_prefix: &str) -> Result<String, ClavusError> {
        let prefix = hash_or_prefix.trim();
        if prefix.is_empty() {
            return Err(ClavusError::SnapshotNotFound(hash_or_prefix.to_string()));
        }
        // An exact match, but only for a snapshot of this project. The
        // blob store is shared, so a bare hash would otherwise find
        // another project's snapshot and restore it over this one.
        if self
            .store
            .load_meta(&self.project.key, prefix)
            .map(|s| Self::belongs_to(&s, &self.project))
            .unwrap_or(false)
        {
            return Ok(prefix.to_string());
        }

        // Prefer a match on the current chain.
        //
        // A prefix that matches two snapshots of the chain is refused
        // just like a prefix that matches two snapshots off it. Silently
        // restoring one of them is how a user ends up with the wrong
        // version of their project.
        let mut chain_matches = self
            .walk_chain(None)
            .into_iter()
            .filter(|s| s.hash.starts_with(prefix));
        if let Some(first) = chain_matches.next() {
            if let Some(second) = chain_matches.next() {
                return Err(ClavusError::AmbiguousSnapshot(format!(
                    "{} (matches {} and {})",
                    prefix,
                    crate::short(&first.hash, 8),
                    crate::short(&second.hash, 8)
                )));
            }
            return Ok(first.hash);
        }

        // Fall back to the other snapshots of *this* project.
        //
        // Every project keeps its own records, so this only ever sees
        // this project's snapshots.
        let mut matches = self
            .store
            .list_meta_hashes(&self.project.key)
            .into_iter()
            .filter(|h| h.starts_with(prefix))
            .filter(|h| {
                self.store
                    .load_meta(&self.project.key, h)
                    .map(|s| Self::belongs_to(&s, &self.project))
                    .unwrap_or(false)
            });
        let first = match matches.next() {
            Some(h) => h,
            None => return Err(ClavusError::SnapshotNotFound(hash_or_prefix.to_string())),
        };
        if let Some(second) = matches.next() {
            return Err(ClavusError::AmbiguousSnapshot(format!(
                "{} (matches {} and {})",
                prefix,
                crate::short(&first, 8),
                crate::short(&second, 8)
            )));
        }
        Ok(first)
    }

    /// Save a pre-built snapshot (from relay pull) without re-reading the file.
    /// Does NOT update HEAD — caller decides when to do that.
    pub fn save_raw(&self, snapshot: Snapshot) -> Result<(), ClavusError> {
        self.store
            .write_meta(&self.project.key, &snapshot)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;
        info!("Saved snapshot {}: {}", snapshot.short_hash(8), snapshot.message);
        Ok(())
    }

    /// Restore a snapshot's raw project bytes back to its original path.
    ///
    /// The HEAD is deliberately **not** moved: a restore is a checkout, not
    /// a new commit. Moving HEAD would drop every snapshot taken after the
    /// one being restored out of the project's history.
    ///
    /// # Arguments
    /// * `hash` — full or short snapshot hash (a unique prefix is accepted)
    /// * `target` — destination path (defaults to snapshot's `project_path` if `None`)
    pub fn restore(&self, hash: &str, target: Option<&Path>) -> Result<Snapshot, ClavusError> {
        let full_hash = self.resolve(hash)?;
        let snap = self.load(&full_hash).ok_or_else(|| {
            ClavusError::SnapshotNotFound(hash.to_string())
        })?;

        // The snapshot must belong to this project.
        if !Self::belongs_to(&snap, &self.project) {
            return Err(ClavusError::ParseError(format!(
                "snapshot {} belongs to a different project ({}); refusing to restore it over {}",
                snap.short_hash(8),
                snap.project_path,
                self.project.path.display()
            )));
        }

        let blob = self.store.get(&snap.hash).ok_or_else(|| {
            ClavusError::BlobNotFound(snap.hash.clone())
        })?;

        let dest = match target {
            Some(t) => t.to_path_buf(),
            None => {
                // The project path comes from the snapshot, which may have
                // been pushed by a remote peer. Only DAW project files are
                // restored, so a crafted path cannot be used to write
                // project bytes somewhere else on the disk.
                let p = Path::new(&snap.project_path);
                let is_project = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| {
                        e.eq_ignore_ascii_case("als") || e.eq_ignore_ascii_case("bwproject")
                    })
                    .unwrap_or(false);
                if !is_project {
                    return Err(ClavusError::ParseError(format!(
                        "refusing to restore to {:?}: not an Ableton (.als) or Bitwig (.bwproject) project",
                        snap.project_path
                    )));
                }
                p.to_path_buf()
            }
        };

        // A snapshot of an Ableton set must not be written over a Bitwig
        // bundle (or the other way round) — that destroys the project.
        if let (Some(target_ext), Some(snapshot_ext)) = (
            dest.extension().and_then(|e| e.to_str()),
            Path::new(&snap.project_path)
                .extension()
                .and_then(|e| e.to_str()),
        ) {
            if !target_ext.eq_ignore_ascii_case(snapshot_ext) {
                return Err(ClavusError::ParseError(format!(
                    "refusing to restore a .{} snapshot over {}: the project types differ",
                    snapshot_ext,
                    dest.display()
                )));
            }
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ClavusError::ParseError(format!("create parent dirs for {}: {}", dest.display(), e)))?;
        }

        // atomic write: temp file → rename
        //
        // A project that is a symlink is written *through* the link. The
        // rename would otherwise replace the link with a regular file: the
        // file the user actually keeps would still hold the newer content,
        // and the restore would report success while changing nothing.
        let target = crate::sample::link_target_of(&dest);
        let tmp = target.with_extension("tmp");
        std::fs::write(&tmp, &blob)
            .map_err(|e| ClavusError::ParseError(format!("write {}: {}", tmp.display(), e)))?;
        std::fs::rename(&tmp, &target)
            .map_err(|e| ClavusError::ParseError(format!("rename {} → {}: {}", tmp.display(), target.display(), e)))?;

        info!(
            "Restored {} from snapshot {} to {}",
            self.project.name,
            snap.short_hash(8),
            dest.display()
        );
        Ok(snap)
    }

    /// The path this store's project lives at.
    pub fn project_path(&self) -> &PathBuf {
        &self.project.path
    }

    /// Store the audio a snapshot records.
    ///
    /// The bytes of every sample the project points at are kept in the
    /// blob store under their own hash, together with the path they
    /// live at *inside the project*. The path is what a restore needs:
    /// the same project on another machine keeps its audio in the same
    /// place relative to itself.
    ///
    /// A sample that cannot be read is skipped. The snapshot still
    /// records it, and the restore says the audio is missing.
    fn capture_samples(
        &self,
        project_path: &Path,
        sample_paths: &std::collections::HashMap<String, String>,
    ) {
        let root = project_path.parent().unwrap_or(Path::new("."));
        for path in sample_paths.values() {
            let recorded = Path::new(path);
            // The manifest holds the path the sample has inside the
            // project; an older snapshot holds an absolute path.
            let file = if recorded.is_absolute() {
                recorded.to_path_buf()
            } else {
                root.join(recorded)
            };
            let Some(relative) = relative_inside(root, &file) else {
                // The sample lives outside the project. It is not the
                // project's to keep.
                continue;
            };
            if let Err(e) = self.store.store_sample(&file, &relative) {
                warn!("Could not store the audio {}: {}", file.display(), e);
            }
        }
    }

    /// Put the audio a snapshot recorded back where the project expects it.
    ///
    /// The samples are written where the project says they are — the
    /// path they had inside the project when the snapshot was taken.
    /// A file that is already there is never overwritten: the user's
    /// own audio is not the tool's to replace, and a different file
    /// with the same name is left exactly as it is.
    pub fn restore_samples(&self, snap: &Snapshot, project_path: &Path) -> SampleRestore {
        let mut report = SampleRestore::default();
        let root = project_path.parent().unwrap_or(Path::new("."));
        for hash in snap.sample_hashes.iter() {
            let Some(bytes) = self.store.get(hash) else {
                // The audio was never captured, or it has not been
                // fetched from the relay yet.
                report.missing.push(PathBuf::from(hash));
                continue;
            };
            let Some(relative) = self.store.sample_relpath(hash) else {
                report.missing.push(PathBuf::from(hash));
                continue;
            };
            let dest = root.join(&relative);
            if dest.exists() {
                // The user has a file there. It stays.
                report.kept.push(dest);
                continue;
            }
            if let Some(parent) = dest.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    warn!("Could not create {}: {}", parent.display(), e);
                    report.missing.push(dest);
                    continue;
                }
            }
            let tmp = dest.with_extension("clavus-tmp");
            match std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &dest)) {
                Ok(()) => report.restored.push(dest),
                Err(e) => {
                    warn!("Could not restore the audio {}: {}", dest.display(), e);
                    let _ = std::fs::remove_file(&tmp);
                    report.missing.push(dest);
                }
            }
        }
        report
    }
}

/// The path of `file` relative to `root`, if it is inside it.
///
/// A sample outside the project is not the project's to keep, and a
/// path that climbs out of the project is refused rather than
/// followed.
pub fn relative_inside(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// The newest snapshot of a set.
///
/// The newest one is the snapshot that is nobody's parent. Picking the
/// first or the last of a list puts the head on the wrong snapshot when
/// the list is not in chain order — and a relay's list is only in chain
/// order by luck.
pub fn newest_in(snapshots: &[Snapshot]) -> Option<String> {
    if snapshots.is_empty() {
        return None;
    }
    for candidate in snapshots {
        let is_parent_of_another = snapshots.iter().any(|other| {
            other.hash != candidate.hash && other.parent.as_deref() == Some(candidate.hash.as_str())
        });
        if !is_parent_of_another {
            return Some(candidate.hash.clone());
        }
    }
    // Every snapshot is somebody's parent, which is a cycle. It cannot
    // happen, but the list still has to yield something.
    snapshots.last().map(|s| s.hash.clone())
}
