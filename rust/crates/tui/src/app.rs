//! Clavus TUI application state.

use clavus_core::blob::BlobStore;
use clavus_core::snapshot::SnapshotStore;
use clavus_core::types::{Cue, CueStatus, Project, Snapshot};
use std::path::PathBuf;
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
    pub selected_cue: Option<usize>,
    /// Snapshots for the selected project.
    pub snapshots: Vec<Snapshot>,
    pub selected_snapshot: Option<usize>,
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
    /// Push the given local snapshots and cues to the relay.
    Push {
        snapshots: Vec<clavus_core::types::Snapshot>,
        cues: Vec<clavus_core::types::Cue>,
    },
    /// Pull snapshots the relay has that we don't, given our known heads.
    ///
    /// The project name and data root are needed so the background thread
    /// can store what it receives rather than only counting it.
    Pull {
        known_heads: Vec<String>,
        project_name: String,
        clavus_root: std::path::PathBuf,
    },
}

#[derive(Debug, Default)]
pub struct SyncState {
    /// True only once the background thread confirms a live connection.
    pub connected: bool,
    pub relay_addr: Option<String>,
    pub last_sync: Option<String>,
    /// Sender to the background sync thread. None when disconnected.
    pub(crate) cmd_tx: Option<Sender<SyncCmd>>,
    /// Status messages from the background sync thread.
    pub(crate) result_rx: Option<mpsc::Receiver<String>>,
}

impl SyncState {
    /// Returns true if we have an active connection.
    pub fn is_connected(&self) -> bool {
        self.cmd_tx.is_some() && self.connected
    }

    /// Send a command to the sync thread. Returns true if sent.
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
            selected_cue: None,
            snapshots: vec![],
            selected_snapshot: None,
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

    /// The name of the currently selected project, if any.
    pub fn selected_project_name(&self) -> Option<String> {
        self.selected_entry().map(|e| e.name.clone())
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

        self.selected_cue = None;
        let filter = self.cue_filter.as_ref().map(|s| s.as_str());
        self.cues = store
            .list(filter)
            .map_err(|e| format!("list cues: {}", e))?;
        Ok(())
    }

    /// Every cue for the selected project, ignoring the current filter.
    ///
    /// Used when pushing, so a collaborator receives all the notes and
    /// not just the ones currently on screen.
    pub fn project_cues(&self) -> Vec<clavus_core::types::Cue> {
        let Some(entry) = self.selected_entry() else {
            return Vec::new();
        };
        let Ok(store) = clavus_core::CueStore::new(&self.clavus_root, &entry.name) else {
            return Vec::new();
        };
        store.list(None).unwrap_or_default()
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
        self.selected_snapshot = None;
        Ok(())
    }

    /// The local snapshot chain for the selected project, ready to push.
    pub fn selected_snapshots(&self) -> Vec<clavus_core::types::Snapshot> {
        use clavus_core::blob::BlobStore;
        use clavus_core::snapshot::SnapshotStore;

        let entry = match self.selected_entry() {
            Some(e) => e,
            None => return vec![],
        };
        let store = match BlobStore::new(self.clavus_root.join("blobs")) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let snap_store = SnapshotStore::new(store, &entry.name);
        match snap_store.head() {
            Some(head) => snap_store.walk_chain(Some(&head.hash)),
            None => vec![],
        }
    }

    /// The snapshot heads we already have, used to ask the relay only for
    /// what we are missing.
    pub fn known_heads(&self) -> Vec<String> {
        use clavus_core::blob::BlobStore;
        use clavus_core::snapshot::SnapshotStore;

        let entry = match self.selected_entry() {
            Some(e) => e,
            None => return vec![],
        };
        let store = match BlobStore::new(self.clavus_root.join("blobs")) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let snap_store = SnapshotStore::new(store, &entry.name);
        snap_store
            .head()
            .map(|h| vec![h.hash])
            .unwrap_or_default()
    }

