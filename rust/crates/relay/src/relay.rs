//! Clavus relay server.
//!
//! Accepts connections from multiple clients, syncs snapshots and cues between them.

use clavus_core::types::{Cue, Snapshot};
use clavus_sync::client::SyncMsg;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// A connected client.
#[derive(Debug, Clone)]
pub struct Client {
    addr: SocketAddr,
    name: String,
    known_heads: Vec<String>,
    stream: Arc<Mutex<TcpStream>>,
    subscribed: Vec<String>,
}

impl Client {
    fn new(addr: SocketAddr, stream: TcpStream) -> Self {
        stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
        Self {
            addr,
            name: String::new(),
            known_heads: vec![],
            stream: Arc::new(Mutex::new(stream)),
            subscribed: vec![],
        }
    }

    fn send(&self, msg: &SyncMsg) -> std::io::Result<()> {
        let json = serde_json::to_vec(msg)
            .map_err(std::io::Error::other)?;
        let len = json.len() as u32;
        let mut stream = self.stream.lock().unwrap();
        stream.write_all(&len.to_be_bytes())?;
        stream.write_all(&json)?;
        stream.flush()
    }
}

/// Relay state, persisted to disk as JSON.
pub struct RelayState {
    /// All snapshots known to the relay, keyed by project and then hash.
    ///
    /// A hash alone is not enough: two projects can hold the same bytes,
    /// and storing one record per hash made the second project overwrite
    /// the first one's message, timestamp and path.
    pub snapshots: HashMap<String, HashMap<String, Snapshot>>,
    /// All cues known to the relay, keyed by project.
    pub cues: HashMap<String, Vec<Cue>>,
    /// Current HEAD hash per project (derived from project_path filename).
    /// Used for conflict detection.
    heads: HashMap<String, String>,
    /// Snapshot hashes per project, in the order they were pushed.
    ///
    /// A client working on one project must not be offered another
    /// project's history, so the relay needs to know which snapshots
    /// belong to which project.
    by_project: HashMap<String, Vec<String>>,
    /// Connected clients (not persisted).
    clients: Vec<Client>,
    /// Directory for persisted state.
    state_dir: std::path::PathBuf,
}

