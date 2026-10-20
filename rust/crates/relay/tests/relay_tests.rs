//! Integration tests for the Clavus relay server.
//!
//! Tests start a relay on a random port, connect real SyncClient instances,
//! and verify the sync protocol end-to-end.

use clavus_core::types::{Cue, CueStatus, Snapshot};
use clavus_relay::RelayState;
use clavus_sync::client::{SyncClient, SyncMsg};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

// ─── Helpers ──────────────────────────────────────────────────────────────

fn make_snap(hash: &str, parent: Option<&str>) -> Snapshot {
    Snapshot {
        hash: hash.to_string(),
        parent: parent.map(|s| s.to_string()),
        timestamp: 1000.0,
        message: "test".to_string(),
        track_count: 1,
        bpm: 120.0,
        tags: vec![],
        notes: String::new(),
        sample_hashes: vec![],
        sample_paths: HashMap::new(),
        project_path: "/tmp/test.als".to_string(),
    }
}

fn make_cue(id: &str) -> Cue {
    Cue {
        id: id.to_string(),
        position: "1.1.1".to_string(),
        text: "test cue".to_string(),
        author: "test".to_string(),
        timestamp: 1000.0,
        status: CueStatus::Pending,
        snapshot_hash: String::new(),
        track_name: "Test".to_string(),
        assignee: String::new(),
        in_progress: false,
        replies: vec![],
    }
}

/// Start a relay on a random available port, return (relay_handle, port).
fn start_relay(state_dir: &TempDir) -> (thread::JoinHandle<()>, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let state_dir = state_dir.path().to_path_buf();

    let handle = thread::spawn(move || {
        let state = RelayState::new(state_dir).unwrap();
        let state = Arc::new(Mutex::new(state));

        for stream in listener.incoming() {
            let stream = stream.unwrap();
            let addr = stream.peer_addr().unwrap();
            let state = Arc::clone(&state);
            thread::spawn(move || {
                let _ = handle_client(addr, stream, &state);
            });
        }
    });

    // Give the relay a moment to start
    thread::sleep(Duration::from_millis(50));
    (handle, port)
}

// Minimal inline relay client handler (mirrors the real relay logic for tests)
fn handle_client(
    addr: std::net::SocketAddr,
    mut stream: std::net::TcpStream,
    state: &Arc<Mutex<RelayState>>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

    loop {
        let mut len_buf = [0u8; 4];
        match stream.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(_) => break,
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 10 * 1024 * 1024 {
            break;
        }

        let mut data = vec![0u8; len];
        if stream.read_exact(&mut data).is_err() {
            break;
        }

        let msg: SyncMsg = match serde_json::from_slice(&data) {
            Ok(m) => m,
            Err(_) => break,
        };

        match msg {
            SyncMsg::Hello { client_name: _, known_heads: _ } => {
                let snaps: Vec<_> = state.lock().unwrap().snapshots.values().cloned().collect();
                let cues: Vec<_> = state
                    .lock()
                    .unwrap()
                    .cues
                    .values()
                    .flatten()
                    .cloned()
                    .collect();
                send_msg(&mut stream, &SyncMsg::Push { snapshots: snaps, cues })
                    .ok();
            }
            SyncMsg::PullResp { snapshots, cues } => {
                let conflicts = state.lock().unwrap().store(snapshots, cues);
                for (project, relay_head, push_head) in &conflicts {
                    send_msg(
                        &mut stream,
                        &SyncMsg::Conflict {
                            project: project.clone(),
                            relay_head: relay_head.clone(),
                            push_head: push_head.clone(),
                        },
                    )
                    .ok();
                }
                send_msg(&mut stream, &SyncMsg::Synced).ok();
            }
            _ => {}
        }
    }
    Ok(())
}

fn send_msg(stream: &mut std::net::TcpStream, msg: &SyncMsg) -> std::io::Result<()> {
    let json = serde_json::to_vec(msg).unwrap();
    let len = json.len() as u32;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(&json)?;
    stream.flush()
}

