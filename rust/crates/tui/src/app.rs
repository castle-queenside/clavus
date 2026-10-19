//! Clavus TUI application state.

use clavus_core::types::{Cue, CueStatus, Project, Snapshot};
use std::path::PathBuf;

/// Application screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    ProjectList,
    ProjectDetail,
    CueList,
    CueAdd,
    SnapshotList,
    Sync,
}

/// Main application state.
#[derive(Debug)]
pub struct App {
    /// Clavus data root (default: ~/.clavus).
    pub clavus_root: PathBuf,
    /// Projects directory.
    pub projects_dir: PathBuf,
    /// Discovered projects.
    pub projects: Vec<ProjectEntry>,
    /// Currently selected project.
    pub selected_project: Option<usize>,
    /// Project detail (parsed project).
    pub project_detail: Option<Project>,
    /// Cues for the selected project.
    pub cues: Vec<Cue>,
    /// Snapshots for the selected project.
    pub snapshots: Vec<Snapshot>,
    /// Cue filter.
    pub cue_filter: Option<CueStatus>,
    /// Current screen.
    pub screen: Screen,
    /// Sync state.
    pub sync_state: SyncState,
    /// Error message to display.
    pub error: Option<String>,
    /// Info message.
    pub info: Option<String>,
    /// Input buffer (for cue text entry).
    pub input_buf: String,
    /// Loading indicator.
    pub loading: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    pub project_type: String,
    pub head: Option<String>,
    pub bpm: f64,
    pub track_count: usize,
    pub modified_secs: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SyncState {
    pub connected: bool,
    pub relay_addr: Option<String>,
    pub last_sync: Option<String>,
}

impl App {
    pub fn new(projects_dir: PathBuf) -> Self {
        let clavus_root = dirs::home_dir()
            .map(|h| h.join(".clavus"))
            .unwrap_or_else(|| PathBuf::from(".clavus"));
        Self {
            clavus_root: clavus_root.clone(),
            projects_dir,
            projects: vec![],
            selected_project: None,
            project_detail: None,
            cues: vec![],
            snapshots: vec![],
            cue_filter: None,
            screen: Screen::ProjectList,
            sync_state: SyncState::default(),
            error: None,
            info: None,
            input_buf: String::new(),
            loading: false,
        }
    }

    pub fn refresh_projects(&mut self) -> Result<(), String> {
        let mut entries = Vec::new();

        let dir = std::fs::read_dir(&self.projects_dir)
            .map_err(|e| format!("Cannot read {}: {}", self.projects_dir.display(), e))?;

        for entry in dir.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");

            if ext == "als" || ext == "bwproject" {
                let name = path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();

                let modified_secs = entry.metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
                    .unwrap_or(0);

                let project_type = if ext == "als" { "Ableton" } else { "Bitwig" };

                entries.push(ProjectEntry {
                    path,
                    name,
                    project_type: project_type.to_string(),
                    head: None,
                    bpm: 120.0,
                    track_count: 0,
                    modified_secs,
                });
            }
        }

        // Sort by modified time descending
        entries.sort_by_key(|a| std::cmp::Reverse(a.modified_secs));
        self.projects = entries;
        Ok(())
    }

    pub fn load_project(&mut self, idx: usize) -> Result<(), String> {
        if idx >= self.projects.len() {
            return Err("Invalid project index".into());
        }

        let entry = &self.projects[idx];
        let ext = entry.path.extension().and_then(|s| s.to_str()).unwrap_or("");

        self.loading = true;
        let project = if ext == "als" {
            clavus_parser_als::AlsParser::new(&entry.path)
                .map(|p| p.project)
                .map_err(|e| format!("Parse error: {}", e))?
        } else if ext == "bwproject" {
            clavus_parser_bw::BitwigParser::new(&entry.path)
                .map(|p| p.project)
                .map_err(|e| format!("Parse error: {}", e))?
        } else {
            return Err("Unknown project type".into());
        };

        self.project_detail = Some(project);
        self.selected_project = Some(idx);
        self.loading = false;
        Ok(())
    }

    pub fn selected_entry(&self) -> Option<&ProjectEntry> {
        self.selected_project.and_then(|i| self.projects.get(i))
    }

    pub fn set_error(&mut self, msg: String) {
        self.error = Some(msg);
    }

    pub fn set_info(&mut self, msg: String) {
        self.info = Some(msg);
    }

    pub fn clear_messages(&mut self) {
        self.error = None;
        self.info = None;
    }

    /// Load cues for the selected project from the cue store.
    pub fn load_cues(&mut self) -> Result<(), String> {
        let entry = self.selected_entry()
            .ok_or_else(|| "No project selected".to_string())?;

        let store = clavus_core::CueStore::new(&self.clavus_root, &entry.name)
            .map_err(|e| format!("CueStore: {}", e))?;

        let filter = self.cue_filter.as_ref().map(|s| s.as_str());
        self.cues = store
            .list(filter)
            .map_err(|e| format!("list cues: {}", e))?;
        Ok(())
    }

    /// Load the snapshot chain for the selected project.
    pub fn load_snapshots(&mut self) -> Result<(), String> {
        use clavus_core::blob::BlobStore;
        use clavus_core::snapshot::SnapshotStore;

        let entry = self.selected_entry()
            .ok_or_else(|| "No project selected".to_string())?;

        let store = BlobStore::new(self.clavus_root.join("blobs"))
            .map_err(|e| format!("BlobStore: {}", e))?;
        let snap_store = SnapshotStore::new(store, &entry.name);

        if let Some(head) = snap_store.head() {
            self.snapshots = snap_store.walk_chain(Some(&head.hash));
        } else {
            self.snapshots = vec![];
        }
        Ok(())
    }
}
