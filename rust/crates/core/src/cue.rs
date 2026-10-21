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
    /// The project these cues belong to, stamped onto every cue so the
    /// relay can group them correctly.
    project_name: String,
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
        Ok(Self {
            cues_dir,
            project_name: project_name.to_string(),
        })
    }

    fn cue_path(&self, id: &str) -> PathBuf {
        self.cues_dir.join(format!("{}.json", id))
    }

    /// Validate a cue identifier.
    ///
    /// A cue id becomes a file name, so it must not be able to escape the
    /// project's cue directory. Cues arrive from remote peers over the
    /// sync protocol, so a hostile peer must not be able to read or write
    /// arbitrary files by sending a crafted id.
    pub fn validate_cue_id(id: &str) -> Result<(), ClavusError> {
        if id.is_empty() {
            return Err(ClavusError::ParseError("cue id must not be empty".into()));
        }
        if id.len() > 128 {
            return Err(ClavusError::ParseError(
                "cue id is too long (max 128 characters)".into(),
            ));
        }
        if !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ClavusError::ParseError(format!(
                "cue id may only contain letters, digits, '-' and '_': {:?}",
                id
            )));
        }
        Ok(())
    }

    /// Whether `id` is usable as a cue id.
    pub fn is_valid_cue_id(id: &str) -> bool {
        Self::validate_cue_id(id).is_ok()
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
            project: self.project_name.clone(),
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
        // An id that could escape the cue directory is never a cue.
        if Self::validate_cue_id(cue_id).is_err() {
            return false;
        }
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
        Self::validate_cue_id(&cue.id)?;
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
        Self::validate_cue_id(cue_id)?;
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
        // An unrecognised status must not silently match nothing — the
        // user would see "No cues." and think they have none.
        if let Some(status) = filter {
            if CueStatus::parse(status).is_none() {
                return Err(ClavusError::ParseError(format!(
                    "unknown cue status '{}' (expected one of: {})",
                    status,
                    CueStatus::ALL.join(", ")
                )));
            }
        }
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
        // The cue comes from a remote peer — never trust its id.
        Self::validate_cue_id(&remote_cue.id)?;
        // The cue now lives in this project's store, so that is the
        // project it belongs to, whatever the sender said.
        let mut remote_cue = remote_cue;
        remote_cue.project = self.project_name.clone();
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
