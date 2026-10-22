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
    /// The name of the folder the project lives in.
    folder: String,
    /// The hash of the client's project file.
    content_hash: Option<String>,
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
            folder: String::new(),
            content_hash: None,
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
    /// The project bytes, in the middle of being received.
    ///
    /// A blob arrives in chunks, so the pieces are collected here until
    /// the last one turns up.
    incoming: HashMap<String, clavus_sync::blob_transfer::BlobAssembler>,
}

impl RelayState {
    /// Load from disk or create a fresh state.
    pub fn new(state_dir: std::path::PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&state_dir).ok();
        let snapshots = Self::load_snaps(&state_dir).unwrap_or_default();
        let cues = Self::load_cues(&state_dir).unwrap_or_default();
        let heads = Self::load_heads(&state_dir).unwrap_or_default();

        // A relay that was written before projects were told apart by
        // their folder files its heads and notes under the bare file
        // name. They are moved to the project's own key, so an
        // existing relay keeps its history after the upgrade.
        //
        // When two projects share the file name, the old record cannot
        // be placed safely: it might belong to either. It is left
        // where it is rather than handed to the wrong project.
        let heads = Self::migrate_heads(heads, &snapshots);
        let cues = Self::migrate_cues(cues, &snapshots);
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
            incoming: HashMap::new(),
        })
    }


    /// Move the heads of an older relay under the project's own key.
    fn migrate_heads(
        heads: HashMap<String, String>,
        snapshots: &HashMap<String, HashMap<String, Snapshot>>,
    ) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for (project, hash) in heads {
            // The snapshot with this hash knows which project it
            // belongs to.
            let key = snapshots
                .iter()
                .find(|(_, by_hash)| by_hash.contains_key(&hash))
                .map(|(key, _)| key.clone())
                .unwrap_or(project);
            out.insert(key, hash);
        }
        out
    }

    /// Move the notes of an older relay under the project's own key.
    fn migrate_cues(
        cues: HashMap<String, Vec<Cue>>,
        snapshots: &HashMap<String, HashMap<String, Snapshot>>,
    ) -> HashMap<String, Vec<Cue>> {
        let mut out: HashMap<String, Vec<Cue>> = HashMap::new();
        for (project, list) in cues {
            let matching: Vec<&String> = snapshots
                .keys()
                .filter(|k| wire_name_of(k) == project)
                .collect();
            match matching.as_slice() {
                // One project with that name — the notes belong to it.
                [only] => {
                    out.entry((*only).clone()).or_default().extend(list);
                }
                // Several projects share the name. The notes cannot be
                // placed safely, so they are left under the bare name
                // and are not served to either project.
                _ => {
                    warn!(
                        "The relay holds {} projects named '{}' — their notes cannot be told apart and are not served",
                        matching.len(),
                        project
                    );
                    out.entry(project).or_default().extend(list);
                }
            }
        }
        out
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
            let project = relay_key_of(&snap);
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

    /// The head of a project: the newest snapshot the relay holds.
    pub fn head_of(&self, project: &str) -> Option<&String> {
        self.heads.get(project)
    }

    /// The projects the relay knows under a wire name.
    ///
    /// A wire name is a file name. Two projects can share one, and
    /// they are kept apart by their folder.
    pub fn lineages_of(&self, wire_name: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .snapshots
            .keys()
            .chain(self.cues.keys())
            .filter(|k| wire_name_of(k) == wire_name)
            .cloned()
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// The project the client means.
    ///
    /// The client says which file it is working on and which folder it
    /// lives in. The relay prefers the project in that folder, then a
    /// project the client already has history from, and then the only
    /// project with that file name. When the relay holds several
    /// projects with that name and none of them can be identified, it
    /// serves none of them — handing over the wrong project's history
    /// is worse than handing over nothing.
    pub fn resolve_lineage(
        &self,
        wire_name: &str,
        folder: &str,
        known: &[String],
        content_hash: Option<&str>,
    ) -> Option<String> {
        let lineages = self.lineages_of(wire_name);
        if lineages.is_empty() {
            return None;
        }

        // A project the client already has history from.
        for lineage in &lineages {
            let has_history = self
                .snapshots
                .get(lineage)
                .map(|by_hash| by_hash.keys().any(|h| known.contains(h)))
                .unwrap_or(false);
            if has_history {
                return Some(lineage.clone());
            }
        }

        // A project whose snapshots include the client's file. The hash
        // of the file is the hash of a snapshot of it, so this finds the
        // project even when the file sits in a different folder on
        // another machine — which is the normal case for a
        // collaborator.
        // The project in the client's own folder.
        let wanted = clavus_core::project::relay_key_for(wire_name, folder);
        if lineages.contains(&wanted) {
            return Some(wanted);
        }

        if let Some(hash) = content_hash {
            // The hash of the file points at a project only when it
            // points at exactly one. Two projects can hold the same
            // bytes — a copy of a set, a template — and then the hash
            // says nothing.
            let matching: Vec<&String> = lineages
                .iter()
                .filter(|lineage| {
                    self.snapshots
                        .get(*lineage)
                        .map(|by_hash| by_hash.contains_key(hash))
                        .unwrap_or(false)
                })
                .collect();
            match matching.as_slice() {
                [only] => return Some((*only).clone()),
                [] => {}
                _ => {
                    warn!(
                        "The relay holds {} projects with the same bytes — the file's hash tells them apart not at all",
                        matching.len()
                    );
                }
            }
        }

        // The only project with that name.
        if lineages.len() == 1 {
            return Some(lineages[0].clone());
        }

        // Several projects share the name and none of them can be
        // told apart. Serving nothing is the safe answer.
        None
    }

    /// The project a snapshot belongs to.
    ///
    /// A snapshot that continues a chain the relay already holds
    /// belongs to that chain's project, whatever folder it was taken
    /// in. A project is a history, not a file name.
    ///
    /// The folder the snapshot was taken in wins over the chain, so
    /// two projects that hold the same bytes — a copy of a set, a
    /// template — stay two projects.
    pub fn lineage_of_snapshot(&self, snap: &Snapshot) -> Option<String> {
        let path_key = relay_key_of(snap);
        // The project in that folder is already known.
        if self.snapshots.contains_key(&path_key) {
            return Some(path_key);
        }
        // Its parent is known: the snapshot continues that history.
        if let Some(parent) = snap.parent.as_deref() {
            for (lineage, by_hash) in &self.snapshots {
                if by_hash.contains_key(parent) {
                    return Some(lineage.clone());
                }
            }
        }
        None
    }

    /// Find snapshots the client is missing, for one project only.
    ///
    /// `project` is the client's project key; snapshots belonging to
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

    /// The directory the project bytes live in.
    fn blob_dir(&self) -> std::path::PathBuf {
        self.state_dir.join("blobs")
    }

    /// Take one chunk of a project's bytes.
    ///
    /// The bytes are only written once every chunk has arrived *and*
    /// the whole blob hashes to the name it was sent under. A peer
    /// therefore cannot make the relay file a blob under someone else's
    /// name.
    pub fn store_blob_chunk(
        &mut self,
        chunk: &clavus_sync::blob_transfer::BlobChunk,
    ) -> Result<bool, String> {
        if !clavus_core::blob::BlobStore::is_valid_hash(&chunk.hash) {
            return Err(format!("'{}' is not a valid hash", chunk.hash));
        }
        let key = format!("{}::{}", chunk.project, chunk.hash);
        let assembler = self.incoming.entry(key.clone()).or_default();
        match assembler.add(chunk) {
            Ok(Some(whole)) => {
                clavus_sync::blob_transfer::verify_blob(&chunk.hash, &whole)?;
                self.write_blob(&chunk.hash, &whole)?;
                self.incoming.remove(&key);
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(e) => {
                self.incoming.remove(&key);
                Err(e)
            }
        }
    }

    /// Write a blob to the relay's store.
    fn write_blob(&self, hash: &str, data: &[u8]) -> Result<(), String> {
        let dir = self.blob_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(hash);
        // The hash is checked before it is used as a file name, so it
        // cannot escape the blob directory.
        let tmp = dir.join(format!("{}.tmp", hash));
        std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Read a blob from the relay's store.
    pub fn read_blob(&self, hash: &str) -> Option<Vec<u8>> {
        if !clavus_core::blob::BlobStore::is_valid_hash(hash) {
            return None;
        }
        std::fs::read(self.blob_dir().join(hash)).ok()
    }

    /// Whether the relay has the bytes of a snapshot.
    pub fn has_blob(&self, hash: &str) -> bool {
        self.blob_dir().join(hash).exists()
    }

    /// Store incoming snapshots and cues. Detects divergent HEADs.
    /// Returns a list of conflicts: (project, relay_head, new_head).
    pub fn store(&mut self, snapshots: Vec<Snapshot>, cues: Vec<Cue>) -> Vec<(String, String, String)> {
        self.store_for_client(snapshots, cues, None)
    }

    /// Store incoming snapshots and cues, filing the cues under the
    /// client's project.
    ///
    /// `client` is the project the client introduced itself with: the
    /// wire name and the folder it lives in. The notes of a project
    /// have to sit beside its snapshots, or a client would pull one
    /// project's notes and another's history.
    pub fn store_for_client(
        &mut self,
        snapshots: Vec<Snapshot>,
        cues: Vec<Cue>,
        client: Option<(String, String)>,
    ) -> Vec<(String, String, String)> {
        let mut conflicts = Vec::new();
        // The keys of the projects this push carried snapshots for.
        let mut pushed_keys: Vec<String> = Vec::new();

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
            // A snapshot that continues a chain the relay already holds
            // belongs to that chain's project, whatever folder it was
            // taken in. A project is a history, not a file name.
            let project_name = self
                .lineage_of_snapshot(&snap)
                .unwrap_or_else(|| relay_key_of(&snap));

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
                        // A push that is refused must not be stored, and
                        // it must not move the project's head. The client
                        // is told to pull and merge first, and it does
                        // that — a stored snapshot would have been a
                        // silent overwrite of the relay's history.
                        continue;
                    }
                }
            }

            // Store the snapshot under its own project, so one project's
            // record can never overwrite another's.
            if !pushed_keys.contains(&project_name) {
                pushed_keys.push(project_name.clone());
            }
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

            // The head only ever moves *forward*. A client that pushes
            // older history — a collaborator whose own HEAD is behind —
            // must not drag the relay's head back to an older snapshot:
            // the relay's record of the project's tip would go
            // backwards, and the next client to pull would be offered
            // the older history as if it were the newest.
            let move_forward = match self.heads.get(&project_name) {
                None => true,
                Some(current) => {
                    if current == &snap_hash {
                        false
                    } else if self.is_ancestor(current, &snap_hash, &project_name) {
                        // The relay's head is an ancestor of the
                        // incoming snapshot: the incoming one is newer.
                        true
                    } else {
                        // The incoming snapshot is older (or unrelated,
                        // which the conflict check above has already
                        // refused). The head stays where it is.
                        false
                    }
                }
            };
            if move_forward {
                self.heads.insert(project_name, snap_hash);
            }
        }

        // The key the client's notes belong under.
        //
        // The snapshots in this push say which project it is. If the
        // push carried no snapshots, the relay's own record of the
        // project says so.
        let cue_key: Option<String> = client.as_ref().and_then(|(name, folder)| {
            pushed_keys
                .iter()
                .find(|k| wire_name_of(k) == name.as_str())
                .cloned()
                .or_else(|| self.resolve_lineage(name, folder, &[], None))
                .or_else(|| Some(name.clone()))
        });

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
            } else if client
                .as_ref()
                .map(|(name, _)| cue.project == *name)
                .unwrap_or(false)
            {
                // The cue belongs to the project the client is working
                // on. It is filed under that project's key, beside its
                // snapshots.
                cue_key.clone().unwrap_or_else(|| cue.project.clone())
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

        // Notes that were filed under the bare wire name — by a push
        // that carried no snapshots — belong to the project the
        // snapshots name. They move there, so the notes of a project
        // are never split in two.
        if let (Some(key), Some((name, _))) = (cue_key.clone(), client.as_ref()) {
            if key != *name {
                if let Some(moved) = self.cues.remove(name) {
                    let entry = self.cues.entry(key.clone()).or_default();
                    for cue in moved {
                        match entry.iter_mut().find(|c| c.id == cue.id) {
                            Some(existing) => *existing = cue,
                            None => entry.push(cue),
                        }
                    }
                }
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
            SyncMsg::Hello {
                client_name,
                known_heads,
                folder,
                content_hash,
            } => {
                info!("Client {} (name={}) hello, {} known heads", addr, client_name, known_heads.len());
                client.name = client_name.clone();
                client.known_heads = known_heads.clone();
                client.folder = folder.clone();
                client.content_hash = content_hash.clone();

                let resp = {
                    let s = state.lock().unwrap();
                    // Which project does the client mean? Two projects
                    // can share a file name, and they must not be mixed
                    // up.
                    let lineage = s.resolve_lineage(
                        &client_name,
                        &folder,
                        &known_heads,
                        content_hash.as_deref(),
                    );
                    let (snaps, cues) = match &lineage {
                        Some(key) => (
                            s.find_missing(key, &known_heads),
                            s.cues.get(key).cloned().unwrap_or_default(),
                        ),
                        None => {
                            // The relay holds several projects with this
                            // name and cannot tell which one the client
                            // means. It serves none of them: handing over
                            // the wrong project's history is worse than
                            // handing over nothing.
                            let lineages = s.lineages_of(&client_name);
                            if lineages.len() > 1 {
                                warn!(
                                    "Client {} asked for '{}' but the relay holds {} projects with that name — serving none of them",
                                    addr, client_name, lineages.len()
                                );
                            }
                            (Vec::new(), Vec::new())
                        }
                    };
                    SyncMsg::Push { snapshots: snaps, cues }
                };

                state.lock().unwrap().upsert_client(client.clone());
                let _ = client.send(&resp);
            }
            SyncMsg::BlobPush { chunk } => {
                debug!(
                    "Client {} pushing blob chunk {}/{} for {}",
                    addr,
                    chunk.index + 1,
                    chunk.total,
                    chunk.hash
                );
                match state.lock().unwrap().store_blob_chunk(&chunk) {
                    Ok(true) => {
                        let _ = client.send(&SyncMsg::BlobStored {
                            hash: chunk.hash.clone(),
                        });
                    }
                    Ok(false) => {}
                    Err(e) => {
                        warn!("Rejected blob chunk from {}: {}", addr, e);
                    }
                }
            }
            SyncMsg::BlobEnd { project, hash } => {
                // The chunks are written as they complete, so the end of
                // a blob is only a confirmation that nothing is missing.
                let has = state.lock().unwrap().has_blob(&hash);
                if has {
                    let _ = client.send(&SyncMsg::BlobStored { hash });
                } else {
                    warn!(
                        "Client {} said blob {} for {} was finished, but it never arrived",
                        addr, hash, project
                    );
                }
            }
            SyncMsg::BlobRequest { project, hash } => {
                let data = state.lock().unwrap().read_blob(&hash);
                match data {
                    Some(bytes) => {
                        let chunks =
                            clavus_sync::blob_transfer::chunk_blob(&project, &hash, &bytes);
                        for chunk in chunks {
                            let _ = client.send(&SyncMsg::BlobChunk { chunk });
                        }
                        let _ = client.send(&SyncMsg::BlobDone { project, hash });
                    }
                    None => {
                        let _ = client.send(&SyncMsg::BlobMissing { project, hash });
                    }
                }
            }
            SyncMsg::BlobChunk { chunk } => {
                // A chunk is how the relay *sends* bytes. A client that
                // sends one back is not following the protocol.
                warn!(
                    "Client {} sent a blob chunk where none belongs ({} for {})",
                    addr, chunk.hash, chunk.project
                );
            }
            SyncMsg::BlobStored { hash } => {
                debug!("Client {} stored blob {}", addr, hash);
            }
            SyncMsg::BlobDone { project, hash } => {
                debug!("Client {} finished blob {} for {}", addr, hash, project);
            }
            SyncMsg::BlobMissing { project, hash } => {
                warn!("Relay has no bytes for {} ({})", project, hash);
            }
            SyncMsg::Pull { missing } => {
                debug!("Client {} requesting {} missing objects", addr, missing.len());
                let (snapshots, cues) = {
                    let s = state.lock().unwrap();
                    // Which project does the client mean? The client's
                    // name is the wire name; the folder tells two
                    // projects that share a file name apart.
                    let lineage = s.resolve_lineage(
                        &client.name,
                        &client.folder,
                        &client.known_heads,
                        client.content_hash.as_deref(),
                    );
                    match lineage {
                        Some(key) => {
                            // Only the client's own project's snapshots
                            // are served.
                            let snapshots: Vec<Snapshot> = missing
                                .iter()
                                .filter_map(|h| {
                                    s.snapshots
                                        .get(&key)
                                        .and_then(|by_hash| by_hash.get(h))
                                        .cloned()
                                })
                                .collect();
                            let cues: Vec<Cue> = s.cues.get(&key).cloned().unwrap_or_default();
                            (snapshots, cues)
                        }
                        // The relay cannot tell which project the client
                        // means, so it serves none of them.
                        None => (Vec::new(), Vec::new()),
                    }
                };
                let _ = client.send(&SyncMsg::PullResp { snapshots, cues });
            }
            SyncMsg::Push { snapshots, cues } => {
                debug!("Client {} pushing {} snapshots, {} cues", addr, snapshots.len(), cues.len());
                // The cues go beside the snapshots of the project the
                // client introduced itself with.
                let identity = (client.name.clone(), client.folder.clone());
                let conflicts =
                    state.lock().unwrap().store_for_client(snapshots, cues, Some(identity));
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

/// The relay's key for a project: its file name and the folder it lives
/// in.
///
/// The file name alone is not enough. Two projects that share a file
/// name — `Album1/Track.als` and `Album2/Track.als` — are two
/// different sets, and a relay that filed them together handed one
/// project's history and notes to the other.
///
/// The folder is the folder's *path*, not just its name: `p1/studio`
/// and `p2/studio` are two folders, even though both are called
/// `studio`.
fn relay_key_from_path(path: &str, folder: &str) -> String {
    let file_name = project_name_from_path(path);
    clavus_core::project::relay_key_for(&file_name, folder)
}

/// The relay's key for a snapshot's project.
fn relay_key_of(snap: &Snapshot) -> String {
    let folder = if snap.project_folder.is_empty() {
        // An older client that does not send the folder. The last
        // component of the path is the best guess there is.
        std::path::Path::new(&snap.project_path)
            .parent()
            .and_then(|d| d.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty() && s != "." && s != "..")
            .unwrap_or_default()
    } else {
        snap.project_folder.clone()
    };
    relay_key_from_path(&snap.project_path, &folder)
}

/// The wire name of a relay key: the file name on its own.
fn wire_name_of(key: &str) -> String {
    // The key is `file name::folder`, and both parts are escaped, so
    // the first separator is the real one.
    let name = key.split("::").next().unwrap_or(key);
    clavus_core::project::unescape_key_part(name)
}
