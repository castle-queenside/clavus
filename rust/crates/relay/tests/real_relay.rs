//! Integration tests for the *real* relay server.
//!
//! These start `clavus_relay::run_with_listener` and drive it with real
//! `SyncClient` instances, so the actual production code path is exercised —
//! not a re-implementation of it.

use clavus_core::types::{Cue, CueStatus, Snapshot};
use clavus_relay::RelayState;
use clavus_sync::client::{SyncClient, SyncMsg};
use std::collections::HashMap;
use std::io::{Read, Write};
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

/// Start the real relay on a random port. Returns the bound address.
fn start_real_relay(state_dir: &TempDir) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let state_dir = state_dir.path().to_path_buf();

    thread::spawn(move || {
        let _ = clavus_relay::run_with_listener(listener, state_dir);
    });

    // Give the relay a moment to start accepting.
    thread::sleep(Duration::from_millis(50));
    addr
}

/// Send a raw framed payload to the relay and return the raw response bytes.
fn raw_exchange(addr: &str, payload: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = std::net::TcpStream::connect(addr)?;
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.write_all(&(payload.len() as u32).to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()?;

    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(len_buf) as usize;
    if n > 10 * 1024 * 1024 {
        return Err(std::io::Error::other("response too large"));
    }
    let mut data = vec![0u8; n];
    stream.read_exact(&mut data)?;
    Ok(data)
}

// ─── Real relay behaviour ──────────────────────────────────────────────────

#[test]
fn real_relay_responds_to_hello() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let client = SyncClient::connect(&addr).expect("connect");
    client.hello("tester", vec![]).expect("hello");

    let msg = client
        .recv_timeout(Duration::from_secs(3))
        .expect("relay should respond to hello");

    match msg {
        SyncMsg::Push { snapshots, cues } => {
            assert!(snapshots.is_empty(), "no snapshots pushed yet");
            assert!(cues.is_empty(), "no cues pushed yet");
        }
        other => panic!("unexpected reply: {:?}", other),
    }
    client.close();
}

#[test]
fn real_relay_stores_and_returns_pushed_snapshots() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let client = SyncClient::connect(&addr).expect("connect");
    client.hello("pusher", vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));

    client
        .send(SyncMsg::Push {
            snapshots: vec![make_snap("a".repeat(64).as_str(), None)],
            cues: vec![make_cue("cue-1")],
        })
        .expect("push");

    // A second client should be offered the stored snapshot.
    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello("puller", vec![]).expect("hello");
    let msg = client2
        .recv_timeout(Duration::from_secs(3))
        .expect("relay should push stored snapshots");

    match msg {
        SyncMsg::Push { snapshots, cues } => {
            assert_eq!(snapshots.len(), 1, "stored snapshot should be offered");
            assert_eq!(snapshots[0].hash, "a".repeat(64));
            assert_eq!(cues.len(), 1);
            assert_eq!(cues[0].id, "cue-1");
        }
        other => panic!("unexpected reply: {:?}", other),
    }
    client.close();
    client2.close();
}

#[test]
fn real_relay_survives_a_garbage_client() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    // Send a frame that is not valid JSON.
    let _ = raw_exchange(&addr, b"not json at all");

    // Send an absurd length prefix.
    {
        let mut stream = std::net::TcpStream::connect(&addr).unwrap();
        let _ = stream.write_all(&u32::MAX.to_be_bytes());
        let _ = stream.flush();
    }

    // Send a partial frame and hang up.
    {
        let mut stream = std::net::TcpStream::connect(&addr).unwrap();
        let _ = stream.write_all(&[0, 0, 0]);
        let _ = stream.flush();
    }

    // The relay must still be healthy for a well-behaved client.
    let client = SyncClient::connect(&addr).expect("relay should still accept");
    client.hello("after-garbage", vec![]).expect("hello");
    let msg = client
        .recv_timeout(Duration::from_secs(3))
        .expect("relay should still respond");
    assert!(matches!(msg, SyncMsg::Push { .. }));
    client.close();
}

#[test]
fn real_relay_handles_a_client_that_disappears() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    for _ in 0..5 {
        let mut stream = std::net::TcpStream::connect(&addr).unwrap();
        let _ = stream.write_all(&100u32.to_be_bytes());
        let _ = stream.write_all(b"partial payload that never completes");
        let _ = stream.flush();
        // Hang up mid-frame.
    }

    let client = SyncClient::connect(&addr).expect("relay should still accept");
    client.hello("after-drop", vec![]).expect("hello");
    assert!(client
        .recv_timeout(Duration::from_secs(3))
        .is_some());
    client.close();
}

