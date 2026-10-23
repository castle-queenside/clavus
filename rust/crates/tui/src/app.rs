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

/// Keep a scroll offset and a selected row in step.
///
/// `offset` is the index of the first row drawn, `selected` is the
/// position of the cursor among the rows, and `len` is how many rows
/// are drawn at once. The offset moves so that the cursor is always on
/// a row that is on screen.
///
/// A list that does not fit in the terminal used to be drawn from the
/// top and clipped. The arrow keys still moved the cursor, so it
/// walked off the bottom of the screen and the user was left with a
/// selection they could not see and a list that never moved.
pub fn scroll_to_show(offset: &mut usize, selected: Option<usize>, len: usize, total: usize) {
    if len == 0 || total == 0 {
        *offset = 0;
        return;
    }
    let max_offset = total.saturating_sub(len);
    if *offset > max_offset {
        *offset = max_offset;
    }
    if let Some(sel) = selected {
        if sel < *offset {
            *offset = sel;
        } else if sel >= offset.saturating_add(len) {
            *offset = sel + 1 - len;
        }
    }
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
    /// Every directory the project list is drawn from.
    ///
    /// The CLI's `--dir` names one, and it is the only one. Without it,
    /// the standard places are scanned — `~/Music`, `~/Documents`, and
    /// `~/Clavus/Projects`, which is where Clavus itself used to keep
    /// its sets.
    pub project_dirs: Vec<PathBuf>,
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
    /// How the project list is ordered.
    pub sort: SortMode,
    /// The text the project list is narrowed by. Empty means everything.
    pub filter: String,
    /// True while the user is typing the filter.
    pub filtering: bool,
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
    /// The screen the sync screen was opened from.
    pub sync_opened_from: Option<Screen>,
    /// The first project row on screen.
    ///
    /// A library of a thousand sets does not fit in a terminal, so the
    /// list has to scroll. The renderer owns the number — it knows how
    /// many rows fit — and keeps this in step with the selection, so
    /// the row under the cursor is always one of the rows on screen.
    pub list_scroll: usize,
    /// The first cue row on screen.
    pub cue_scroll: usize,
    /// The first snapshot row on screen.
    pub snapshot_scroll: usize,
    /// The rows on the screen that mean something, and where they are.
    ///
    /// The renderer fills this in every frame and the mouse handler
    /// reads it. The layout is computed in one place, so the map can
    /// never disagree with what is on screen.
    pub hits: Vec<crate::mouse::HitRegion>,
    /// The last click, so that a second one can be recognised as a
    /// double click.
    pub last_click: Option<crate::mouse::Click>,
    /// What the background reader has found so far.
    summaries: Option<std::sync::mpsc::Receiver<(usize, Summary)>>,
    /// Kept so that the reader stops when the app is dropped.
    summary_sender: Option<std::sync::mpsc::Sender<()>>,
    /// When the store was last read for the list's counts.
    ///
    /// Re-reading the store for every project in a library of a
    /// thousand costs a walk of the chain for each one, and that is far
    /// too much to do on every keypress. The store's own clock decides
    /// when it is worth doing again.
    last_counts_refresh: Option<std::time::Instant>,
}

/// What the list knows about a project's tempo and track count.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Summary {
    /// Not read yet.
    ///
    /// Reading every project in a real library is slow — a studio's
    /// sets are gigabytes of gzipped XML — so it is done in the
    /// background, and the rows that have not been read yet say so
    /// rather than showing a number that is not known.
    #[default]
    Pending,
    /// Read: the tempo and the number of tracks.
    Known { bpm: f64, tracks: usize },
    /// The file could not be read.
    Unreadable,
}