impl RelayState {
    /// Load from disk or create a fresh state.
    pub fn new(state_dir: std::path::PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&state_dir).ok();
        let snapshots = Self::load_snaps(&state_dir).unwrap_or_default();
        let cues = Self::load_cues(&state_dir).unwrap_or_default();
        let heads = Self::load_heads(&state_dir).unwrap_or_default();
        // The per-project index is rebuilt from the snapshots themselves,
        // so an older state file still works.
        //
        // The order is the *chain* order — oldest first — and not the
        // order of a hash map. A random order meant that after a relay
        // restart the same project came back in a different order every
        // time, and a pull installed an arbitrary snapshot as the head.
        let mut by_project: HashMap<String, Vec<String>> = HashMap::new();
        for (project, by_hash) in &snapshots {
            by_project.insert(project.clone(), chain_order(by_hash));
        }
        info!(
            "Relay state loaded: {} snapshots, {} cue lists, {} project heads",
            snapshots.len(),
            cues.len(),
            heads.len()
        );
        Ok(Self {
            snapshots,
            cues,
            heads,
            by_project,
            clients: Vec::new(),
            state_dir,
        })
    }

    fn snaps_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("snapshots.json")
    }

    fn cues_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("cues.json")
    }

    fn heads_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("heads.json")
    }

    fn load_heads(dir: &std::path::Path) -> std::io::Result<HashMap<String, String>> {
        let path = Self::heads_path(dir);
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = std::fs::read_to_string(&path)?;
        serde_json::from_str(&data)
            .map_err(|e| std::io::Error::other(format!("heads parse: {}", e)))
    }

    fn load_snaps(dir: &std::path::Path) -> std::io::Result<HashMap<String, HashMap<String, Snapshot>>> {
        let path = Self::snaps_path(dir);
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = std::fs::read_to_string(&path)?;
        let snaps: Vec<Snapshot> = serde_json::from_str(&data)
            .map_err(|e| std::io::Error::other(format!("snapshots parse: {}", e)))?;
        let mut out: HashMap<String, HashMap<String, Snapshot>> = HashMap::new();
        for snap in snaps {
            let project = project_name_from_path(&snap.project_path);
            out.entry(project)
                .or_default()
                .insert(snap.hash.clone(), snap);
        }
        Ok(out)
    }

    fn load_cues(dir: &std::path::Path) -> std::io::Result<HashMap<String, Vec<Cue>>> {
        let path = Self::cues_path(dir);
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = std::fs::read_to_string(&path)?;
        serde_json::from_str(&data)
            .map_err(|e| std::io::Error::other(format!("cues parse: {}", e)))
    }

    /// Persist snapshots, cues, and heads to disk (atomic: temp file + rename).
    fn persist(&self) -> std::io::Result<()> {
        let all_snaps: Vec<Snapshot> = self
            .snapshots
            .values()
            .flat_map(|by_hash| by_hash.values().cloned())
            .collect();
        Self::atomic_json_write(&Self::snaps_path(&self.state_dir), &all_snaps)?;
        Self::atomic_json_write(
            &Self::cues_path(&self.state_dir),
            &self.cues,
        )?;
        Self::atomic_json_write(
            &Self::heads_path(&self.state_dir),
            &self.heads,
        )?;
        debug!("Relay state persisted");
        Ok(())
    }

    fn atomic_json_write<T: serde::Serialize>(path: &std::path::Path, value: &T) -> std::io::Result<()> {
        let tmp = std::path::PathBuf::from(path.to_string_lossy().to_string() + ".tmp");
        let json = serde_json::to_string(value)
            .map_err(|e| std::io::Error::other(format!("json serialize: {}", e)))?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Register or update a client.
    pub fn upsert_client(&mut self, client: Client) {
        self.clients.retain(|c| c.addr != client.addr);
        self.clients.push(client);
    }

    /// Remove a client by address.
    pub fn remove_client(&mut self, addr: &SocketAddr) {
        self.clients.retain(|c| &c.addr != addr);
    }

    /// Every snapshot the relay holds, across all projects.
    pub fn all_snapshots(&self) -> Vec<Snapshot> {
        self.snapshots
            .values()
            .flat_map(|by_hash| by_hash.values().cloned())
            .collect()
    }

    /// One snapshot, by project and hash.
    pub fn snapshot(&self, project: &str, hash: &str) -> Option<&Snapshot> {
        self.snapshots.get(project)?.get(hash)
    }

    /// Find snapshots the client is missing, for one project only.
    ///
    /// `project` is the client's project name; snapshots belonging to
    /// other projects are never offered.
    pub fn find_missing(&self, project: &str, known: &[String]) -> Vec<Snapshot> {
        let Some(hashes) = self.by_project.get(project) else {
            return Vec::new();
        };
        let Some(by_hash) = self.snapshots.get(project) else {
            return Vec::new();
        };
        hashes
            .iter()
            .filter(|h| !known.contains(h))
            .filter_map(|h| by_hash.get(h).cloned())
            .collect()
    }

    /// Store incoming snapshots and cues. Detects divergent HEADs.
    /// Returns a list of conflicts: (project, relay_head, new_head).
    pub fn store(&mut self, snapshots: Vec<Snapshot>, cues: Vec<Cue>) -> Vec<(String, String, String)> {
        let mut conflicts = Vec::new();

        for snap in snapshots {
            // A snapshot hash becomes a file name on every client that
            // pulls it, so a malformed one is rejected here rather than
            // being served to the network.
            // A hash is a SHA-256 digest, so it is exactly 64 characters.
            // A shorter one could be stored here but could never be filed
            // by a client, so it is refused at the door.
            if snap.hash.len() != 64
                || !clavus_core::blob::BlobStore::is_valid_hash(&snap.hash)
            {
                warn!("Rejected snapshot with invalid hash: {:?}", snap.hash);
                continue;
            }
            if let Some(parent) = snap.parent.as_deref() {
                if parent.len() != 64
                    || !clavus_core::blob::BlobStore::is_valid_hash(parent)
                {
                    warn!("Rejected snapshot with invalid parent: {:?}", parent);
                    continue;
                }
            }

            // Extract fields before moving the snapshot
            let snap_hash = snap.hash.clone();
            let snap_parent = snap.parent.clone();
            let project_name = project_name_from_path(&snap.project_path);

            // Detect HEAD divergence per project
            if let Some(existing_head) = self.heads.get(&project_name) {
                let incoming_parent = snap_parent.as_deref();
                if incoming_parent != Some(existing_head.as_str())
                    && snap_hash.as_str() != existing_head.as_str()
                {
                    // Not a conflict when the relay's head is already part
                    // of the incoming snapshot's history — that is a client
                    // pushing history the relay already has. It was also
                    // not a conflict when the incoming snapshot is an
                    // *ancestor* of the relay's head, which is a client
                    // that simply has older history.
                    let relay_head_is_ancestor =
                        self.is_ancestor(existing_head, &snap_hash, &project_name);
                    let incoming_is_ancestor =
                        self.is_ancestor(&snap_hash, existing_head, &project_name);
                    if !relay_head_is_ancestor && !incoming_is_ancestor {
                        conflicts.push((project_name.clone(), existing_head.clone(), snap_hash.clone()));
                        debug!(
                            "Conflict: {} relay_head={} incoming_parent={} hash={}",
                            project_name, existing_head,
                            incoming_parent.unwrap_or("(none)"), snap_hash
                        );
                    }
                }
            }

            // Store the snapshot under its own project, so one project's
            // record can never overwrite another's.
            self.snapshots
                .entry(project_name.clone())
                .or_default()
                .insert(snap_hash.clone(), snap);

            // Remember which project it belongs to, so it is only ever
            // offered to a client working on that project.
            let project_hashes = self.by_project.entry(project_name.clone()).or_default();
            if !project_hashes.contains(&snap_hash) {
                project_hashes.push(snap_hash.clone());
            }

            // Update HEAD for this project to the latest snapshot
            self.heads.insert(project_name, snap_hash);
        }

        for cue in cues {
            // A cue id becomes a file name on every client that pulls it,
            // so a malformed one is rejected here rather than served to
            // the network.
            if clavus_core::cue::CueStore::validate_cue_id(&cue.id).is_err() {
                warn!("Rejected cue with invalid id: {:?}", cue.id);
                continue;
            }
            // Cues are grouped by project, not by track name — two
            // projects with a track called "Drums" must not share notes.
            let project = if cue.project.is_empty() {
                // An older client that does not send the project name.
                cue.track_name.clone()
            } else {
                cue.project.clone()
            };
            let entry = self.cues.entry(project).or_default();
            // Replacing by id keeps a repeated push from duplicating the
            // cue in every client's list.
            match entry.iter_mut().find(|c| c.id == cue.id) {
                Some(existing) => *existing = cue,
                None => entry.push(cue),
            }
        }

        // Persist after every store to ensure durability
        if let Err(e) = self.persist() {
            warn!("Relay persist failed: {}", e);
        }

        conflicts
    }

    /// Returns true if `ancestor` is an ancestor of `descendant` in the snapshot chain.
    fn is_ancestor(&self, ancestor: &str, descendant: &str, project: &str) -> bool {
        if ancestor == descendant {
            return true;
        }
        let Some(by_hash) = self.snapshots.get(project) else {
            return false;
        };
        let mut current = descendant.to_string();
        for _ in 0..100 {
            let snap = match by_hash.get(&current) {
                Some(s) => s,
                None => return false,
            };
            match &snap.parent {
                Some(parent) if parent != &current => {
                    if parent == ancestor {
                        return true;
                    }
                    current = parent.clone();
                }
                _ => return false,
            }
        }
        false // chain too deep or loop
    }
}