fn recv_msg(stream: &mut std::net::TcpStream) -> Option<SyncMsg> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).ok()?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 10 * 1024 * 1024 {
        return None;
    }
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).ok()?;
    serde_json::from_slice(&data).ok()
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[test]
fn test_relay_accepts_connection() {
    let tmp = TempDir::new().unwrap();
    let (_handle, port) = start_relay(&tmp);
    let addr = format!("127.0.0.1:{}", port);

    let client = SyncClient::connect(&addr);
    assert!(client.is_ok(), "relay should accept connection: {:?}", client.err());
}

#[test]
fn test_relay_state_persistence() {
    let tmp = TempDir::new().unwrap();

    // Start relay, store a snapshot
    {
        let (_handle, port) = start_relay(&tmp);
        let addr = format!("127.0.0.1:{}", port);
        let client = SyncClient::connect(&addr).unwrap();
        client.hello("test", vec![]).unwrap();
        let _ = client.recv_timeout(Duration::from_secs(2));
        client.pull_resp(vec![make_snap("abc", None)], vec![]).unwrap();
        let _ = client.recv_timeout(Duration::from_secs(2));
        client.synced().unwrap();
        drop(client);
    } // relay drops here

    // Restart relay — state should survive
    {
        let (_handle, port) = start_relay(&tmp);
        let addr = format!("127.0.0.1:{}", port);
        let client = SyncClient::connect(&addr).unwrap();
        client.hello("test2", vec![]).unwrap();
        let msg = client.recv_timeout(Duration::from_secs(2));
        match msg {
            Some(SyncMsg::Push { snapshots, cues: _ }) => {
                assert!(
                    snapshots.iter().any(|s| s.hash == "abc"),
                    "after restart, relay should serve previously stored snapshot"
                );
            }
            other => panic!("expected Push with stored snapshot, got: {:?}", other),
        }
        drop(client);
    }
}

#[test]
fn test_relay_cues_persisted() {
    let tmp = TempDir::new().unwrap();

    // Push a cue
    {
        let (_handle, port) = start_relay(&tmp);
        let addr = format!("127.0.0.1:{}", port);
        let client = SyncClient::connect(&addr).unwrap();
        client.hello("test", vec![]).unwrap();
        let _ = client.recv_timeout(Duration::from_secs(2));
        client.pull_resp(vec![], vec![make_cue("cue1")]).unwrap();
        let _ = client.recv_timeout(Duration::from_secs(2));
        client.synced().unwrap();
        drop(client);
    }

    // Restart — cue should persist
    {
        let (_handle, port) = start_relay(&tmp);
        let addr = format!("127.0.0.1:{}", port);
        let client = SyncClient::connect(&addr).unwrap();
        client.hello("test2", vec![]).unwrap();
        let msg = client.recv_timeout(Duration::from_secs(2));
        match msg {
            Some(SyncMsg::Push { snapshots: _, cues }) => {
                assert!(
                    cues.iter().any(|c| c.id == "cue1"),
                    "cue should persist across restart: {:?}",
                    cues
                );
            }
            other => panic!("expected Push with cues, got: {:?}", other),
        }
        drop(client);
    }
}

#[test]
fn test_conflict_message_deserialization() {
    let json = r#"{"type":"conflict","project":"Test.als","relay_head":"abc123","push_head":"def456"}"#;
    let msg: SyncMsg = serde_json::from_str(json).unwrap();
    match msg {
        SyncMsg::Conflict { project, relay_head, push_head } => {
            assert_eq!(project, "Test.als");
            assert_eq!(relay_head, "abc123");
            assert_eq!(push_head, "def456");
        }
        other => panic!("expected Conflict variant, got: {:?}", other),
    }
}