    /// Connect to a relay server, spawning a background sync thread.
    pub fn sync_connect(&mut self, addr: &str) {
        use clavus_sync::SyncClient;

        // Disconnect existing first.
        if let Some(tx) = self.sync_state.cmd_tx.take() {
            let _ = tx.send(SyncCmd::Disconnect);
        }
        self.sync_state.connected = false;
        self.sync_state.relay_addr = None;

        let (cmd_tx, cmd_rx) = mpsc::channel::<SyncCmd>();
        let addr_owned = addr.to_string();

        // Result channel: the background thread reports connection outcomes
        // here so the UI can show failures instead of assuming success.
        let (result_tx, result_rx) = mpsc::channel::<String>();

        // Spawn the background sync thread.
        thread::spawn(move || {
            // Use Option to allow re-binding client on reconnect.
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

            let _ = result_tx.send(format!("connected to {}", addr_owned));

            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    SyncCmd::Disconnect => {
                        if let Some(c) = client.take() {
                            c.close();
                        }
                        let _ = result_tx.send("disconnected".to_string());
                        break;
                    }
                    SyncCmd::Push { snapshots, cues } => {
                        if let Some(ref c) = client {
                            let count = snapshots.len();
                            let cue_count = cues.len();
                            match c.push(snapshots, cues) {
                                Ok(()) => {
                                    // Wait for the relay's acknowledgement.
                                    match c.recv_timeout(std::time::Duration::from_secs(10)) {
                                        Some(clavus_sync::client::SyncMsg::Synced) => {
                                            let _ = result_tx.send(format!("pushed {} snapshot(s), {} cue(s)", count, cue_count));
                                        }
                                        Some(clavus_sync::client::SyncMsg::Conflict { project, .. }) => {
                                            let _ = result_tx.send(format!("conflict on '{}' — pull first", project));
                                        }
                                        Some(_) => {
                                            let _ = result_tx.send(format!("pushed {} snapshot(s), {} cue(s)", count, cue_count));
                                        }
                                        None => {
                                            let _ = result_tx.send("push timed out".to_string());
                                        }
                                    }
                                }
                                Err(e) => {
                                    let _ = result_tx.send(format!("push failed: {}", e));
                                }
                            }
                        } else {
                            let _ = result_tx.send("not connected".to_string());
                        }
                    }
                    SyncCmd::Pull {
                        known_heads,
                        project_name,
                        clavus_root,
                    } => {
                        if let Some(ref c) = client {
                            // The relay replies to Hello with a Push of
                            // everything we are missing.
                            if let Err(e) = c.hello("clavus-tui", known_heads) {
                                let _ = result_tx.send(format!("pull failed: {}", e));
                                continue;
                            }
                            match c.recv_timeout(std::time::Duration::from_secs(10)) {
                                Some(clavus_sync::client::SyncMsg::Push { snapshots, cues }) => {
                                    // Store what arrived, so a pull actually
                                    // brings the collaborator's work in.
                                    let (stored, imported) =
                                        store_pulled(&snapshots, &cues, &clavus_root, &project_name);
                                    let _ = result_tx.send(format!(
                                        "pulled {} snapshot(s), {} cue(s)",
                                        stored, imported
                                    ));
                                }
                                Some(_) => {
                                    let _ = result_tx.send("pulled (nothing new)".to_string());
                                }
                                None => {
                                    let _ = result_tx.send("pull timed out".to_string());
                                }
                            }
                        } else {
                            let _ = result_tx.send("not connected".to_string());
                        }
                    }
                    SyncCmd::Connect(new_addr) => {
                        // Close the old connection, then reconnect.
                        if let Some(c) = client.take() {
                            c.close();
                        }
                        client = match SyncClient::connect(&new_addr) {
                            Ok(c) => {
                                let _ = c.hello("clavus-tui", vec![]);
                                let _ = result_tx.send(format!("connected to {}", new_addr));
                                Some(c)
                            }
                            Err(e) => {
                                let _ = result_tx.send(format!("connect failed: {}", e));
                                None
                            }
                        };
                    }
                }
            }
        });

        // Store the command sender. The `connected` flag stays false until the
        // background thread confirms the connection.
        self.sync_state.cmd_tx = Some(cmd_tx);
        self.sync_state.result_rx = Some(result_rx);
        self.sync_state.relay_addr = Some(addr.to_string());
        self.set_info(format!("connecting to {}...", addr));
    }

    /// Drain status messages from the background sync thread and update the
    /// UI state accordingly. Safe to call frequently.
    pub fn poll_sync(&mut self) {
        let messages: Vec<String> = match self.sync_state.result_rx.as_ref() {
            Some(rx) => {
                let mut out = Vec::new();
                while let Ok(msg) = rx.try_recv() {
                    out.push(msg);
                }
                out
            }
            None => return,
        };

        for msg in messages {
            if let Some(addr) = msg.strip_prefix("connected to ") {
                self.sync_state.connected = true;
                self.set_info(format!("connected to {}", addr));
            } else if let Some(err) = msg.strip_prefix("connect failed: ") {
                self.sync_state.connected = false;
                self.set_error(format!("connect failed: {}", err));
            } else if let Some(err) = msg.strip_prefix("hello failed: ") {
                self.sync_state.connected = false;
                self.set_error(format!("hello failed: {}", err));
            } else if msg == "disconnected" {
                self.sync_state.connected = false;
                self.set_info("disconnected".into());
            } else {
                self.set_info(msg);
            }
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

/// Store snapshots and cues that arrived from a relay.
///
/// Returns `(snapshots stored, cues imported)`. Everything that arrives is
/// untrusted, so the store validates hashes and cue ids before writing.
pub fn store_pulled(
    snapshots: &[clavus_core::types::Snapshot],
    cues: &[clavus_core::types::Cue],
    clavus_root: &std::path::Path,
    project_name: &str,
) -> (usize, usize) {
    use clavus_core::blob::BlobStore;
    use clavus_core::cue::{CueStore, ImportOutcome};
    use clavus_core::snapshot::SnapshotStore;

    let mut stored = 0;
    let mut imported = 0;

    if !snapshots.is_empty() {
        if let Ok(blobs) = BlobStore::new(clavus_root.join("blobs")) {
            let snap_store = SnapshotStore::new(blobs, project_name);
            for snap in snapshots {
                if snap_store.has(&snap.hash) {
                    continue;
                }
                if snap_store.save_raw(snap.clone()).is_ok() {
                    stored += 1;
                }
            }
            // Advance HEAD to the newest snapshot the relay sent.
            if let Some(newest) = snapshots.last() {
                let _ = snap_store.set_head(&newest.hash, "pull");
            }
        }
    }

    if !cues.is_empty() {
        if let Ok(store) = CueStore::new(clavus_root, project_name) {
            for cue in cues {
                if matches!(store.import(cue.clone()), Ok(ImportOutcome::Imported)) {
                    imported += 1;
                }
            }
        }
    }

    (stored, imported)
}