#[derive(Debug)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    /// The folder the project lives in, relative to the home directory.
    ///
    /// Two projects can share a file name, and the user has to be able
    /// to tell them apart on screen — the list showed two rows both
    /// named "Song" with nothing to tell them apart.
    pub folder: String,
    pub project_type: String,
    pub head: Option<String>,
    /// Number of snapshots in the chain (0 if never snapshotted).
    pub snap_count: usize,
    /// The tempo and the number of tracks, once they have been read.
    pub summary: Summary,
    pub modified_secs: i64,
    /// Notes on this project that have not been dealt with.
    ///
    /// A note you left and never closed is the clearest sign that the
    /// project is still open. It is the one signal that comes from
    /// Clavus itself rather than from the file's timestamps.
    pub open_cues: usize,
    /// The shape of the project's history, one bar height per cell.
    pub activity: Vec<u8>,
}

impl ProjectEntry {
    /// The project's tempo, or zero if it has not been read.
    pub fn bpm(&self) -> f64 {
        match self.summary {
            Summary::Known { bpm, .. } => bpm,
            _ => 0.0,
        }
    }

    /// The project's number of tracks, or zero if it has not been read.
    pub fn track_count(&self) -> usize {
        match self.summary {
            Summary::Known { tracks, .. } => tracks,
            _ => 0,
        }
    }

    /// The project's identity.
    ///
    /// Two projects that share a file name are two different projects, so
    /// everything that is stored per project — the HEAD, the snapshot
    /// records, the cues — is keyed by this identity and not by the name.
    pub fn id(&self) -> clavus_core::project::ProjectId {
        clavus_core::project::ProjectId::new(&self.path)
    }

    /// Whether the project matches what the user typed to filter by.
    ///
    /// The name and the folder are both searched, so "drums" finds a
    /// set called *Drums* and a set called *Song* that happens to live
    /// in a folder called *Drums*.
    pub fn matches(&self, needle: &str) -> bool {
        let n = needle.to_lowercase();
        self.name.to_lowercase().contains(&n) || self.folder.to_lowercase().contains(&n)
    }
}

/// How the project list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    /// Most recently touched first. The default, because the project
    /// you were working on yesterday is the one you want to see.
    Recent,
    /// By name.
    Name,
    /// Most snapshots first — the projects with the most history.
    History,
    /// Most open notes first — the projects with the most unfinished
    /// work.
    Open,
}

impl SortMode {
    /// The word shown in the status strip.
    pub fn label(&self) -> &'static str {
        match self {
            SortMode::Recent => "recent",
            SortMode::Name => "name",
            SortMode::History => "history",
            SortMode::Open => "open notes",
        }
    }

    /// The next mode in the cycle.
    pub fn next(&self) -> SortMode {
        match self {
            SortMode::Recent => SortMode::Name,
            SortMode::Name => SortMode::History,
            SortMode::History => SortMode::Open,
            SortMode::Open => SortMode::Recent,
        }
    }
}

