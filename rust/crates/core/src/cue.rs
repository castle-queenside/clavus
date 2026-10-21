//! Cue store — timeline-anchored comments for DAW projects.
//!
//! Cues are stored as individual JSON files:
//!   `~/.clavus/cues/{project_name}/{cue_id}.json`

use crate::types::{ClavusError, Cue, CueReply, CueStatus};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Manages cue files for a single project.
#[derive(Debug)]
pub struct CueStore {
    cues_dir: PathBuf,
}

/// Outcome of importing a remote cue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportOutcome {
    /// The remote cue was new (or newer) and has been stored.
    Imported,
    /// The remote cue was identical to the local one.
    Unchanged,
    /// The local cue was newer; a conflict note was added to it.
    Conflict,
}

impl std::fmt::Display for ImportOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportOutcome::Imported => write!(f, "imported"),
            ImportOutcome::Unchanged => write!(f, "unchanged"),
            ImportOutcome::Conflict => write!(f, "conflict"),
        }
    }
}

impl CueStore {
    /// Create a CueStore for a project.
    pub fn new(clavus_root: &Path, project_name: &str) -> std::io::Result<Self> {
        let cues_dir = clavus_root.join("cues").join(project_name);
        fs::create_dir_all(&cues_dir)?;
        Ok(Self { cues_dir })
    }

    fn cue_path(&self, id: &str) -> PathBuf {
        self.cues_dir.join(format!("{}.json", id))
    }

    // ─── CRUD ──────────────────────────────────────────────────────

    /// Add a new cue.
    pub fn add(
        &self,
        text: &str,
        position: &str,
        author: &str,
        snapshot_hash: &str,
        track_name: &str,
    ) -> Result<Cue, ClavusError> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let id = Self::new_cue_id(text, position, timestamp, author);

        let cue = Cue {
            id: id.clone(),
            position: position.to_string(),
            text: text.to_string(),
            author: author.to_string(),
            timestamp,
            status: CueStatus::Pending,
            snapshot_hash: snapshot_hash.to_string(),
            track_name: track_name.to_string(),
            assignee: String::new(),
            in_progress: false,
            replies: vec![],
        };

