//! Clavus core types — shared data structures for all crates.
//!
//! This module is the **single source of truth** for types used across
//! the core, parser, sync, relay, and tui crates.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use std::default::Default;

// ─── Project ────────────────────────────────────────────────────────────

/// A tracked DAW project (Ableton Live or Bitwig Studio).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    /// Path to the .als or .bwproject file
    pub path: PathBuf,
    /// Which DAW this project belongs to
    pub project_type: ProjectType,
    /// Human-readable name (derived from file name)
    pub name: String,
    /// Current snapshot hash (HEAD of the working chain)
    pub head: Option<String>,
    /// Number of audio/MIDI tracks
    pub track_count: usize,
    /// Beats per minute
    pub bpm: f64,
    /// Time signature string e.g. "4/4"
    pub time_signature: String,
    /// Ableton/Bitwig version string (when known)
    pub ableton_version: Option<String>,
    /// Project schema version (when known)
    pub schema_version: Option<String>,
    /// Audio/MIDI tracks
    #[serde(default)]
    pub tracks: Vec<Track>,
    /// Return tracks
    #[serde(default)]
    pub return_tracks: Vec<Track>,
    /// Master track
    #[serde(default)]
    pub master_track: Option<Track>,
    /// Arrangement markers
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Tempo events
    #[serde(default)]
    pub tempo_events: Vec<TempoEvent>,
}

/// The DAW that created a project file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectType {
    Ableton,
    Bitwig,
}

impl std::fmt::Display for ProjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectType::Ableton => write!(f, "Ableton"),
            ProjectType::Bitwig => write!(f, "Bitwig"),
        }
    }
}

impl ProjectType {
    /// Detect project type from file extension.
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|s| s.to_str()) {
            Some("bwproject") => ProjectType::Bitwig,
            _ => ProjectType::Ableton,
        }
    }

    /// File extension for this project type.
    pub fn extension(&self) -> &'static str {
        match self {
            ProjectType::Ableton => ".als",
            ProjectType::Bitwig => ".bwproject",
        }
    }
}

/// A single audio, MIDI, group, or return track.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub track_type: String,    // "Audio", "MIDI", "Group", "Return", "Master"
    pub index: usize,
    pub color: u32,
    pub is_frozen: bool,
    pub is_muted: bool,
    pub is_solo: bool,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub sends: HashMap<String, f64>,  // send_name → level
    #[serde(default)]
    pub clips: Vec<Clip>,
}

/// A plugin or effect on a track.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    pub device_type: String,  // e.g. "Compressor", "Eq8", "Instrument"
}

/// A clip in the arrangement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Clip {
    pub start_beats: f64,
    pub end_beats: f64,
    pub name: String,
    pub color: u32,
    pub clip_type: String,  // "MidiClip" or "AudioClip"
}

/// An arrangement marker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Marker {
    /// Position — Ableton: "bar.beat.tix", Bitwig: "bars.beats.sixteenths"
    pub time: String,
    pub name: String,
}

/// A tempo change event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TempoEvent {
    pub time_beats: f64,
    pub bpm: f64,
}

// ─── Snapshot ─────────────────────────────────────────────────────────

/// A snapshot of a project at a point in time.
///
/// **Identity rule**: snapshot identity = SHA256 of the **raw** `.als`/`.bwproject`
/// bytes — NOT the parsed structure. Any save in the DAW that changes the file
/// produces a new snapshot, even if the parsed content looks identical.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// SHA256 of raw project file bytes (the true snapshot identity)
    pub hash: String,
    /// Parent snapshot hash. None for the first/root snapshot.
    pub parent: Option<String>,
    /// Unix timestamp when this snapshot was created
    pub timestamp: f64,
    /// User-provided message e.g. "arrangement pass 3"
    pub message: String,
    /// Track count at snapshot time
    pub track_count: usize,
    /// BPM at snapshot time
    pub bpm: f64,
    /// User-applied tags
    pub tags: Vec<String>,
    /// Longer-form session notes (markdown supported)
    pub notes: String,
    /// SHA256 hashes of all collected audio samples
    pub sample_hashes: Vec<String>,
    /// Maps sample hash → relative path from project root.
    /// Used to reconstruct sample locations on restore.
    pub sample_paths: HashMap<String, String>,
    /// Absolute path to the project file at snapshot time
    pub project_path: String,
}

impl Snapshot {
    /// Short hash prefix for display (e.g. in TUI).
    pub fn short_hash(&self, len: usize) -> String {
        self.hash.chars().take(len).collect()
    }

    /// True if this snapshot has notes attached.
    pub fn has_notes(&self) -> bool {
        !self.notes.trim().is_empty()
    }
}

// ─── Cue ──────────────────────────────────────────────────────────────

/// A timeline-anchored comment / annotation on a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cue {
    pub id: String,
    /// Position in the DAW timeline.
    /// Ableton: "4.1.1" (bar.beat.sixteenth)
    /// Bitwig: "4.1.01" (bar.beat.sixteenth, zero-padded)
    pub position: String,
    pub text: String,
    pub author: String,
    pub timestamp: f64,
    pub status: CueStatus,
    /// Snapshot hash this cue is attached to
    pub snapshot_hash: String,
    /// Optional: which track this cue is about
    pub track_name: String,
    /// Who is responsible for addressing this cue
    pub assignee: String,
    /// True if someone is actively working on this cue
    pub in_progress: bool,
    pub replies: Vec<CueReply>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CueStatus {
    Pending,
    Resolved,
    Skipped,
    Deferred,
    Archived,
}