/// Run the relay server.
pub fn run(bind_addr: &str, state_dir: std::path::PathBuf) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind_addr)?;
    run_with_listener(listener, state_dir)
}

/// Run the relay server on an already-bound listener.
///
/// Tests use this to discover the bound port when binding to port 0.
pub fn run_with_listener(
    listener: TcpListener,
    state_dir: std::path::PathBuf,
) -> std::io::Result<()> {
    info!(
        "Relay listening on {}",
        listener.local_addr().map(|a| a.to_string()).unwrap_or_default()
    );

    let state = Arc::new(Mutex::new(match RelayState::new(state_dir) {
        Ok(s) => s,
        Err(e) => return Err(std::io::Error::other(format!("init relay state: {}", e))),
    }));

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let addr = match stream.peer_addr() {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("Failed to get peer addr: {}", e);
                        continue;
                    }
                };

                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    if let Err(e) = handle_client(addr, stream, &state) {
                        warn!("Client {} error: {}", addr, e);
                    }
                    state.lock().unwrap().remove_client(&addr);
                });
            }
            Err(e) => {
                error!("accept error: {}", e);
            }
        }
    }

    Ok(())
}

fn handle_client(addr: SocketAddr, stream: TcpStream, state: &Arc<Mutex<RelayState>>) -> std::io::Result<()> {
    info!("Client connected: {}", addr);
    let mut client = Client::new(addr, stream);

    loop {
        // Read 4-byte length prefix (refresh timeout before each read)
        let mut len_buf = [0u8; 4];
        {
            let mut s = client.stream.lock().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(30))).ok();
            s.read_exact(&mut len_buf)?;
        }
        let len = u32::from_be_bytes(len_buf) as usize;

        if len > 10 * 1024 * 1024 {
            warn!("Payload too large from {}: {} bytes", addr, len);
            break;
        }

        let mut data = vec![0u8; len];
        {
            let mut s = client.stream.lock().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(30))).ok();
            s.read_exact(&mut data)?;
        }

        let msg: SyncMsg = match serde_json::from_slice(&data) {
            Ok(m) => m,
            Err(e) => {
                warn!("Invalid message from {}: {}", addr, e);
                break;
            }
        };

        match msg {
            SyncMsg::Hello { client_name, known_heads } => {
                info!("Client {} (name={}) hello, {} known heads", addr, client_name, known_heads.len());
                client.name = client_name.clone();
                client.known_heads = known_heads.clone();

                let resp = {
                    let s = state.lock().unwrap();
                    let snaps = s.find_missing(&client_name, &known_heads);
                    // Only the client's own project's cues — sending every
                    // project's notes would mix them up.
                    let cues: Vec<Cue> = s.cues.get(&client_name).cloned().unwrap_or_default();
                    SyncMsg::Push { snapshots: snaps, cues }
                };

                state.lock().unwrap().upsert_client(client.clone());
                let _ = client.send(&resp);
            }
            SyncMsg::Pull { missing } => {
                debug!("Client {} requesting {} missing objects", addr, missing.len());
                let (snapshots, cues) = {
                    let s = state.lock().unwrap();
                    // Only the client's own project's snapshots are served.
                    let snapshots: Vec<Snapshot> = missing
                        .iter()
                        .filter_map(|h| {
                            s.snapshots
                                .get(&client.name)
                                .and_then(|by_hash| by_hash.get(h))
                                .cloned()
                        })
                        .collect();
                    let cues: Vec<Cue> = s.cues.get(&client.name).cloned().unwrap_or_default();
                    (snapshots, cues)
                };
                let _ = client.send(&SyncMsg::PullResp { snapshots, cues });
            }
            SyncMsg::Push { snapshots, cues } => {
                debug!("Client {} pushing {} snapshots, {} cues", addr, snapshots.len(), cues.len());
                let conflicts = state.lock().unwrap().store(snapshots, cues);
                // Send conflict notifications first (client can handle them before Synced)
                for (project, relay_head, push_head) in &conflicts {
                    warn!(
                        "HEAD divergence on project '{}': relay head {} vs incoming {}",
                        project, relay_head, push_head
                    );
                    let _ = client.send(&SyncMsg::Conflict {
                        project: project.clone(),
                        relay_head: relay_head.clone(),
                        push_head: push_head.clone(),
                    });
                }
                let _ = client.send(&SyncMsg::Synced);
            }
            SyncMsg::PullResp { snapshots, cues } => {
                debug!("Client {} pushing {} snapshots, {} cues", addr, snapshots.len(), cues.len());
                let conflicts = state.lock().unwrap().store(snapshots, cues);
                // Send conflict notifications first (client can handle them before Synced)
                for (project, relay_head, push_head) in &conflicts {
                    warn!(
                        "HEAD divergence on project '{}': relay head {} vs incoming {}",
                        project, relay_head, push_head
                    );
                    let _ = client.send(&SyncMsg::Conflict {
                        project: project.clone(),
                        relay_head: relay_head.clone(),
                        push_head: push_head.clone(),
                    });
                }
                let _ = client.send(&SyncMsg::Synced);
            }
            SyncMsg::Subscribe { project } => {
                client.subscribed.push(project);
            }
            _ => {}
        }
    }

    info!("Client disconnected: {}", addr);
    Ok(())
}