        self.save(&cue)?;
        debug!("Added cue {} at {}", id, position);
        Ok(cue)
    }

    /// Add a reply to a cue.
    pub fn reply(
        &self,
        cue_id: &str,
        text: &str,
        author: &str,
        snapshot_hash: &str,
    ) -> Result<Option<CueReply>, ClavusError> {
        let mut cue = self.load(cue_id)?.ok_or_else(|| {
            ClavusError::ParseError(format!("cue not found: {}", cue_id))
        })?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let reply = CueReply {
            id: Self::new_cue_id(text, "", timestamp, author),
            text: text.to_string(),
            author: author.to_string(),
            timestamp,
            snapshot_hash: snapshot_hash.to_string(),
        };

        cue.replies.push(reply.clone());
        self.save(&cue)?;
        Ok(Some(reply))
    }

    /// Resolve a cue.
    pub fn resolve(&self, cue_id: &str, note: &str) -> Result<Option<Cue>, ClavusError> {
        let mut cue = self.load(cue_id)?.ok_or_else(|| {
            ClavusError::ParseError(format!("cue not found: {}", cue_id))
        })?;

        cue.status = CueStatus::Resolved;
        if !note.is_empty() {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            cue.replies.push(CueReply {
                id: "resolve".to_string(),
                text: format!("[ok] Resolved: {}", note),
                author: "system".to_string(),
                timestamp,
                snapshot_hash: String::new(),
            });
        }
        self.save(&cue)?;
        info!("Resolved cue {}", cue_id);
        Ok(Some(cue))
    }

    /// Skip a cue.
    pub fn skip(&self, cue_id: &str, _reason: &str) -> Result<Option<Cue>, ClavusError> {
        let mut cue = self.load(cue_id)?.ok_or_else(|| {
            ClavusError::ParseError(format!("cue not found: {}", cue_id))
        })?;
        cue.status = CueStatus::Skipped;
        self.save(&cue)?;
        Ok(Some(cue))
    }

    /// Assign a cue to someone.
    pub fn assign(&self, cue_id: &str, assignee: &str) -> Result<Option<Cue>, ClavusError> {
        let mut cue = self.load(cue_id)?.ok_or_else(|| {
            ClavusError::ParseError(format!("cue not found: {}", cue_id))
        })?;
        cue.assignee = assignee.to_string();
        cue.in_progress = false;
        self.save(&cue)?;
        Ok(Some(cue))
    }

    /// Delete a cue.
    pub fn delete(&self, cue_id: &str) -> bool {
        let path = self.cue_path(cue_id);
        if path.exists() {
            return fs::remove_file(path).is_ok();
        }
        // Try prefix scan
        if let Ok(entries) = fs::read_dir(&self.cues_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(cue_id) && name.ends_with(".json") {
                    return fs::remove_file(entry.path()).is_ok();
                }
            }
        }
        false
    }

    // ─── Persistence ─────────────────────────────────────────────

    /// Save a cue atomically.
    fn save(&self, cue: &Cue) -> Result<(), ClavusError> {
        let path = self.cue_path(&cue.id);
        let tmp = PathBuf::from(path.to_string_lossy().to_string() + ".tmp");
        let json = serde_json::to_string_pretty(cue)
            .map_err(ClavusError::JsonError)?;
        fs::write(&tmp, &json)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;
        fs::rename(&tmp, &path)
            .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;
        Ok(())
    }

    /// Load a cue by ID.
    pub fn load(&self, cue_id: &str) -> Result<Option<Cue>, ClavusError> {
        // Try exact path
        let path = self.cue_path(cue_id);
        if path.exists() {
            let data = fs::read_to_string(&path)
                .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;
            let cue: Cue = serde_json::from_str(&data)
                .map_err(ClavusError::JsonError)?;
            return Ok(Some(cue));
        }

        // Try prefix scan
        if let Ok(entries) = fs::read_dir(&self.cues_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(cue_id) && name.ends_with(".json") {
                    let data = fs::read_to_string(entry.path())
                        .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;
                    let cue: Cue = serde_json::from_str(&data)
                        .map_err(ClavusError::JsonError)?;
                    return Ok(Some(cue));
                }
            }
        }
        Ok(None)
    }

    /// List all cues, optionally filtered by status.
    pub fn list(&self, filter: Option<&str>) -> Result<Vec<Cue>, ClavusError> {
        let mut cues = Vec::new();

        if let Ok(entries) = fs::read_dir(&self.cues_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".json") {
                    continue;
                }
                let data = fs::read_to_string(entry.path())
                    .map_err(|e| ClavusError::IndexCorrupted(e.to_string()))?;
                if let Ok(cue) = serde_json::from_str::<Cue>(&data) {
                    if let Some(status) = filter {
                        if cue.status.as_str() != status {
                            continue;
                        }
                    }
                    cues.push(cue);
                }
            }
        }

        // Sort by timestamp ascending (oldest first)
        cues.sort_by(|a, b| {
            a.timestamp.partial_cmp(&b.timestamp).unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(cues)
    }

    /// Count pending/open cues.
    pub fn count_pending(&self) -> Result<usize, ClavusError> {
        Ok(self.list(Some("pending"))?.len())
    }

    /// Import a cue from a remote sync.
    pub fn import(&self, remote_cue: Cue) -> Result<ImportOutcome, ClavusError> {
        if let Some(local) = self.load(&remote_cue.id)? {
            let key = |c: &Cue| {
                format!("{}|{}|{}|{}", c.text, c.status.as_str(), c.position, c.assignee)
            };
            if key(&local) == key(&remote_cue) {
                return Ok(ImportOutcome::Unchanged);
            }
            if remote_cue.timestamp > local.timestamp {
                self.save(&remote_cue)?;
                return Ok(ImportOutcome::Imported);
            }
            // Local is newer — mark conflict
            let mut conflicted = local;
            conflicted.replies.push(CueReply {
                id: "conflict".to_string(),
                text: format!(
                    "⚠ Remote version: {} (status={})",
                    remote_cue.text,
                    remote_cue.status.as_str()
                ),
                author: "system".to_string(),
                timestamp: remote_cue.timestamp,
                snapshot_hash: String::new(),
            });
            self.save(&conflicted)?;
            Ok(ImportOutcome::Conflict)
        } else {
            self.save(&remote_cue)?;
            Ok(ImportOutcome::Imported)
        }
    }

    /// Generate a short unique ID for a new cue.
    fn new_cue_id(text: &str, position: &str, timestamp: f64, author: &str) -> String {
        let seed = format!("{}{}{}{}", text, position, timestamp, author);
        let hash = Sha256::digest(seed.as_bytes());
        // Use first 12 hex chars of SHA256 as the cue ID
        format!("{:024x}", u128::from_le_bytes(
            hash[0..16].try_into().unwrap_or([0u8; 16])
        ))
    }
}
