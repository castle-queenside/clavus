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
    make_snap_for(hash, parent, "test.als")
}

/// A snapshot belonging to `project`.
fn make_snap_for(hash: &str, parent: Option<&str>, project: &str) -> Snapshot {
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
        project_path: format!("/tmp/{}", project),
    }
}

fn make_cue(id: &str) -> Cue {
    make_cue_for(id, "demo.als")
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

/// Start the real relay on a random port. Returns the bound address.
fn start_real_relay(state_dir: &TempDir) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let state_dir = state_dir.path().to_path_buf();

    thread::spawn(move || {
        let _ = clavus_relay::run_with_listener(listener, state_dir);
    });

    // Wait until the relay really accepts connections. A fixed sleep
    // was not enough when the machine was busy, and the test then failed
    // for a reason that had nothing to do with the code.
    for _ in 0..200 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
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

    // Both clients work on the same project, so both name it in Hello.
    let client = SyncClient::connect(&addr).expect("connect");
    client.hello("test.als", vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));

    client
        .send(SyncMsg::Push {
            snapshots: vec![make_snap("a".repeat(64).as_str(), None)],
            cues: vec![make_cue_for("cue-1", "test.als")],
        })
        .expect("push");

    // A second client should be offered the stored snapshot.
    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello("test.als", vec![]).expect("hello");
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
        client.hello("test.als", vec![]).expect("hello");
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
        client.hello("test.als", vec![]).expect("hello");
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
    client.hello("test.als", vec![]).expect("hello");
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
    client2.hello("test.als", vec![]).expect("hello");
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
    for _ in 0..200 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

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

    // The snapshot's project path is "/tmp/test.als", so it belongs to
    // the project "test.als" — the name a client uses on the wire.
    let found = state.find_missing("test.als", &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].hash, "f".repeat(64));

    // A client that already knows the head gets nothing.
    let known = vec!["f".repeat(64)];
    assert!(state.find_missing("test.als", &known).is_empty());

    // A client on a different project is offered nothing.
    assert!(
        state.find_missing("some-other-project", &[]).is_empty(),
        "another project must not be offered this history"
    );
}

#[test]
fn real_relay_keeps_each_projects_cues_to_itself() {
    // Regression: the relay grouped cues by track name, so two projects
    // with a track called "Drums" mixed their notes together.
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    // Project "alpha" pushes a cue.
    let alpha = SyncClient::connect(&addr).expect("connect");
    alpha.hello("alpha.als", vec![]).expect("hello");
    let _ = alpha.recv_timeout(Duration::from_secs(2));
    alpha
        .push(vec![], vec![make_cue_for("alpha-cue", "alpha.als")])
        .expect("push");
    let _ = alpha.recv_timeout(Duration::from_secs(2));

    // Project "beta" pushes a cue with the same track name.
    let beta = SyncClient::connect(&addr).expect("connect");
    beta.hello("beta.als", vec![]).expect("hello");
    let _ = beta.recv_timeout(Duration::from_secs(2));
    beta
        .push(vec![], vec![make_cue_for("beta-cue", "beta.als")])
        .expect("push");
    let _ = beta.recv_timeout(Duration::from_secs(2));

    // A client on "alpha" sees only alpha's cue.
    let alpha2 = SyncClient::connect(&addr).expect("connect");
    alpha2.hello("alpha.als", vec![]).expect("hello");
    let msg = alpha2.recv_timeout(Duration::from_secs(3));
    match msg {
        Some(SyncMsg::Push { cues, .. }) => {
            let ids: Vec<&str> = cues.iter().map(|c| c.id.as_str()).collect();
            assert!(
                ids.contains(&"alpha-cue"),
                "alpha should see its own cue: {:?}",
                ids
            );
            assert!(
                !ids.contains(&"beta-cue"),
                "alpha must not see beta's cue: {:?}",
                ids
            );
        }
        other => panic!("unexpected reply: {:?}", other),
    }

    alpha.close();
    beta.close();
    alpha2.close();
}