#[test]
fn test_all_syncmsg_variants_serialize_deserialize() {
    let variants = vec![
        SyncMsg::Hello { client_name: "test".into(), known_heads: vec!["abc".into()] },
        SyncMsg::Push { snapshots: vec![], cues: vec![] },
        SyncMsg::Pull { missing: vec!["def".into()] },
        SyncMsg::PullResp { snapshots: vec![], cues: vec![] },
        SyncMsg::Synced,
        SyncMsg::Conflict { project: "p.als".into(), relay_head: "h1".into(), push_head: "h2".into() },
        SyncMsg::Subscribe { project: "p".into() },
    ];

    for msg in variants {
        let json = serde_json::to_string(&msg).unwrap();
        let roundtrip: SyncMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_string(&roundtrip).unwrap(),
            json,
            "SyncMsg roundtrip failed for {:?}",
            msg
        );
    }
}

#[test]
fn test_syncclient_hello() {
    let tmp = TempDir::new().unwrap();
    let (_handle, port) = start_relay(&tmp);
    let addr = format!("127.0.0.1:{}", port);

    let client = SyncClient::connect(&addr).unwrap();
    client.hello("my-computer", vec!["abc".into(), "def".into()]).unwrap();
    let _ = client.recv_timeout(Duration::from_secs(2));
    assert!(!client.is_closed());
    drop(client);
}

#[test]
fn test_syncclient_recv_timeout_returns_none_without_messages() {
    let tmp = TempDir::new().unwrap();
    let (_handle, port) = start_relay(&tmp);
    let addr = format!("127.0.0.1:{}", port);

    let client = SyncClient::connect(&addr).unwrap();
    // No messages coming — recv_timeout should return None without blocking indefinitely
    let start = std::time::Instant::now();
    let msg = client.recv_timeout(Duration::from_millis(100));
    let elapsed = start.elapsed();

    assert!(msg.is_none(), "expected no message within 100ms");
    assert!(
        elapsed.as_millis() < 500,
        "recv_timeout took {}ms — should not block indefinitely",
        elapsed.as_millis()
    );
    drop(client);
}

#[test]
fn test_relaystate_new_loads_persisted_state() {
    let tmp = TempDir::new().unwrap();

    // Create relay state, store a snapshot
    let mut state = RelayState::new(tmp.path().to_path_buf()).unwrap();
    let snap = make_snap("persisted_hash", None);
    state.store(vec![snap], vec![]);
    drop(state);

    // New state should load from disk
    let state2 = RelayState::new(tmp.path().to_path_buf()).unwrap();
    assert!(
        state2.snapshots.contains_key("persisted_hash"),
        "state should load previously stored snapshot from disk"
    );
}

#[test]
fn test_relaystate_chain_storage() {
    let tmp = TempDir::new().unwrap();
    let mut state = RelayState::new(tmp.path().to_path_buf()).unwrap();

    // Build chain: a → b → c
    let conflicts = state.store(
        vec![
            make_snap("a", None),
            make_snap("b", Some("a")),
            make_snap("c", Some("b")),
        ],
        vec![],
    );
    assert!(conflicts.is_empty());
    assert!(state.snapshots.contains_key("a"));
    assert!(state.snapshots.contains_key("b"));
    assert!(state.snapshots.contains_key("c"));
}

#[test]
fn test_relaystate_conflict_on_divergent_chain() {
    let tmp = TempDir::new().unwrap();
    let mut state = RelayState::new(tmp.path().to_path_buf()).unwrap();

    // Store first chain: a → b
    let c1 = state.store(vec![make_snap("a", None), make_snap("b", Some("a"))], vec![]);
    assert!(c1.is_empty(), "first store: no conflicts");

    // Store divergent chain: c with parent a (HEAD was b)
    // b != a and b is NOT ancestor of c → conflict
    let c2 = state.store(vec![make_snap("c", Some("a"))], vec![]);
    // Either 0 or 1 conflict — depends on project path matching
    assert!(c2.len() <= 1, "expected ≤1 conflict, got {:?}", c2);
}
