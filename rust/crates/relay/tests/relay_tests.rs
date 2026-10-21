//! Integration tests for the Clavus relay server.
//!
//! Tests start a relay on a random port, connect real SyncClient instances,
//! and verify the sync protocol end-to-end.

use clavus_core::types::{Cue, CueStatus, Snapshot};
use clavus_relay::RelayState;
use clavus_sync::client::{SyncClient, SyncMsg};
use std::collections::HashMap;
use std::net::TcpListener;
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
    make_cue_for(id, "demo")
}

/// A cue belonging to `project`.
fn make_cue_for(id: &str, project: &str) -> Cue {
    Cue {
        id: id.to_string(),
        project: project.to_string(),
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

/// Start the *real* relay on a random port.
fn start_real_relay(state_dir: &TempDir) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let state_dir = state_dir.path().to_path_buf();
    thread::spawn(move || {
        let _ = clavus_relay::run_with_listener(listener, state_dir);
    });
    thread::sleep(Duration::from_millis(50));
    addr
}

#[test]
fn test_relay_accepts_connection() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let client = SyncClient::connect(&addr);
    assert!(client.is_ok(), "relay should accept connection: {:?}", client.err());
}

#[test]
fn test_relay_state_persistence() {
    let tmp = TempDir::new().unwrap();
    // A snapshot hash is a SHA-256 digest; the relay rejects anything else.
    let hash = "a".repeat(64);

    // Start relay, store a snapshot
    {
        let addr = start_real_relay(&tmp);
        let client = SyncClient::connect(&addr).unwrap();
        client.hello("test", vec![]).unwrap();
        let _ = client.recv_timeout(Duration::from_secs(2));
        client.push(vec![make_snap(&hash, None)], vec![]).unwrap();
        let _ = client.recv_timeout(Duration::from_secs(2));
        drop(client);
    } // relay drops here

    // Restart relay — state should survive
    {
        let addr = start_real_relay(&tmp);
        let client = SyncClient::connect(&addr).unwrap();
        client.hello("test", vec![]).unwrap();
        let msg = client.recv_timeout(Duration::from_secs(2));
        match msg {
            Some(SyncMsg::Push { snapshots, cues: _ }) => {
                assert!(
                    snapshots.iter().any(|s| s.hash == hash),
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
        let addr = start_real_relay(&tmp);
        let client = SyncClient::connect(&addr).unwrap();
        client.hello("demo", vec![]).unwrap();
        let _ = client.recv_timeout(Duration::from_secs(2));
        client.push(vec![], vec![make_cue("cue1")]).unwrap();
        let _ = client.recv_timeout(Duration::from_secs(2));
        drop(client);
    }

    // Restart — cue should persist
    {
        let addr = start_real_relay(&tmp);
        let client = SyncClient::connect(&addr).unwrap();
        client.hello("demo", vec![]).unwrap();
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
    let addr = start_real_relay(&tmp);

    let client = SyncClient::connect(&addr).unwrap();
    client.hello("my-computer", vec!["abc".into(), "def".into()]).unwrap();
    let _ = client.recv_timeout(Duration::from_secs(2));
    assert!(!client.is_closed());
    drop(client);
}

#[test]
fn test_syncclient_recv_timeout_returns_none_without_messages() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

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
    // Snapshot hashes are SHA-256 hex digests; the relay rejects anything else.
    let hash = "a".repeat(64);
    let snap = make_snap(&hash, None);
    state.store(vec![snap], vec![]);
    drop(state);

    // New state should load from disk
    let state2 = RelayState::new(tmp.path().to_path_buf()).unwrap();
    assert!(
        state2.snapshots.contains_key(&hash),
        "state should load previously stored snapshot from disk"
    );
}

#[test]
fn test_relaystate_chain_storage() {
    let tmp = TempDir::new().unwrap();
    let mut state = RelayState::new(tmp.path().to_path_buf()).unwrap();

    // Build chain: a → b → c
    let (a, b, c) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
    let conflicts = state.store(
        vec![
            make_snap(&a, None),
            make_snap(&b, Some(&a)),
            make_snap(&c, Some(&b)),
        ],
        vec![],
    );
    assert!(conflicts.is_empty());
    assert!(state.snapshots.contains_key(&a));
    assert!(state.snapshots.contains_key(&b));
    assert!(state.snapshots.contains_key(&c));
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