#[test]
fn real_relay_does_not_duplicate_a_repeatedly_pushed_cue() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let client = SyncClient::connect(&addr).expect("connect");
    client.hello("test.als", vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(2));

    for _ in 0..3 {
        client
            .push(vec![], vec![make_cue_for("cue-1", "test.als")])
            .expect("push");
        let _ = client.recv_timeout(Duration::from_secs(2));
    }

    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello("test.als", vec![]).expect("hello");
    let msg = client2.recv_timeout(Duration::from_secs(3));
    match msg {
        Some(SyncMsg::Push { cues, .. }) => {
            let count = cues.iter().filter(|c| c.id == "cue-1").count();
            assert_eq!(count, 1, "the cue should appear once: {:?}", cues);
        }
        other => panic!("unexpected reply: {:?}", other),
    }
    client.close();
    client2.close();
}

#[test]
fn real_relay_does_not_offer_one_projects_snapshots_to_another() {
    // Regression: the relay offered every snapshot it held to every
    // client, so pulling one project pulled another project's history
    // into the wrong store.
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    // Project "alpha" pushes a snapshot.
    let alpha = SyncClient::connect(&addr).expect("connect");
    alpha.hello("alpha.als", vec![]).expect("hello");
    let _ = alpha.recv_timeout(Duration::from_secs(2));
    alpha
        .push(
            vec![make_snap_for(&"a".repeat(64), None, "alpha.als")],
            vec![],
        )
        .expect("push");
    let _ = alpha.recv_timeout(Duration::from_secs(2));

    // A client on "beta" is offered nothing.
    let beta = SyncClient::connect(&addr).expect("connect");
    beta.hello("beta.als", vec![]).expect("hello");
    let msg = beta.recv_timeout(Duration::from_secs(3));
    match msg {
        Some(SyncMsg::Push { snapshots, cues }) => {
            assert!(
                snapshots.is_empty(),
                "beta must not be offered alpha's snapshots: {:?}",
                snapshots
            );
            assert!(cues.is_empty(), "beta must not be offered alpha's cues");
        }
        other => panic!("unexpected reply: {:?}", other),
    }

    // A client on "alpha" is offered alpha's snapshot.
    let alpha2 = SyncClient::connect(&addr).expect("connect");
    alpha2.hello("alpha.als", vec![]).expect("hello");
    let msg = alpha2.recv_timeout(Duration::from_secs(3));
    match msg {
        Some(SyncMsg::Push { snapshots, .. }) => {
            assert_eq!(snapshots.len(), 1, "alpha should get its snapshot");
            assert_eq!(snapshots[0].hash, "a".repeat(64));
        }
        other => panic!("unexpected reply: {:?}", other),
    }

    alpha.close();
    beta.close();
    alpha2.close();
}

// ─── Chaos: a hostile client must not take the relay down ─────────────────

#[test]
fn the_relay_survives_a_client_that_sends_garbage() {
    // A client that sends junk must not stop the relay from serving
    // anybody else.
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    // Three kinds of abuse: an enormous length prefix, invalid JSON, and
    // a length prefix that is not followed by a body.
    for abuse in 0..3 {
        let mut sock = std::net::TcpStream::connect(&addr).expect("connect");
        match abuse {
            0 => {
                // Claims a huge payload.
                let _ = sock.write_all(&u32::MAX.to_be_bytes());
            }
            1 => {
                // Sends a valid frame with invalid JSON.
                let payload = b"{{{not json";
                let _ = sock.write_all(&(payload.len() as u32).to_be_bytes());
                let _ = sock.write_all(payload);
            }
            _ => {
                // Claims a body and then hangs up.
                let _ = sock.write_all(&64u32.to_be_bytes());
            }
        }
        drop(sock);
    }

    // The relay is still alive and serving.
    let client = SyncClient::connect(&addr).expect("connect");
    client.hello("test.als", vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));
    client
        .push(vec![make_snap(&"a".repeat(64), None)], vec![])
        .expect("push");
    let _ = client.recv_timeout(Duration::from_secs(3));

    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello("test.als", vec![]).expect("hello");
    match client2.recv_timeout(Duration::from_secs(3)) {
        Some(SyncMsg::Push { snapshots, .. }) => {
            assert_eq!(snapshots.len(), 1, "the relay should still serve");
        }
        other => panic!("unexpected: {:?}", other),
    }
    client.close();
    client2.close();
}