#[test]
fn real_relay_persists_across_restart() {
    let tmp = TempDir::new().unwrap();

    // First relay instance: push a snapshot.
    {
        let addr = start_real_relay(&tmp);
        let client = SyncClient::connect(&addr).expect("connect");
        client.hello("writer", vec![]).expect("hello");
        let _ = client.recv_timeout(Duration::from_secs(3));
        client
            .send(SyncMsg::Push {
                snapshots: vec![make_snap(&"b".repeat(64), None)],
                cues: vec![],
            })
            .expect("push");
        // Give the relay a moment to persist.
        thread::sleep(Duration::from_millis(200));
        client.close();
    }

    // Second relay instance on the same state dir.
    {
        let addr = start_real_relay(&tmp);
        let client = SyncClient::connect(&addr).expect("connect");
        client.hello("reader", vec![]).expect("hello");
        let msg = client
            .recv_timeout(Duration::from_secs(3))
            .expect("relay should offer the persisted snapshot");
        match msg {
            SyncMsg::Push { snapshots, .. } => {
                assert_eq!(snapshots.len(), 1, "snapshot should have been persisted");
                assert_eq!(snapshots[0].hash, "b".repeat(64));
            }
            other => panic!("unexpected reply: {:?}", other),
        }
        client.close();
    }
}

#[test]
fn real_relay_reports_divergent_heads_as_a_conflict() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    // Client A pushes a snapshot with head H1.
    let a = SyncClient::connect(&addr).expect("connect");
    a.hello("client-a", vec![]).expect("hello");
    let _ = a.recv_timeout(Duration::from_secs(3));
    let h1 = "c".repeat(64);
    a.send(SyncMsg::Push {
        snapshots: vec![make_snap(&h1, None)],
        cues: vec![],
    })
    .expect("push");
    thread::sleep(Duration::from_millis(150));

    // Client B pushes a *different* snapshot with a different head.
    let b = SyncClient::connect(&addr).expect("connect");
    b.hello("client-b", vec![]).expect("hello");
    let _ = b.recv_timeout(Duration::from_secs(3));
    let h2 = "d".repeat(64);
    b.send(SyncMsg::Push {
        snapshots: vec![make_snap(&h2, None)],
        cues: vec![],
    })
    .expect("push");

    // The relay should notice the divergence.
    let mut saw_conflict = false;
    for _ in 0..10 {
        if let Some(SyncMsg::Conflict { .. }) = b.recv_timeout(Duration::from_millis(300)) {
            saw_conflict = true;
            break;
        }
    }
    assert!(saw_conflict, "divergent heads should produce a conflict");
    a.close();
    b.close();
}

#[test]
fn real_relay_ignores_a_duplicate_push() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let client = SyncClient::connect(&addr).expect("connect");
    client.hello("dupe", vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));

    let snap = make_snap(&"e".repeat(64), None);
    for _ in 0..3 {
        client
            .send(SyncMsg::Push {
                snapshots: vec![snap.clone()],
                cues: vec![],
            })
            .expect("push");
    }
    thread::sleep(Duration::from_millis(200));

    // The relay should still hold exactly one copy.
    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello("check", vec![]).expect("hello");
    match client2.recv_timeout(Duration::from_secs(3)).expect("reply") {
        SyncMsg::Push { snapshots, .. } => {
            assert_eq!(snapshots.len(), 1, "duplicate pushes must not duplicate");
        }
        other => panic!("unexpected reply: {:?}", other),
    }
    client.close();
    client2.close();
}

#[test]
fn relay_state_survives_a_corrupt_state_file() {
    // A corrupt snapshots.json must not stop the relay from starting.
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(state_dir.join("snapshots.json"), b"{ this is not valid json").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let sd = state_dir.clone();
    thread::spawn(move || {
        let _ = clavus_relay::run_with_listener(listener, sd);
    });
    thread::sleep(Duration::from_millis(100));

    let client = SyncClient::connect(&addr).expect("relay should start despite corrupt state");
    client.hello("after-corrupt", vec![]).expect("hello");
    assert!(client
        .recv_timeout(Duration::from_secs(3))
        .is_some());
    client.close();
}

#[test]
fn relay_state_lists_stored_snapshots() {
    let tmp = TempDir::new().unwrap();
    let mut state = RelayState::new(tmp.path().to_path_buf()).unwrap();
    let snap = make_snap(&"f".repeat(64), None);
    state.store(vec![snap.clone()], vec![]);

    let found = state.find_missing(&[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].hash, "f".repeat(64));

    // A client that already knows the head gets nothing.
    let known = vec!["f".repeat(64)];
    assert!(state.find_missing(&known).is_empty());
}
