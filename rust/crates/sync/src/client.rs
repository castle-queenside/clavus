//! Clavus sync client — connects to a relay server and syncs snapshots + cues.

use crate::validate;
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
    Hello {
        client_name: String,
        known_heads: Vec<String>,
        /// The path of the folder the project lives in, relative to the
        /// home directory.
        ///
        /// Two projects can share a file name, and the folder tells
        /// them apart. It is the folder's *path* — `p1/studio` and
        /// `p2/studio` are two folders — because two projects in
        /// folders of the same name are two projects.
        #[serde(default)]
        folder: String,
        /// The hash of the client's project file.
        ///
        /// It is the hash a snapshot of that file carries, so the relay
        /// can tell which project the client means even when the file
        /// sits in a different folder on another machine.
        #[serde(default)]
        content_hash: Option<String>,
    },
    Push { snapshots: Vec<Snapshot>, cues: Vec<Cue> },
    Pull { missing: Vec<String> },
    PullResp { snapshots: Vec<Snapshot>, cues: Vec<Cue> },
    Synced,
    /// The relay detected a divergent HEAD for a project.
    /// Client should pull and resolve.
    Conflict { project: String, relay_head: String, push_head: String },
    Subscribe { project: String },
    Event { project: String, event: SyncEvent },
    /// A chunk of a project's bytes, on its way to the relay.
    BlobPush { chunk: crate::blob_transfer::BlobChunk },
    /// The last chunk of a blob has been sent.
    BlobEnd { project: String, hash: String },
    /// The relay has the blob and has checked its hash.
    BlobStored { hash: String },
    /// The client asks for a project's bytes.
    BlobRequest { project: String, hash: String },
    /// A chunk of a project's bytes, on its way to the client.
    BlobChunk { chunk: crate::blob_transfer::BlobChunk },
    /// The whole blob has been sent.
    BlobDone { project: String, hash: String },
    /// The relay does not have those bytes.
    BlobMissing { project: String, hash: String },
    /// The relay could not do what it was asked to, and says why.
    ///
    /// A relay that cannot write its state to disk must not acknowledge
    /// a push: the history would be gone on the next restart. The
    /// client is told, and the operation fails.
    Error { message: String },
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

/// How long to wait for a relay to answer a connection attempt.
///
/// The operating system's own timeout is over a minute, which left the
/// terminal frozen with no output.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Connect to a relay, giving up after `timeout`.
///
/// A plain `TcpStream::connect` waits for the operating system to give
/// up, which on a firewalled address took over a minute.
fn connect_with_timeout(addr: &str, timeout: Duration) -> std::io::Result<TcpStream> {
    use std::net::ToSocketAddrs;

    let mut last_error = std::io::Error::other("no address to connect to");
    for socket_addr in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&socket_addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

/// Result type alias for this module.
pub type Result<T> = std::result::Result<T, ClavusError>;

/// Why a blob did not arrive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobError {
    /// The relay does not have those bytes.
    Missing,
    /// The relay did not answer in time.
    Timeout,
    /// The bytes do not hash to the name they were sent under.
    HashMismatch(String),
    /// The relay sent something else.
    WrongBlob,
    /// The relay stopped before the whole blob arrived.
    Incomplete,
}

impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlobError::Missing => write!(f, "the relay does not have those bytes"),
            BlobError::Timeout => write!(f, "the relay did not send the bytes in time"),
            BlobError::HashMismatch(e) => write!(f, "the bytes do not match their hash: {}", e),
            BlobError::WrongBlob => write!(f, "the relay sent a different blob"),
            BlobError::Incomplete => write!(f, "the relay stopped before the blob was complete"),
        }
    }
}