impl CueStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CueStatus::Pending => "pending",
            CueStatus::Resolved => "resolved",
            CueStatus::Skipped => "skipped",
            CueStatus::Deferred => "deferred",
            CueStatus::Archived => "archived",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        Self::parse(s).unwrap_or(CueStatus::Pending)
    }

    /// Every valid status name, in the order they are displayed.
    pub const ALL: [&'static str; 5] = [
        "pending",
        "resolved",
        "skipped",
        "deferred",
        "archived",
    ];

    /// Parse a status name, returning `None` when it is not recognised.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(CueStatus::Pending),
            "resolved" => Some(CueStatus::Resolved),
            "skipped" => Some(CueStatus::Skipped),
            "deferred" => Some(CueStatus::Deferred),
            "archived" => Some(CueStatus::Archived),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CueReply {
    pub id: String,
    pub text: String,
    pub author: String,
    pub timestamp: f64,
    pub snapshot_hash: String,
}

// ─── Sample Audit (THE AUDIO INTEGRITY FIX) ─────────────────────────

/// A reference to an audio sample file found inside a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleRef {
    /// Path as stored in the project file XML
    pub path: PathBuf,
    /// Just the filename
    pub filename: String,
    /// Resolved absolute path on disk. None if file not found.
    pub resolved_path: Option<PathBuf>,
    /// Relative path from project root, e.g. "Samples/Kick.wav".
    /// None if the file is outside the project folder.
    pub relpath_from_project: Option<String>,
    /// SHA256 of file content. None if file not found or inaccessible.
    pub hash: Option<String>,
    pub size_bytes: u64,
}

/// Result of auditing a project's sample references before snapshotting.
///
/// Call `report.ensure_clean()` before creating a snapshot.
/// If it returns `Err`: **do not create the snapshot** — samples are missing
/// or outside the project folder. This is the core audio integrity fix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleAuditReport {
    /// Samples that were found and live inside the project folder.
    /// These are safe — included in the snapshot.
    pub collected: Vec<SampleRef>,
    /// Samples referenced in the project file but NOT found on disk.
    /// Block snapshot creation.
    pub missing: Vec<SampleRef>,
    /// Samples found on disk but outside the project folder.
    /// Block snapshot creation unless user runs `clavus collect`.
    pub outside_project: Vec<SampleRef>,
}

impl SampleAuditReport {
    /// Returns `Ok(())` if all samples are present and inside project folder.
    /// Returns `Err(SampleError)` if any sample is missing or outside project dir.
    ///
    /// **This replaces the Python `except: pass` silent failure.**
    pub fn ensure_clean(&self) -> Result<(), SampleError> {
        if !self.missing.is_empty() {
            return Err(SampleError::MissingSamples(
                self.missing.len(),
                self.missing.iter().map(|s| s.path.clone()).collect(),
            ));
        }
        if !self.outside_project.is_empty() {
            return Err(SampleError::OutsideProjectDir(
                self.outside_project.len(),
                self.outside_project.iter().map(|s| s.path.clone()).collect(),
            ));
        }
        Ok(())
    }

    /// True if the project is clean for snapshotting.
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.outside_project.is_empty()
    }

    /// Total number of sample references found.
    pub fn total(&self) -> usize {
        self.collected.len() + self.missing.len() + self.outside_project.len()
    }

    /// Total size of all collected samples in bytes.
    pub fn total_size(&self) -> u64 {
        self.collected.iter().map(|s| s.size_bytes).sum()
    }
}

/// Report from a `collect` operation (copying missing/outside samples).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectReport {
    /// Paths that were successfully copied into the project folder.
    pub copied: Vec<PathBuf>,
    /// (original path, error message) for files that failed to copy.
    pub failed: Vec<(PathBuf, String)>,
}

// ─── Error Types ─────────────────────────────────────────────────────

/// Errors from sample resolution and collection.
#[derive(Debug, Error)]
pub enum SampleError {
    #[error("{0} sample(s) not found on disk — run `clavus collect` first")]
    MissingSamples(usize, Vec<PathBuf>),

    #[error(
        "{0} sample(s) are outside the project folder — run `clavus collect --copy-missing`"
    )]
    OutsideProjectDir(usize, Vec<PathBuf>),

    #[error("IO error while accessing sample: {0}")]
    IoError(#[from] std::io::Error),
}

impl SampleError {
    /// Human-readable detail lines for all paths in this error.
    pub fn detail_lines(&self) -> Vec<String> {
        match self {
            SampleError::MissingSamples(_, paths) | SampleError::OutsideProjectDir(_, paths) => {
                paths.iter()
                    .map(|p| format!("  {}", p.display()))
                    .collect()
            }
            SampleError::IoError(e) => vec![format!("  IO error: {}", e)],
        }
    }
}

/// All Clavus error types.
/// All errors produced by clavus-core.
#[derive(Debug, Error)]
pub enum ClavusError {
    #[error("Project not found: {0}")]
    ProjectNotFound(PathBuf),

    #[error("Snapshot not found: {0}")]
    SnapshotNotFound(String),

    #[error("Ambiguous snapshot hash '{0}' — use more characters")]
    AmbiguousSnapshot(String),

    #[error("Blob not found in store: {0}")]
    BlobNotFound(String),

    #[error("Index is corrupted: {0}")]
    IndexCorrupted(String),

    #[error("Sync failed: {0}")]
    SyncError(String),

    #[error("Failed to parse project file: {0}")]
    ParseError(String),

    #[error("Sample integrity error: {0}")]
    SampleError(#[from] SampleError),

    #[error("JSON serialization error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("UTF-8 decode error: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Protocol error: {0}")]
    Protocol(String),
}
