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
    /// All snapshots known to the relay, keyed by hash.
    pub snapshots: HashMap<String, Snapshot>,
    /// All cues known to the relay, keyed by project.
    pub cues: HashMap<String, Vec<Cue>>,
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
        info!(
            "Relay state loaded: {} snapshots, {} cue lists",
            snapshots.len(),
            cues.len()
        );
        Ok(Self {
            snapshots,
            cues,
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

    fn load_snaps(dir: &std::path::Path) -> std::io::Result<HashMap<String, Snapshot>> {
        let path = Self::snaps_path(dir);
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = std::fs::read_to_string(&path)?;
        let snaps: Vec<Snapshot> = serde_json::from_str(&data)
            .map_err(|e| std::io::Error::other(format!("snapshots parse: {}", e)))?;
        Ok(snaps.into_iter().map(|s| (s.hash.clone(), s)).collect())
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

    /// Persist snapshots and cues to disk (atomic: temp file + rename).
    fn persist(&self) -> std::io::Result<()> {
        Self::atomic_json_write(
            &Self::snaps_path(&self.state_dir),
            &self.snapshots.values().cloned().collect::<Vec<_>>(),
        )?;
        Self::atomic_json_write(
            &Self::cues_path(&self.state_dir),
            &self.cues,
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

    /// Find snapshots the client is missing.
    pub fn find_missing(&self, known: &[String]) -> Vec<Snapshot> {
        self.snapshots
            .values()
            .filter(|s| !known.contains(&s.hash))
            .cloned()
            .collect()
    }

    /// Store incoming snapshots and cues. Persists to disk.
    pub fn store(&mut self, snapshots: Vec<Snapshot>, cues: Vec<Cue>) {
        for snap in snapshots {
            self.snapshots.insert(snap.hash.clone(), snap);
        }
        for cue in cues {
            let project = cue.track_name.clone();
            self.cues
                .entry(project)
                .or_default()
                .push(cue);
        }
        // Persist after every store to ensure durability
        if let Err(e) = self.persist() {
            warn!("Relay persist failed: {}", e);
        }
    }
}

/// Run the relay server.
pub fn run(bind_addr: &str, state_dir: std::path::PathBuf) -> std::io::Result<()> {
    let listener = TcpListener::bind(bind_addr)?;
    info!("Relay listening on {}", bind_addr);

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
        // Read 4-byte length prefix
        let mut len_buf = [0u8; 4];
        {
            let mut s = client.stream.lock().unwrap();
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
                    let snaps = s.find_missing(&known_heads);
                    let cues: Vec<Cue> = s.cues.values().flatten().cloned().collect();
                    SyncMsg::Push { snapshots: snaps, cues }
                };

                state.lock().unwrap().upsert_client(client.clone());
                let _ = client.send(&resp);
            }
            SyncMsg::Pull { missing } => {
                debug!("Client {} requesting {} missing objects", addr, missing.len());
                let (snapshots, cues) = {
                    let s = state.lock().unwrap();
                    let snapshots: Vec<Snapshot> = missing
                        .iter()
                        .filter_map(|h| s.snapshots.get(h).cloned())
                        .collect();
                    let cues: Vec<Cue> = s.cues.values().flatten().cloned().collect();
                    (snapshots, cues)
                };
                let _ = client.send(&SyncMsg::PullResp { snapshots, cues });
            }
            SyncMsg::PullResp { snapshots, cues } => {
                debug!("Client {} pushing {} snapshots, {} cues", addr, snapshots.len(), cues.len());
                state.lock().unwrap().store(snapshots, cues);
                let _ = client.send(&SyncMsg::Synced);
            }
            SyncMsg::Ping => {
                let _ = client.send(&SyncMsg::Pong);
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