#[test]
fn the_relay_ignores_a_snapshot_with_a_short_hash() {
    // A hash is a SHA-256 digest. A shorter one could be stored on the
    // relay but could never be filed by a client, so it is refused.
    let tmp = TempDir::new().unwrap();
    let mut state = RelayState::new(tmp.path().to_path_buf()).unwrap();
    state.store(vec![make_snap("abc", None)], vec![]);
    assert!(
        state.snapshots.is_empty(),
        "a short hash must not be stored: {:?}",
        state.snapshots.keys().collect::<Vec<_>>()
    );
}

#[test]
fn the_relay_ignores_a_cue_with_a_bad_id() {
    let tmp = TempDir::new().unwrap();
    let mut state = RelayState::new(tmp.path().to_path_buf()).unwrap();
    state.store(
        vec![],
        vec![make_cue_for("../../escaped", "test.als"), make_cue_for("good", "test.als")],
    );
    let cues = state.cues.get("test.als").cloned().unwrap_or_default();
    assert_eq!(cues.len(), 1, "only the well-formed cue is kept: {:?}", cues);
    assert_eq!(cues[0].id, "good");
}

// ─── The chain order survives a restart ───────────────────────────────────
//
// Regression: the relay rebuilt its per-project index by iterating a
// hash map, whose order is random for every process. After a restart
// the same project came back in a different order each time, and a
// pull installed an arbitrary snapshot as the head.

#[test]
fn the_chain_order_survives_a_relay_restart() {
    let tmp = TempDir::new().unwrap();

    // Three snapshots in a chain: a → b → c.
    let a = "a".repeat(64);
    let b = "b".repeat(64);
    let c = "c".repeat(64);

    {
        let addr = start_real_relay(&tmp);
        let client = SyncClient::connect(&addr).expect("connect");
        client.hello("chain.als", vec![]).expect("hello");
        let _ = client.recv_timeout(Duration::from_secs(3));
        for (hash, parent) in [
            (a.as_str(), None),
            (b.as_str(), Some(a.as_str())),
            (c.as_str(), Some(b.as_str())),
        ] {
            client
                .send(SyncMsg::Push {
                    snapshots: vec![make_snap_for(hash, parent, "chain.als")],
                    cues: vec![],
                })
                .expect("push");
        }
        thread::sleep(Duration::from_millis(200));
        client.close();
    }

    // After a restart the order must be the chain order, every time.
    for _ in 0..5 {
        let addr = start_real_relay(&tmp);
        let client = SyncClient::connect(&addr).expect("connect");
        client.hello("chain.als", vec![]).expect("hello");
        let msg = client
            .recv_timeout(Duration::from_secs(3))
            .expect("the relay should offer the chain");
        match msg {
            SyncMsg::Push { snapshots, .. } => {
                let hashes: Vec<String> = snapshots.iter().map(|s| s.hash.clone()).collect();
                assert_eq!(
                    hashes,
                    vec![a.clone(), b.clone(), c.clone()],
                    "the chain must come back oldest first"
                );
            }
            other => panic!("unexpected reply: {:?}", other),
        }
        client.close();
    }
}

// ─── A cue-only push reaches the relay ───────────────────────────────────
//
// Regression: a push returned early when the project had no snapshots,
// so a collaborator who only left notes never sent them. The note
// silently never left the machine.

#[test]
fn a_cue_only_push_reaches_the_relay() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let client = SyncClient::connect(&addr).expect("connect");
    client.hello("notes.als", vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));
    client
        .send(SyncMsg::Push {
            snapshots: vec![],
            cues: vec![make_cue_for("note-1", "notes.als")],
        })
        .expect("push");
    thread::sleep(Duration::from_millis(150));

    // A second client must see the cue.
    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello("notes.als", vec![]).expect("hello");
    let msg = client2
        .recv_timeout(Duration::from_secs(3))
        .expect("the relay should offer the cue");
    match msg {
        SyncMsg::Push { cues, .. } => {
            assert_eq!(cues.len(), 1, "the cue must have been stored");
            assert_eq!(cues[0].id, "note-1");
        }
        other => panic!("unexpected reply: {:?}", other),
    }
    client.close();
    client2.close();
}
