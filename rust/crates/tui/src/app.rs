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
    /// Typing a relay address.
    RelayInput,
}

/// Which field of the cue form has the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueStep {
    /// The message the cue carries.
    Message,
    /// Where in the arrangement the cue sits.
    Position,
}

/// The position a new cue gets when the user does not type one.
pub const DEFAULT_CUE_POSITION: &str = "1.1.1";

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
    /// The message of the cue being typed.
    pub cue_message: String,
    /// Which field of the cue form has the focus.
    pub cue_step: CueStep,
    /// The relay the TUI talks to.
    pub relay_addr: String,
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

impl ProjectEntry {
    /// The project's identity.
    ///
    /// Two projects that share a file name are two different projects, so
    /// everything that is stored per project — the HEAD, the snapshot
    /// records, the cues — is keyed by this identity and not by the name.
    pub fn id(&self) -> clavus_core::project::ProjectId {
        clavus_core::project::ProjectId::new(&self.path)
    }
}

/// Commands sent from the TUI event loop to the background sync thread.
#[derive(Debug)]
pub enum SyncCmd {
    /// Connect to a relay, optionally for a named project.
    Connect {
        addr: String,
        project: Option<String>,
        /// The name of the folder the project lives in.
        folder: Option<String>,
    },
    Disconnect,
    /// Tell the relay which project we are working on.
    SetProject(String, String, Option<String>),
    /// Push the given local snapshots and cues to the relay.
    Push {
        snapshots: Vec<clavus_core::types::Snapshot>,
        cues: Vec<clavus_core::types::Cue>,
        /// The project bytes, as (hash, bytes). Without them a
        /// collaborator could see the snapshot but not restore it.
        blobs: Vec<(String, Vec<u8>)>,
        /// The project these belong to — the relay keys its storage by
        /// project name, so this has to be right.
        project: Option<String>,
    },
    /// Pull snapshots the relay has that we don't, given our known heads.
    ///
    /// The project's path and the data root are needed so the background
    /// thread can store what it receives rather than only counting it.
    Pull {
        known_heads: Vec<String>,
        project_path: std::path::PathBuf,
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
            cue_message: String::new(),
            cue_step: CueStep::Message,
            relay_addr: "127.0.0.1:4477".to_string(),
        }
    }

    /// The screen to return to when the user backs out of this one.
    ///
    /// Esc used to jump straight to the project list from everywhere,
    /// so a user in the cue list could not step back to the project
    /// they were working on.
    pub fn previous_screen(&self) -> Screen {
        match self.screen {
            Screen::CueAdd | Screen::RelayInput => match self.screen {
                Screen::CueAdd => Screen::CueList,
                _ => Screen::Sync,
            },
            Screen::CueList | Screen::SnapshotList | Screen::Sync => Screen::ProjectDetail,
            Screen::ProjectDetail => Screen::ProjectList,
            Screen::ProjectList => Screen::ProjectList,
        }
    }

    /// The relay address the TUI uses.
    pub fn relay_address(&self) -> &str {
        &self.relay_addr
    }

    /// Remember the relay address to talk to.
    pub fn set_relay_address(&mut self, addr: String) {
        self.relay_addr = addr;
    }

    /// Find every project under the projects directory.
    ///
    /// The scan is recursive, so the TUI shows the same projects the CLI
    /// shows with `clavus projects --recursive`. It was not recursive
    /// before, and a project in a sub-folder was invisible.
    pub fn refresh_projects(&mut self) -> Result<(), String> {
        // A projects directory that cannot be read is an error — the
        // user needs to know that the TUI is looking in the wrong place.
        std::fs::read_dir(&self.projects_dir).map_err(|e| {
            format!("Cannot read {}: {}", self.projects_dir.display(), e)
        })?;

        let mut entries = Vec::new();
        self.scan_dir(&self.projects_dir, 0, &mut entries);

        // Sort by modified time descending
        entries.sort_by_key(|a| std::cmp::Reverse(a.modified_secs));
        self.projects = entries;
        Ok(())
    }

    /// How deep the project scan goes.
    const MAX_SCAN_DEPTH: usize = 12;

    fn scan_dir(&self, dir: &std::path::Path, depth: usize, out: &mut Vec<ProjectEntry>) {
        if depth > Self::MAX_SCAN_DEPTH {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            if ext == "als" || ext == "bwproject" {
                if let Some(project_entry) = self.project_entry(&path, &ext) {
                    out.push(project_entry);
                }
            } else if path.is_dir() {
                // A symlink loop must not make the scan run forever.
                let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                if real.is_dir() {
                    self.scan_dir(&path, depth + 1, out);
                }
            }
        }
    }

    /// Build a list entry for one project file.
    fn project_entry(&self, path: &std::path::Path, ext: &str) -> Option<ProjectEntry> {
        let name = path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let modified_secs = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        let project_type = if ext == "als" { "Ableton" } else { "Bitwig" };

        // Check for existing snapshots
        let (head, snap_count) = Self::snap_info(&self.clavus_root, path);

        // The list used to show 120 BPM and 0 tracks for every project —
        // numbers that are known to be false. The project is parsed so
        // the list agrees with `clavus status`.
        let (bpm, track_count) = match Self::parse_summary(path, ext) {
            Some((bpm, tracks)) => (bpm, tracks),
            None => (0.0, 0),
        };

        Some(ProjectEntry {
            path: path.to_path_buf(),
            name,
            project_type: project_type.to_string(),
            head,
            snap_count,
            bpm,
            track_count,
            modified_secs,
        })
    }

    /// The project's tempo and track count, read from the project file.
    fn parse_summary(path: &std::path::Path, ext: &str) -> Option<(f64, usize)> {
        let project = if ext == "als" {
            clavus_parser_als::AlsParser::new(path).ok()?.project
        } else {
            clavus_parser_bw::BitwigParser::new(path).ok()?.project
        };
        Some((project.bpm, project.tracks.len()))
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

        // Tell the relay which project we switched to, so it serves the
        // right history and stores our pushes under the right name.
        let name = self.selected_project_sync_name();
        let folder = self.selected_project_folder().unwrap_or_default();
        let hash = self
            .selected_entry()
            .and_then(|e| clavus_core::blob::BlobStore::content_hash_of_file(&e.path));
        if let (Some(name), Some(tx)) = (name, self.sync_state.cmd_tx.as_ref()) {
            let _ = tx.send(SyncCmd::SetProject(name, folder, hash));
        }
        Ok(())
    }

    pub fn selected_entry(&self) -> Option<&ProjectEntry> {
        self.selected_project.and_then(|i| self.projects.get(i))
    }

    /// The name of the currently selected project, if any.
    pub fn selected_project_name(&self) -> Option<String> {
        self.selected_entry().map(|e| e.name.clone())
    }

    /// The name the relay knows this project by.
    ///
    /// It is the file name with its extension, so a collaborator on
    /// another machine recognises the same project, while an Ableton set
    /// and a Bitwig bundle with the same stem stay apart.
    pub fn selected_project_sync_name(&self) -> Option<String> {
        self.selected_entry()
            .map(|e| e.id().sync_name().to_string())
    }

    /// The name of the folder the selected project lives in.
    ///
    /// It is the second half of the project's identity on the wire: two
    /// projects can share a file name, and the folder tells them apart.
    pub fn selected_project_folder(&self) -> Option<String> {
        self.selected_entry()
            .map(|e| e.id().folder().to_string())
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

        let store = clavus_core::CueStore::new(&self.clavus_root, &entry.id())
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
        let Ok(store) = clavus_core::CueStore::new(&self.clavus_root, &entry.id()) else {
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
        let snap_store = SnapshotStore::new(store, &entry.id());

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
        let snap_store = SnapshotStore::new(store, &entry.id());
        match snap_store.head() {
            Some(head) => snap_store.walk_chain(Some(&head.hash)),
            None => vec![],
        }
    }

    /// The project bytes of the selected project's snapshots.
    ///
    /// A snapshot without its bytes cannot be restored on the other
    /// machine, so the bytes travel with the metadata.
    pub fn selected_snapshot_bytes(&self) -> Vec<(String, Vec<u8>)> {
        use clavus_core::blob::BlobStore;

        let store = BlobStore::open(self.clavus_root.join("blobs"));
        self.selected_snapshots()
            .into_iter()
            .filter_map(|snap| {
                store.get(&snap.hash).map(|bytes| (snap.hash.clone(), bytes))
            })
            .collect()
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
        let snap_store = SnapshotStore::new(store, &entry.id());
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

        // The project we are working on, so the relay serves us the
        // right history.
        let initial_project = self.selected_project_sync_name();
        let initial_folder = self.selected_project_folder();
        // The hash of the project file. It is the hash of a snapshot of
        // it, so the relay can tell which project this is even when
        // the file sits in a different folder on another machine.
        let initial_hash = self
            .selected_entry()
            .and_then(|e| clavus_core::blob::BlobStore::content_hash_of_file(&e.path));

        // Spawn the background sync thread.
        thread::spawn(move || {
            // The relay keys its storage by project name, so the client
            // has to introduce itself with the project it is working on.
            // A placeholder is used until a project is chosen, so the
            // connection is not rejected before then.
            let mut project: Option<String> = initial_project.clone();
            // The folder tells two projects that share a file name
            // apart, so the relay never serves one project's history to
            // the other.
            let mut folder: Option<String> = initial_folder.clone();
            let mut content_hash: Option<String> = initial_hash.clone();

            // Use Option to allow re-binding client on reconnect.
            let mut client: Option<SyncClient> = match SyncClient::connect(&addr_owned) {
                Ok(c) => Some(c),
                Err(e) => {
                    let _ = result_tx.send(format!("connect failed: {}", e));
                    return;
                }
            };

            if let Some(ref c) = client {
                if let Err(e) = c.hello_in(
                    project.as_deref().unwrap_or("clavus-tui"),
                    vec![],
                    folder.as_deref().unwrap_or(""),
                    content_hash.clone(),
                ) {
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
                    SyncCmd::SetProject(name, new_folder, new_hash) => {
                        project = Some(name.clone());
                        folder = Some(new_folder.clone());
                        content_hash = new_hash.clone();
                        // Re-introduce ourselves so the relay serves the
                        // new project's history from now on.
                        if let Some(ref c) = client {
                            let _ = c.hello_in(
                                project.as_deref().unwrap_or("clavus-tui"),
                                vec![],
                                folder.as_deref().unwrap_or(""),
                                content_hash.clone(),
                            );
                        }
                    }
                    SyncCmd::Push {
                        snapshots,
                        cues,
                        blobs,
                        project: push_project,
                    } => {
                        if let Some(ref c) = client {
                            if let Some(p) = push_project {
                                project = Some(p.clone());
                            }
                            let count = snapshots.len();
                            let cue_count = cues.len();
                            // The project bytes travel first, so the
                            // relay has them by the time it learns
                            // about the snapshot.
                            let push_name = project.clone().unwrap_or_default();
                            for (hash, bytes) in &blobs {
                                if let Err(e) = c.push_blob(&push_name, hash, bytes) {
                                    let _ = result_tx.send(format!("could not send the project bytes: {}", e));
                                    break;
                                }
                            }
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
                        project_path,
                        clavus_root,
                    } => {
                        if let Some(ref c) = client {
                            // The relay replies to Hello with a Push of
                            // everything we are missing.
                            let project_id =
                                clavus_core::project::ProjectId::new(&project_path);
                            let pull_name = project_id.sync_name().to_string();
                            if let Err(e) = c.hello_in(
                                pull_name.as_str(),
                                known_heads,
                                project_id.folder(),
                                clavus_core::blob::BlobStore::content_hash_of_file(&project_path),
                            ) {
                                let _ = result_tx.send(format!("pull failed: {}", e));
                                continue;
                            }
                            match c.recv_timeout(std::time::Duration::from_secs(10)) {
                                Some(clavus_sync::client::SyncMsg::Push { snapshots, cues }) => {
                                    // Store what arrived, so a pull actually
                                    // brings the collaborator's work in.
                                    // The project bytes come too, so the
                                    // snapshot can be restored here and
                                    // not only seen.
                                    for snap in &snapshots {
                                        if store_has_blob(&clavus_root, &snap.hash) {
                                            continue;
                                        }
                                        let _ = c.request_blob(
                                            project_id.sync_name(),
                                            &snap.hash,
                                        );
                                        if let Ok(Ok(bytes)) = c.recv_blob(
                                            project_id.sync_name(),
                                            &snap.hash,
                                            std::time::Duration::from_secs(30),
                                        ) {
                                            store_put_blob(&clavus_root, &bytes);
                                        }
                                    }
                                    let (stored, imported) =
                                        store_pulled(&snapshots, &cues, &clavus_root, &project_id);
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
                    SyncCmd::Connect {
                        addr: new_addr,
                        project: new_project,
                        folder: new_folder,
                    } => {
                        if let Some(p) = new_project {
                            project = Some(p);
                        }
                        if let Some(f) = new_folder {
                            folder = Some(f);
                        }
                        // Close the old connection, then reconnect.
                        if let Some(c) = client.take() {
                            c.close();
                        }
                        client = match SyncClient::connect(&new_addr) {
                            Ok(c) => {
                                let _ = c.hello_in(
                                    project.as_deref().unwrap_or("clavus-tui"),
                                    vec![],
                                    folder.as_deref().unwrap_or(""),
                                    content_hash.clone(),
                                );
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
                // A push or a pull that worked is worth remembering: the
                // sync panel said "last sync never" even after a
                // successful sync.
                if msg.starts_with("pushed ") || msg.starts_with("pulled ") {
                    self.sync_state.last_sync = Some(msg.clone());
                    // A pull changes what the project list shows (the
                    // snapshot count and the head), so the list is
                    // refreshed — otherwise the TUI and the CLI disagree
                    // about the same project.
                    if msg.starts_with("pulled ") {
                        let _ = self.refresh_projects();
                    }
                }
                self.set_info(msg);
            }
        }
    }

    /// Advance the interface's own clock.
    ///
    /// The idle phrases are meant to rotate, and the spectrum analyzer
    /// is meant to move. Both are driven from here, so the screen is
    /// alive even when the user is not pressing keys.
    pub fn tick(&mut self) {
        self.idle_index = self.idle_index.wrapping_add(1);
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
    fn snap_info(clavus_root: &std::path::Path, project_path: &std::path::Path) -> (Option<String>, usize) {
        let blobs = match BlobStore::new(clavus_root.join("blobs")) {
            Ok(b) => b,
            Err(_) => return (None, 0),
        };
        let id = clavus_core::project::ProjectId::new(project_path);
        let store = SnapshotStore::new(blobs, &id);
        let head = store.head().map(|s| s.hash);
        let count = store.chain_length(None);
        (head, count)
    }
}

/// Whether the local store already has a blob.
fn store_has_blob(clavus_root: &std::path::Path, hash: &str) -> bool {
    clavus_core::blob::BlobStore::open(clavus_root.join("blobs")).has(hash)
}

/// File a blob in the local store.
fn store_put_blob(clavus_root: &std::path::Path, bytes: &[u8]) {
    let store = clavus_core::blob::BlobStore::open(clavus_root.join("blobs"));
    store.put(bytes);
}

/// Store snapshots and cues that arrived from a relay.
///
/// Returns `(snapshots stored, cues imported)`. Everything that arrives is
/// untrusted, so the store validates hashes and cue ids before writing.
pub fn store_pulled(
    snapshots: &[clavus_core::types::Snapshot],
    cues: &[clavus_core::types::Cue],
    clavus_root: &std::path::Path,
    project: &clavus_core::project::ProjectId,
) -> (usize, usize) {
    use clavus_core::blob::BlobStore;
    use clavus_core::cue::{CueStore, ImportOutcome};
    use clavus_core::snapshot::SnapshotStore;

    let mut stored = 0;
    let mut imported = 0;

    if !snapshots.is_empty() {
        if let Ok(blobs) = BlobStore::new(clavus_root.join("blobs")) {
            let snap_store = SnapshotStore::new(blobs, project);
            for snap in snapshots {
                if snap_store.has(&snap.hash) {
                    continue;
                }
                // The snapshot now lives here, so it records the local
                // path. A collaborator's machine has a different path
                // for the same project, and the restore has to work on
                // this one.
                let mut local = snap.clone();
                local.project_path = project.path.to_string_lossy().to_string();
                if snap_store.save_raw(local).is_ok() {
                    stored += 1;
                }
            }
            // Advance HEAD to the newest snapshot the relay sent. The
            // list is not necessarily in chain order, so the newest is
            // the one that is nobody's parent.
            if let Some(newest) = clavus_core::snapshot::newest_in(snapshots) {
                let _ = snap_store.set_head(&newest, "pull");
            }
        }
    }

    if !cues.is_empty() {
        if let Ok(store) = CueStore::new(clavus_root, project) {
            for cue in cues {
                if matches!(store.import(cue.clone()), Ok(ImportOutcome::Imported)) {
                    imported += 1;
                }
            }
        }
    }

    (stored, imported)
}
