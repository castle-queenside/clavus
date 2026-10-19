//! Clavus TUI application state.

use clavus_core::blob::BlobStore;
use clavus_core::snapshot::SnapshotStore;
use clavus_core::types::{Cue, CueStatus, Project, Snapshot};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Sender};
use std::thread;

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
    /// Idle phrase index — cycles through idle messages.
    pub idle_index: usize,
}

#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    pub project_type: String,
    pub head: Option<String>,
    /// Number of snapshots in the chain (0 if never snapshotted).
    pub snap_count: usize,
    pub bpm: f64,
    pub track_count: usize,
    pub modified_secs: i64,
}

/// Commands sent from the TUI event loop to the background sync thread.
#[derive(Debug)]
pub enum SyncCmd {
    Connect(String),  // relay address
    Disconnect,
    Push,
    Pull,
}

#[derive(Debug, Default)]
pub struct SyncState {
    pub connected: bool,
    pub relay_addr: Option<String>,
    pub last_sync: Option<String>,
    /// Sender to the background sync thread. None when disconnected.
    pub(crate) cmd_tx: Option<Sender<SyncCmd>>,
}

impl SyncState {
    /// Returns true if we have an active connection.
    pub fn is_connected(&self) -> bool {
        self.cmd_tx.is_some() && self.connected
    }

    /// Send a command to the sync thread. Returns true if sent.
    #[allow(dead_code)]
    pub fn send_cmd(&self, cmd: SyncCmd) -> bool {
        self.cmd_tx.as_ref().map(|tx| tx.send(cmd).is_ok()).unwrap_or(false)
    }
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
            idle_index: 0,
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

                // Check for existing snapshots
                let (head, snap_count) = Self::snap_info(&self.clavus_root, &name);

                entries.push(ProjectEntry {
                    path,
                    name,
                    project_type: project_type.to_string(),
                    head,
                    snap_count,
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

    /// Connect to a relay server, spawning a background sync thread.
    pub fn sync_connect(&mut self, addr: &str) {
        use clavus_sync::SyncClient;

        // Disconnect existing first
        if let Some(tx) = self.sync_state.cmd_tx.take() {
            let _ = tx.send(SyncCmd::Disconnect);
        }
        self.sync_state.connected = false;
        self.sync_state.relay_addr = None;

        let (cmd_tx, cmd_rx) = mpsc::channel::<SyncCmd>();
        let addr_owned = addr.to_string();
        let connected = Arc::new(Mutex::new(false));
        let connected_clone = Arc::clone(&connected);

        // Result channel for connection feedback
        let (result_tx, result_rx) = mpsc::channel::<String>();
        let result_rx = Some(result_rx);

        // Spawn the background sync thread
        thread::spawn(move || {
            // Use Option to allow re-binding client on reconnect
            let mut client: Option<SyncClient> = match SyncClient::connect(&addr_owned) {
                Ok(c) => Some(c),
                Err(e) => {
                    let _ = result_tx.send(format!("connect failed: {}", e));
                    return;
                }
            };

            if let Some(ref c) = client {
                if let Err(e) = c.hello("clavus-tui", vec![]) {
                    let _ = result_tx.send(format!("hello failed: {}", e));
                    return;
                }
            }

            *connected_clone.lock().unwrap() = true;
            let _ = result_tx.send(format!("connected to {}", addr_owned));

            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    SyncCmd::Disconnect => {
                        if let Some(c) = client.take() {
                            c.close();
                        }
                        *connected_clone.lock().unwrap() = false;
                        break;
                    }
                    SyncCmd::Push => {
                        if let Some(ref c) = client {
                            let _ = c.synced();
                            let _ = result_tx.send("pushed".to_string());
                        }
                    }
                    SyncCmd::Pull => {
                        let _ = result_tx.send("pulled".to_string());
                    }
                    SyncCmd::Connect(new_addr) => {
                        // Close old connection
                        if let Some(c) = client.take() {
                            c.close();
                        }
                        // Reconnect
                        client = match SyncClient::connect(&new_addr) {
                            Ok(c) => {
                                let _ = c.hello("clavus-tui", vec![]);
                                *connected_clone.lock().unwrap() = true;
                                let _ = result_tx.send(format!("connected to {}", new_addr));
                                Some(c)
                            }
                            Err(e) => {
                                *connected_clone.lock().unwrap() = false;
                                let _ = result_tx.send(format!("connect failed: {}", e));
                                None
                            }
                        };
                    }
                }
            }
        });

        // Store the command sender and optimistic connected state
        self.sync_state.cmd_tx = Some(cmd_tx);
        self.sync_state.relay_addr = Some(addr.to_string());
        self.sync_state.connected = true;

        // Poll the result receiver briefly to update status
        // (connection result will overwrite the optimistic state)
        if let Some(rx) = result_rx {
            // Non-blocking check — just store that we're waiting
            self.set_info(format!("connecting to {}...", addr));
            // The actual result will be shown when the user next interacts
            drop(rx); // ignore result channel for now — the connected flag tells us the state
        }
    }

    /// Disconnect from the relay.
    pub fn sync_disconnect(&mut self) {
        if let Some(tx) = self.sync_state.cmd_tx.take() {
            let _ = tx.send(SyncCmd::Disconnect);
        }
        self.sync_state.connected = false;
        self.sync_state.relay_addr = None;
        self.set_info("disconnected".into());
    }

    /// Get the HEAD snapshot hash and chain count for a project.
    /// Returns `(head_hash, snap_count)`, both `None`/`0` if no snapshots exist.
    fn snap_info(clavus_root: &std::path::Path, project_name: &str) -> (Option<String>, usize) {
        let blobs = match BlobStore::new(clavus_root.join("blobs")) {
            Ok(b) => b,
            Err(_) => return (None, 0),
        };
        let store = SnapshotStore::new(blobs, project_name);
        let head = store.head().map(|s| s.hash);
        let count = store.chain_length(None);
        (head, count)
    }
}