/// How many cells the history sparkline has.
pub const ACTIVITY_CELLS: usize = 8;


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
        Self::with_dirs(vec![projects_dir])
    }

    /// An app that draws its project list from several directories.
    pub fn with_dirs(project_dirs: Vec<PathBuf>) -> Self {
        let projects_dir = project_dirs
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));
        let clavus_root = dirs::home_dir()
            .map(|h| h.join(".clavus"))
            .unwrap_or_else(|| PathBuf::from(".clavus"));
        Self {
            clavus_root: clavus_root.clone(),
            projects_dir,
            project_dirs,
            projects: vec![],
            selected_project: None,
            project_detail: None,
            cues: vec![],
            selected_cue: None,
            snapshots: vec![],
            selected_snapshot: None,
            cue_filter: None,
            screen: Screen::ProjectList,
            sort: SortMode::Recent,
            filter: String::new(),
            filtering: false,
            sync_state: SyncState::default(),
            error: None,
            info: None,
            input_buf: String::new(),
            loading: false,
            idle_index: 0,
            cue_message: String::new(),
            cue_step: CueStep::Message,
            relay_addr: "127.0.0.1:4477".to_string(),
            sync_opened_from: None,
            list_scroll: 0,
            cue_scroll: 0,
            snapshot_scroll: 0,
            hits: Vec::new(),
            last_click: None,
            summaries: None,
            summary_sender: None,
            last_counts_refresh: None,
        }
    }

    /// The screen to return to when the user backs out of this one.
    ///
    /// Esc used to jump straight to the project list from everywhere,
    /// so a user in the cue list could not step back to the project
    /// they were working on.
    pub fn previous_screen(&self) -> Screen {
        match self.screen {
            Screen::CueAdd => Screen::CueList,
            Screen::RelayInput => Screen::Sync,
            // The sync screen was opened from wherever the user was, so
            // Esc goes back there. It used to land on the project detail
            // screen even when the user came from the project list —
            // and that screen says "no project loaded", so the user
            // had to press Esc twice to get back.
            Screen::Sync => self.sync_opened_from.unwrap_or(Screen::ProjectDetail),
            Screen::CueList | Screen::SnapshotList => Screen::ProjectDetail,
            Screen::ProjectDetail => Screen::ProjectList,
            Screen::ProjectList => Screen::ProjectList,
        }
    }

    /// Open the sync screen, remembering where the user came from.
    pub fn open_sync(&mut self) {
        self.sync_opened_from = Some(match self.screen {
            Screen::ProjectList => Screen::ProjectList,
            Screen::ProjectDetail => Screen::ProjectDetail,
            other => other,
        });
        self.screen = Screen::Sync;
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
        for dir in &self.project_dirs {
            std::fs::read_dir(dir).map_err(|e| format!("Cannot read {}: {}", dir.display(), e))?;
        }

        // A Clavus data directory that cannot be read is an error too.
        // The list would show every project with no snapshots, and the
        // user would think their history was gone.
        self.store_readable()?;

        // The CLI and the TUI find projects the same way, so they show
        // the same drive. The walk is cheap on purpose: a real library
        // has tens of thousands of folders, and resolving each of them
        // to its real path costs a path lookup apiece.
        let found = clavus_core::project::discover_projects(&self.project_dirs, true);

        let mut entries = Vec::new();
        for project in found {
            let ext = project
                .path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if let Some(entry) = self.project_entry(&project.path, &ext) {
                entries.push(entry);
            }
        }

        // Sort by modified time descending
        entries.sort_by_key(|a| std::cmp::Reverse(a.modified_secs));
        self.projects = entries;
        self.start_summary_reader();
        Ok(())
    }

    /// Read every project's tempo and track count in the background.
    ///
    /// A studio's library is gigabytes of gzipped XML, and reading it
    /// all before the first screen appears took over a minute on a real
    /// drive. The list does not need all of it at once: it needs the
    /// rows it is showing. So the reading happens on another thread and
    /// the rows fill in as the answers arrive.
    ///
    /// The list is ordered by how recently the project was edited, so
    /// the sets that are read first are the ones the user was last
    /// working on.
    fn start_summary_reader(&mut self) {
        // A reader from an earlier scan is stopped first.
        self.summary_sender = None;
        self.summaries = None;

        let jobs: Vec<(usize, PathBuf, String)> = self
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let ext = p
                    .path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                (i, p.path.clone(), ext)
            })
            .collect();

        if jobs.is_empty() {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel::<(usize, Summary)>();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();

        let handle = std::thread::Builder::new()
            .name("clavus-summaries".into())
            .spawn(move || {
                for (index, path, ext) in jobs {
                    // The app has gone, or the list was scanned again:
                    // there is nobody left to tell.
                    match stop_rx.try_recv() {
                        Ok(()) => return,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    }
                    let summary = match App::parse_summary(&path, &ext) {
                        Some((bpm, tracks)) => Summary::Known { bpm, tracks },
                        None => Summary::Unreadable,
                    };
                    if tx.send((index, summary)).is_err() {
                        return;
                    }
                }
            });

        if handle.is_err() {
            // No thread, no summaries — the list says "not read yet"
            // and carries on.
            return;
        }
        self.summaries = Some(rx);
        self.summary_sender = Some(stop_tx);
    }

    /// Pick up whatever the background reader has found.
    ///
    /// Returns true when something changed, so the caller knows to draw
    /// again.
    pub fn poll_summaries(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.summaries {
            while let Ok((index, summary)) = rx.try_recv() {
                if let Some(entry) = self.projects.get_mut(index) {
                    if entry.summary != summary {
                        entry.summary = summary;
                        changed = true;
                    }
                }
            }
        }
        if self.read_selected_summary() {
            changed = true;
        }
        changed
    }

    /// Read the row under the cursor, if it has not been read yet.
    ///
    /// The background reader works through the whole library, and a
    /// library of a thousand sets is gigabytes of project files. The
    /// row the user is looking at is the one they are waiting for, so
    /// it is read straight away rather than in the background's time.
    fn read_selected_summary(&mut self) -> bool {
        let Some(index) = self.selected_project else {
            return false;
        };
        let Some(entry) = self.projects.get(index) else {
            return false;
        };
        if entry.summary != Summary::Pending {
            return false;
        }
        let path = entry.path.clone();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let summary = match App::parse_summary(&path, &ext) {
            Some((bpm, tracks)) => Summary::Known { bpm, tracks },
            None => Summary::Unreadable,
        };
        if let Some(entry) = self.projects.get_mut(index) {
            entry.summary = summary;
        }
        true
    }

    /// How often the list's counts are re-read from the store.
    ///
    /// The list has to agree with the CLI about the same project, and
    /// the counts change when the user works in the CLI while the TUI
    /// is running. Re-reading the store for every project in a large
    /// library takes long enough to be felt on every keypress, so it
    /// is done at most this often.
    pub const COUNTS_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

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

        // The folder the project lives in, so two projects that share
        // a file name can be told apart on screen.
        let folder = clavus_core::project::ProjectId::new(path)
            .folder_path()
            .to_string();

        // Check for existing snapshots
        let (head, snap_count, activity) = Self::snap_info(&self.clavus_root, path);

        // The notes that have not been dealt with. A project with open
        // notes is one that is still in progress, and that is a signal
        // that comes from Clavus rather than from the file's clock.
        let open_cues = Self::open_cue_count(&self.clavus_root, path);

        Some(ProjectEntry {
            path: path.to_path_buf(),
            name,
            folder,
            project_type: project_type.to_string(),
            head,
            snap_count,
            summary: Summary::Pending,
            modified_secs,
            open_cues,
            activity,
        })
    }

    /// The project's tempo and track count, read from the project file.
    /// Read a project's tempo and track count.
    pub fn parse_summary(path: &std::path::Path, ext: &str) -> Option<(f64, usize)> {
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

    // ─── The view of the project list ─────────────────────────────────
    //
    // The list holds every project Clavus found. What the user sees
    // is a *view* of that: narrowed by the filter they typed, ordered
    // by the sort they chose. The selection is an index into the
    // whole list, not into the view, so a project keeps its place
    // when the order changes underneath it.

    /// The projects the list shows, as indices into `projects`, in the
    /// order they are shown.
    pub fn visible_indices(&self) -> Vec<usize> {
        let needle = self.filter.trim().to_lowercase();
        let mut idx: Vec<usize> = if needle.is_empty() {
            (0..self.projects.len()).collect()
        } else {
            self.projects
                .iter()
                .enumerate()
                .filter(|(_, p)| p.matches(&needle))
                .map(|(i, _)| i)
                .collect()
        };
        self.sort_indices(&mut idx);
        idx
    }

    /// Order the visible projects by the current sort.
    fn sort_indices(&self, idx: &mut [usize]) {
        match self.sort {
            // The list is already held most-recent-first, so this is
            // the order the user has always seen.
            SortMode::Recent => {
                idx.sort_by_key(|&i| std::cmp::Reverse(self.projects[i].modified_secs));
            }
            SortMode::Name => {
                idx.sort_by(|&a, &b| {
                    self.projects[a]
                        .name
                        .to_lowercase()
                        .cmp(&self.projects[b].name.to_lowercase())
                });
            }
            SortMode::History => {
                idx.sort_by(|&a, &b| {
                    self.projects[b]
                        .snap_count
                        .cmp(&self.projects[a].snap_count)
                        .then_with(|| {
                            self.projects[b]
                                .modified_secs
                                .cmp(&self.projects[a].modified_secs)
                        })
                });
            }
            SortMode::Open => {
                idx.sort_by(|&a, &b| {
                    self.projects[b]
                        .open_cues
                        .cmp(&self.projects[a].open_cues)
                        .then_with(|| {
                            self.projects[b]
                                .modified_secs
                                .cmp(&self.projects[a].modified_secs)
                        })
                });
            }
        }
    }

    /// Move the selection by `delta` rows, over the projects on show.
    ///
    /// Moving past the end stops at the end. The selection is kept on
    /// the same *project* even when the filter changes, so narrowing
    /// the list does not silently select a different song.
    pub fn move_selection(&mut self, delta: i32) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            self.selected_project = None;
            return;
        }
        let current = self
            .selected_project
            .and_then(|sel| visible.iter().position(|&i| i == sel));
        let next = match current {
            None => 0,
            Some(pos) => {
                let n = visible.len() as i32;
                (pos as i32 + delta).clamp(0, n - 1) as usize
            }
        };
        self.selected_project = Some(visible[next]);
    }

    /// Move the selection to the first project on show.
    pub fn select_first_visible(&mut self) {
        self.move_selection(0);
    }

    /// Cycle the sort order.
    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        // The selection stays on the same project; the order around it
        // changes, which is the point of sorting.
    }

    /// Narrow the list by what the user typed.
    pub fn set_filter(&mut self, text: String) {
        self.filter = text;
    }

    /// Clear the filter and show everything again.
    pub fn clear_filter(&mut self) {
        self.filter.clear();
    }

    /// Make sure the selection is on a project the list is showing.
    ///
    /// Narrowing the list can hide the project that was selected. The
    /// selection then moves to the first project still on show, so the
    /// user is never left with a cursor on something they cannot see.
    pub fn ensure_selection_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(sel) = self.selected_project {
            if visible.contains(&sel) {
                return;
            }
        }
        self.selected_project = Some(visible[0]);
    }

    /// The cues on show, in the order they are drawn, with each cue's
    /// index in the whole list.
    ///
    /// The cue filter changes which cues are drawn, so a row on screen
    /// is not the same number as a row in the list. The arrow keys used
    /// to move through the unfiltered list while the highlight was
    /// computed on the filtered one, so the cursor could land on a cue
    /// that was not there.
    pub fn visible_cue_indices(&self) -> Vec<usize> {
        let filter = self.cue_filter.clone();
        self.cues
            .iter()
            .enumerate()
            .filter(|(_, c)| match &filter {
                None => true,
                Some(f) => c.status == *f,
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Move the selection by `delta` rows, over the cues on show.
    pub fn move_cue_selection(&mut self, delta: i32) {
        let visible = self.visible_cue_indices();
        if visible.is_empty() {
            self.selected_cue = None;
            return;
        }
        let current = self
            .selected_cue
            .and_then(|sel| visible.iter().position(|&i| i == sel));
        let next = match current {
            None => 0,
            Some(pos) => {
                let n = visible.len() as i32;
                (pos as i32 + delta).clamp(0, n - 1) as usize
            }
        };
        self.selected_cue = Some(visible[next]);
    }

    /// Move the selection by `delta` rows, over the snapshots.
    pub fn move_snapshot_selection(&mut self, delta: i32) {
        if self.snapshots.is_empty() {
            self.selected_snapshot = None;
            return;
        }
        let current = self.selected_snapshot;
        let next = match current {
            None => 0,
            Some(pos) => {
                let n = self.snapshots.len() as i32;
                (pos as i32 + delta).clamp(0, n - 1) as usize
            }
        };
        self.selected_snapshot = Some(next);
    }

    /// Move the selection up one row, on whichever list is on screen.
    pub fn scroll_up(&mut self) {
        match self.screen {
            Screen::ProjectList => self.move_selection(-1),
            Screen::CueList => self.move_cue_selection(-1),
            Screen::SnapshotList => self.move_snapshot_selection(-1),
            _ => {}
        }
    }

    /// Move the selection down one row, on whichever list is on screen.
    pub fn scroll_down(&mut self) {
        match self.screen {
            Screen::ProjectList => self.move_selection(1),
            Screen::CueList => self.move_cue_selection(1),
            Screen::SnapshotList => self.move_snapshot_selection(1),
            _ => {}
        }
    }

    /// The region of the screen under a point, if there is one.
    pub fn hit_at(&self, x: u16, y: u16) -> Option<crate::mouse::HitRegion> {
        self.hits.iter().copied().find(|r| r.contains(x, y))
    }

    /// Move the window of the list on screen.
    ///
    /// The selection goes with it: a scrollbar that moved the window
    /// but left the cursor behind would put the cursor on a row that
    /// is no longer on the screen.
    pub fn scroll_to(&mut self, offset: usize) {
        match self.screen {
            Screen::ProjectList => {
                let visible = self.visible_indices();
                if let Some(&i) = visible.get(offset) {
                    self.selected_project = Some(i);
                }
                self.list_scroll = offset;
            }
            Screen::CueList => {
                let visible = self.visible_cue_indices();
                if let Some(&i) = visible.get(offset) {
                    self.selected_cue = Some(i);
                }
                self.cue_scroll = offset;
            }
            Screen::SnapshotList => {
                if offset < self.snapshots.len() {
                    self.selected_snapshot = Some(offset);
                }
                self.snapshot_scroll = offset;
            }
            _ => {}
        }
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

    /// The path of the folder the selected project lives in.
    ///
    /// It is the second half of the project's identity on the wire: two
    /// projects can share a file name, and the folder tells them apart.
    pub fn selected_project_folder(&self) -> Option<String> {
        self.selected_entry()
            .map(|e| e.id().folder_path().to_string())
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

            // The command channel is waited on with a timeout, so the
            // thread notices a relay that has gone away. A relay that
            // died used to leave the sync screen saying "connected",
            // and the user believed their work was synced when it was
            // not.
            loop {
                let cmd = match cmd_rx.recv_timeout(std::time::Duration::from_secs(2)) {
                    Ok(cmd) => cmd,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // The client notices a dead socket when its
                        // reader hits the end of the stream.
                        if let Some(c) = client.as_ref() {
                            if c.is_closed() {
                                let _ = result_tx.send("connection lost".to_string());
                                break;
                            }
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
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
                                project_id.folder_path(),
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
            } else if msg == "connection lost" {
                // The relay went away. The screen must not keep
                // saying "connected" — the user would believe their
                // work was synced when it was not.
                self.sync_state.connected = false;
                self.sync_state.cmd_tx = None;
                self.set_error("the connection to the relay was lost".into());
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
    fn snap_info(
        clavus_root: &std::path::Path,
        project_path: &std::path::Path,
    ) -> (Option<String>, usize, Vec<u8>) {
        // The store is opened, not created: counting snapshots must
        // never bring a Clavus directory into being, and it is read
        // once per project, so it must not do any work either.
        let blobs = BlobStore::open(clavus_root.join("blobs"));
        let id = clavus_core::project::ProjectId::new(project_path);
        let store = SnapshotStore::new(blobs, &id);
        let head = store.head().map(|s| s.hash);
        let chain = store.walk_chain(None);
        // The shape of the history comes from the same walk that
        // counts the snapshots, so it costs nothing extra.
        let stamps: Vec<f64> = chain.iter().map(|s| s.timestamp).collect();
        let activity = crate::spark::sparkline(&stamps, ACTIVITY_CELLS);
        (head, chain.len(), activity)
    }

    /// The notes on a project that have not been dealt with.
    ///
    /// "Open" means pending or deferred — a note that was skipped or
    /// archived was closed on purpose, and one that was resolved was
    /// finished. The store is opened read-only: counting notes must
    /// never bring a Clavus directory into being.
    fn open_cue_count(clavus_root: &std::path::Path, project_path: &std::path::Path) -> usize {
        let id = clavus_core::project::ProjectId::new(project_path);
        let store = clavus_core::CueStore::open(clavus_root, &id);
        let Ok((cues, _broken)) = store.list_partial(None) else {
            return 0;
        };
        cues
            .iter()
            .filter(|c| {
                matches!(
                    c.status,
                    clavus_core::types::CueStatus::Pending | clavus_core::types::CueStatus::Deferred
                )
            })
            .count()
    }

    /// Refresh the snapshot counts and heads of the projects already in
    /// the list.
    ///
    /// The list is not rebuilt — the projects on disk have not changed
    /// while the TUI was open, only the history behind them has. A
    /// snapshot taken elsewhere, in the CLI, was invisible in the TUI
    /// until it was restarted, so the TUI and the CLI disagreed about
    /// the same project.
    pub fn refresh_project_counts(&mut self) {
        let root = self.clavus_root.clone();
        for entry in &mut self.projects {
            let (head, count, activity) = Self::snap_info(&root, &entry.path);
            entry.head = head;
            entry.snap_count = count;
            entry.activity = activity;
            entry.open_cues = Self::open_cue_count(&root, &entry.path);
        }
    }

    /// Re-read the counts, unless they were read very recently.
    ///
    /// Re-reading the store for every project in a library of a
    /// thousand walks every chain in it, and that is far too much to do
    /// on every keypress — the list was unusable on a large library,
    /// because moving the cursor took a tenth of a second.
    ///
    /// The only thing that can have changed is the store, so the
    /// store's own clock decides when it is worth looking again. A
    /// caller that knows the store has just changed should call
    /// `refresh_project_counts` instead, or `invalidate_counts` first.
    pub fn refresh_project_counts_if_stale(&mut self) {
        let now = std::time::Instant::now();
        if let Some(last) = self.last_counts_refresh {
            if now.duration_since(last) < Self::COUNTS_REFRESH_INTERVAL {
                return;
            }
        }
        self.last_counts_refresh = Some(now);
        self.refresh_project_counts();
    }

    /// Forget when the counts were last read, so that the next refresh
    /// happens straight away.
    ///
    /// Clavus changes the store itself when it takes a snapshot or
    /// pulls from a relay, and the list has to show that at once.
    pub fn invalidate_counts(&mut self) {
        self.last_counts_refresh = None;
    }

    /// Whether the Clavus data directory can be read.
    ///
    /// A store that cannot be read is not an empty store: the user
    /// would be told their project has no snapshots when it has some.
    pub fn store_readable(&self) -> Result<(), String> {
        let blobs = BlobStore::open(self.clavus_root.join("blobs"));
        blobs
            .check_readable()
            .map_err(|e| format!("cannot read the Clavus data directory: {}", e))
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
            let local_head = snap_store.head().map(|h| h.hash);
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
            // Advance HEAD only when the pulled history *extends* ours.
            // Moving the HEAD onto a different branch drops the user's
            // own snapshot out of the chain — the TUI used to do that,
            // and the CLI did not, so the same pull gave two different
            // results depending on which tool ran it.
            let candidates: Vec<String> = snapshots
                .iter()
                .map(|s| s.hash.clone())
                .filter(|h| snap_store.has(h))
                .collect();
            if let Some(newest) =
                clavus_core::snapshot::head_after_pull(&snap_store, local_head.as_deref(), &candidates)
            {
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