/// Order a project's snapshots oldest first, following the parent links.
///
/// The chain is rebuilt from the snapshots' own parent pointers, so the
/// order is the same after a restart as it was when the snapshots were
/// pushed. A hash map's order is random, and a random order made a
/// pull install an arbitrary snapshot as the head.
fn chain_order(by_hash: &HashMap<String, Snapshot>) -> Vec<String> {
    // The children of each snapshot.
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut roots: Vec<&str> = Vec::new();
    for (hash, snap) in by_hash {
        match snap.parent.as_deref() {
            Some(parent) if by_hash.contains_key(parent) => {
                children.entry(parent).or_default().push(hash.as_str());
            }
            _ => roots.push(hash.as_str()),
        }
    }
    for kids in children.values_mut() {
        kids.sort();
    }
    roots.sort();

    let mut ordered = Vec::with_capacity(by_hash.len());
    let mut stack: Vec<&str> = roots;
    while let Some(hash) = stack.pop() {
        ordered.push(hash.to_string());
        if let Some(kids) = children.get(hash) {
            // Push in reverse so the sorted order is kept.
            for kid in kids.iter().rev() {
                stack.push(kid);
            }
        }
    }

    // A snapshot that is part of a cycle would be left out; add anything
    // that is missing so no snapshot is ever silently dropped.
    if ordered.len() != by_hash.len() {
        for hash in by_hash.keys() {
            if !ordered.iter().any(|h| h == hash) {
                ordered.push(hash.clone());
            }
        }
    }
    ordered
}

/// Extract the sync name of a project from a full path.
///
/// E.g. "/Users/chris/Documents/Ableton/User Library/Project.als" → "Project.als"
///
/// The *file name with its extension* is the name a project is known by
/// on the wire, because two machines holding the same project keep it at
/// different paths but must still agree on it. The extension is part of
/// the name so that an Ableton set and a Bitwig bundle that happen to
/// share a stem stay apart.
fn project_name_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| path.to_string())
}
