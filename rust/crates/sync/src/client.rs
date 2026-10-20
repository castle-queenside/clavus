//! Clavus sync client — connects to a relay server and syncs snapshots + cues.

use clavus_core::types::{Cue, ClavusError, Snapshot};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Messages in the Clavus sync protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncMsg {
    Hello { client_name: String, known_heads: Vec<String> },
    Push { snapshots: Vec<Snapshot>, cues: Vec<Cue> },
    Pull { missing: Vec<String> },
    PullResp { snapshots: Vec<Snapshot>, cues: Vec<Cue> },
    Synced,
    /// The relay detected a divergent HEAD for a project.
    /// Client should pull and resolve.
    Conflict { project: String, relay_head: String, push_head: String },
    Subscribe { project: String },
    Event { project: String, event: SyncEvent },
}

/// Events broadcast to subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncEvent {
    NewSnapshot { project: String, hash: String },
    NewCue { project: String, cue_id: String },
    CueResolved { project: String, cue_id: String },
}

/// Maximum payload size: 10 MB.
const MAX_PAYLOAD: usize = 10 * 1024 * 1024;

/// Result type alias for this module.
pub type Result<T> = std::result::Result<T, ClavusError>;

/// A connected sync client.
///
/// Spawns a background receiver thread that delivers messages via `recv()`.
pub struct SyncClient {
    addr: String,
    stream: Arc<Mutex<TcpStream>>,
    /// Channel on which the receiver thread delivers incoming messages.
    rx: Receiver<SyncMsg>,
    /// Set to true when the connection is closed.
    closed: Arc<Mutex<bool>>,
}

impl SyncClient {
    /// Connect to a relay at `addr` (e.g. `"127.0.0.1:4477"`).
    pub fn connect(addr: &str) -> Result<Self> {
        let stream = TcpStream::connect(addr)
            .map_err(ClavusError::from)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(ClavusError::from)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(ClavusError::from)?;

        let (tx, rx) = mpsc::channel();
        let closed = Arc::new(Mutex::new(false));
        let stream0 = stream.try_clone().map_err(ClavusError::from)?;
        let closed0 = Arc::clone(&closed);

        // Receiver thread: reads framed messages from the server and delivers them.
        thread::spawn(move || {
            let stream = Arc::new(Mutex::new(stream0));
            loop {
                if *closed0.lock().unwrap() {
                    break;
                }
                let msg = match recv_msg(&stream, &closed0) {
                    Ok(m) => m,
                    Err(_) => break,
                };
                if tx.send(msg).is_err() {
                    break; // main thread dropped the receiver
                }
            }
        });

        Ok(Self {
            addr: addr.to_string(),
            stream: Arc::new(Mutex::new(stream)),
            rx,
            closed,
        })
    }

    /// Send a Hello message and announce the client's known snapshot heads.
    pub fn hello(&self, client_name: &str, known_heads: Vec<String>) -> Result<()> {
        self.send(SyncMsg::Hello {
            client_name: client_name.to_string(),
            known_heads,
        })
    }

    /// Subscribe to updates for a project.
    pub fn subscribe(&self, project: &str) -> Result<()> {
        self.send(SyncMsg::Subscribe {
            project: project.to_string(),
        })
    }

    /// Pull missing snapshots from the relay.
    pub fn pull(&self, missing: Vec<String>) -> Result<()> {
        self.send(SyncMsg::Pull { missing })
    }

    /// Respond to a Pull with the requested objects.
    pub fn pull_resp(&self, snapshots: Vec<Snapshot>, cues: Vec<Cue>) -> Result<()> {
        self.send(SyncMsg::PullResp { snapshots, cues })
    }

    /// Mark that we've processed a Push (acknowledge sync is complete).
    pub fn synced(&self) -> Result<()> {
        self.send(SyncMsg::Synced)
    }

    /// Receive the next message from the relay.
    ///
    /// Blocks for up to `timeout`. Returns `None` if no message arrived
    /// within the timeout or if the connection was closed.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<SyncMsg> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Receive the next message, blocking indefinitely.
    /// Returns `None` if the channel was closed.
    ///
    /// Prefer [`recv_timeout`] with a bounded timeout to avoid blocking forever
    /// if the relay becomes unresponsive.
    pub fn recv(&self) -> Option<SyncMsg> {
        self.rx.recv().ok()
    }

    /// Send a message to the relay.
    pub fn send(&self, msg: SyncMsg) -> Result<()> {
        if *self.closed.lock().unwrap() {
            return Err(ClavusError::SyncError("sync client closed".into()));
        }
        let mut stream = self.stream.lock().unwrap();
        send_msg(&mut stream, &msg)
    }

    /// Close the connection.
    pub fn close(&self) {
        *self.closed.lock().unwrap() = true;
    }

    /// Returns true if the connection is closed.
    pub fn is_closed(&self) -> bool {
        *self.closed.lock().unwrap()
    }

    /// Relay address this client is connected to.
    pub fn relay_addr(&self) -> &str {
        &self.addr
    }
}

impl Drop for SyncClient {
    fn drop(&mut self) {
        self.close();
    }
}

// ─── Framed I/O helpers ─────────────────────────────────────────────

/// Send a framed JSON message (4-byte BE length prefix).
fn send_msg(stream: &mut TcpStream, msg: &SyncMsg) -> Result<()> {
    let json = serde_json::to_vec(msg)
        .map_err(|e| ClavusError::SyncError(format!("json serialize: {}", e)))?;
    let len = json.len() as u32;

    stream
        .write_all(&len.to_be_bytes())
        .map_err(ClavusError::from)?;
    stream
        .write_all(&json)
        .map_err(ClavusError::from)?;
    stream.flush().map_err(ClavusError::from)?;
    Ok(())
}

/// Receive a framed JSON message. Returns Err if the connection is closed.
fn recv_msg(stream: &Arc<Mutex<TcpStream>>, closed: &Arc<Mutex<bool>>) -> Result<SyncMsg> {
    if *closed.lock().unwrap() {
        return Err(ClavusError::SyncError("connection closed".into()));
    }

    let mut len_buf = [0u8; 4];
    let n = {
        let mut s = stream.lock().unwrap();
        s.read_exact(&mut len_buf).map_err(|e| {
            *closed.lock().unwrap() = true;
            ClavusError::from(e)
        })?;
        u32::from_be_bytes(len_buf) as usize
    };

    if n > MAX_PAYLOAD {
        *closed.lock().unwrap() = true;
        return Err(ClavusError::SyncError(format!(
            "payload too large: {} bytes (max {})",
            n, MAX_PAYLOAD
        )));
    }

    let mut data = vec![0u8; n];
    {
        let mut s = stream.lock().unwrap();
        s.read_exact(&mut data).map_err(|e| {
            *closed.lock().unwrap() = true;
            ClavusError::from(e)
        })?;
    }

    serde_json::from_slice(&data)
        .map_err(|e| ClavusError::SyncError(format!("json deserialize: {}", e)))
}