impl std::error::Error for BlobError {}

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
        // A connect that never answers must not freeze the terminal for
        // a minute. The operating system's own timeout is far longer
        // than a user is willing to wait.
        let stream = connect_with_timeout(addr, CONNECT_TIMEOUT)
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
                // Everything the server sends is untrusted. A malformed
                // hash or cue id would otherwise become a file name, or
                // crash the display code that truncates it.
                let msg = match msg {
                    SyncMsg::Push { snapshots, cues } => SyncMsg::Push {
                        snapshots: validate::filter_snapshots(snapshots),
                        cues: validate::filter_cues(cues),
                    },
                    SyncMsg::PullResp { snapshots, cues } => SyncMsg::PullResp {
                        snapshots: validate::filter_snapshots(snapshots),
                        cues: validate::filter_cues(cues),
                    },
                    other => other,
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
        self.hello_in(client_name, known_heads, "", None)
    }

    /// Say hello, saying which folder the project lives in.
    ///
    /// The folder tells two projects that share a file name apart, so
    /// a relay never hands one project's history to the other.
    pub fn hello_in(
        &self,
        client_name: &str,
        known_heads: Vec<String>,
        folder: &str,
        content_hash: Option<String>,
    ) -> Result<()> {
        self.send(SyncMsg::Hello {
            client_name: client_name.to_string(),
            known_heads,
            folder: folder.to_string(),
            content_hash,
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

    /// Push snapshots and cues to the relay.
    ///
    /// The relay replies with `Synced`, or with `Conflict` messages when it
    /// detects a divergent HEAD for one of the projects involved.
    pub fn push(&self, snapshots: Vec<Snapshot>, cues: Vec<Cue>) -> Result<()> {
        self.send(SyncMsg::Push { snapshots, cues })
    }

    /// Send a project's bytes to the relay.
    ///
    /// The bytes travel in chunks, and the relay checks that the whole
    /// blob hashes to the name it was sent under before it stores it.
    pub fn push_blob(&self, project: &str, hash: &str, data: &[u8]) -> Result<()> {
        for chunk in crate::blob_transfer::chunk_blob(project, hash, data) {
            self.send(SyncMsg::BlobPush { chunk })?;
        }
        self.send(SyncMsg::BlobEnd {
            project: project.to_string(),
            hash: hash.to_string(),
        })
    }

    /// Ask the relay for a project's bytes.
    pub fn request_blob(&self, project: &str, hash: &str) -> Result<()> {
        self.send(SyncMsg::BlobRequest {
            project: project.to_string(),
            hash: hash.to_string(),
        })
    }

    /// Collect a blob that the relay is sending.
    ///
    /// Returns the bytes once every chunk has arrived. The hash is
    /// checked, so a relay cannot hand over bytes under the wrong name.
    pub fn recv_blob(
        &self,
        _project: &str,
        hash: &str,
        timeout: Duration,
    ) -> Result<std::result::Result<Vec<u8>, BlobError>> {
        let mut assembler = crate::blob_transfer::BlobAssembler::new();
        let deadline = std::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(Err(BlobError::Timeout));
            }
            let Some(msg) = self.recv_timeout(remaining) else {
                return Ok(Err(BlobError::Timeout));
            };
            match msg {
                SyncMsg::BlobChunk { chunk } => {
                    if chunk.hash != hash {
                        return Ok(Err(BlobError::WrongBlob));
                    }
                    match assembler.add(&chunk) {
                        Ok(Some(whole)) => {
                            if let Err(e) = crate::blob_transfer::verify_blob(hash, &whole) {
                                return Ok(Err(BlobError::HashMismatch(e)));
                            }
                            return Ok(Ok(whole));
                        }
                        Ok(None) => {}
                        Err(e) => return Ok(Err(BlobError::HashMismatch(e))),
                    }
                }
                SyncMsg::BlobDone { hash: done_hash, .. } => {
                    // The relay sends the chunks and then says the blob
                    // is finished. The last chunk completes the blob
                    // before that message arrives, so the message for
                    // the blob just fetched is still in the queue when
                    // the next blob is fetched. It belongs to the
                    // previous blob and says nothing about this one.
                    if done_hash == hash {
                        return Ok(Err(BlobError::Incomplete));
                    }
                }
                SyncMsg::BlobMissing { .. } => {
                    return Ok(Err(BlobError::Missing));
                }
                _ => {}
            }
        }
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
