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
        project_folder: String::new(),
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
    // the project "test.als" in the folder "tmp" — the name a client
    // uses on the wire, plus the folder that tells two projects with
    // the same file name apart.
    let found = state.find_missing("test.als::tmp", &[]);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].hash, "f".repeat(64));

    // A client that already knows the head gets nothing.
    let known = vec!["f".repeat(64)];
    assert!(state.find_missing("test.als::tmp", &known).is_empty());

    // A client on a different project is offered nothing.
    assert!(
        state.find_missing("some-other-project", &[]).is_empty(),
        "another project must not be offered this history"
    );

    // The client says which file it is working on, and the relay works
    // out which project that is.
    assert_eq!(
        state.resolve_lineage("test.als", "tmp", &[], None),
        Some("test.als::tmp".to_string())
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

// ─── The relay keeps the project bytes ────────────────────────────────────
//
// Regression: the relay only kept the metadata, so a collaborator could
// see that a snapshot existed but could not restore it on their own
// machine. The bytes travel with the snapshot.

#[test]
fn the_relay_stores_and_serves_the_project_bytes() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let project = "Track.als";
    let bytes = b"the project bytes";
    let hash = clavus_core::blob::BlobStore::sha256(bytes);

    let client = SyncClient::connect(&addr).expect("connect");
    client.hello(project, vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));

    client
        .push_blob(project, &hash, bytes)
        .expect("push the bytes");
    let msg = client.recv_timeout(Duration::from_secs(3)).expect("the relay confirms");
    assert!(
        matches!(msg, SyncMsg::BlobStored { .. }),
        "the relay must confirm the blob: {:?}",
        msg
    );

    // A second client asks for the bytes.
    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello(project, vec![]).expect("hello");
    let _ = client2.recv_timeout(Duration::from_secs(3));
    client2.request_blob(project, &hash).expect("request");
    let got = client2
        .recv_blob(project, &hash, Duration::from_secs(5))
        .expect("a blob");
    assert!(got.is_ok(), "the blob must arrive: {:?}", got);
    assert_eq!(got.unwrap(), bytes.to_vec());

    client.close();
    client2.close();
}

#[test]
fn the_relay_refuses_bytes_that_do_not_match_their_name() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let project = "Track.als";
    // The name says one thing, the bytes are another.
    let hash = "a".repeat(64);

    let client = SyncClient::connect(&addr).expect("connect");
    client.hello(project, vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));
    client
        .push_blob(project, &hash, b"not the bytes the name promises")
        .expect("push");

    // The relay must not confirm a blob it did not check.
    let confirmed = matches!(
        client.recv_timeout(Duration::from_secs(2)),
        Some(SyncMsg::BlobStored { .. })
    );
    assert!(!confirmed, "the relay must not store bytes under a false name");

    // And it must not serve them.
    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello(project, vec![]).expect("hello");
    let _ = client2.recv_timeout(Duration::from_secs(3));
    client2.request_blob(project, &hash).expect("request");
    match client2.recv_blob(project, &hash, Duration::from_secs(3)).expect("a reply") {
        Err(clavus_sync::client::BlobError::Missing) => {}
        other => panic!("the relay must not serve bytes it refused: {:?}", other),
    }

    client.close();
    client2.close();
}

#[test]
fn the_relay_refuses_a_blob_name_that_is_not_a_hash() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    let client = SyncClient::connect(&addr).expect("connect");
    client.hello("Track.als", vec![]).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));
    client
        .push_blob("Track.als", "../../etc/passwd", b"hello")
        .expect("push");

    // Nothing may be written outside the blob directory.
    assert!(
        !tmp.path().join("etc").exists(),
        "a blob name must never become a path"
    );
    client.close();
}

// ─── Two projects with the same file name stay apart ─────────────────────
//
// Regression: the relay filed a project under its file name alone, so
// `Album1/Track.als` and `Album2/Track.als` shared one bucket. A pull
// for one of them handed over the other's history and notes.

#[test]
fn the_relay_keeps_same_named_projects_apart() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    // Two projects that share a file name, in different folders.
    let one = make_snap_for("a".repeat(64).as_str(), None, "Album1/Track.als");
    let two = make_snap_for("b".repeat(64).as_str(), None, "Album2/Track.als");

    // The first client works on the first project.
    let client = SyncClient::connect(&addr).expect("connect");
    client
        .hello_in("Track.als", vec![], "Album1", None)
        .expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));
    client
        .send(SyncMsg::Push {
            snapshots: vec![one.clone()],
            cues: vec![make_cue_for("cue-one", "Track.als")],
        })
        .expect("push");

    // The second client works on the second project.
    let client2 = SyncClient::connect(&addr).expect("connect");
    client2
        .hello_in("Track.als", vec![], "Album2", None)
        .expect("hello");
    let _ = client2.recv_timeout(Duration::from_secs(3));
    client2
        .send(SyncMsg::Push {
            snapshots: vec![two.clone()],
            cues: vec![make_cue_for("cue-two", "Track.als")],
        })
        .expect("push");

    // A client in the first folder must only be offered the first
    // project.
    let client3 = SyncClient::connect(&addr).expect("connect");
    client3
        .hello_in("Track.als", vec![], "Album1", None)
        .expect("hello");
    let reply = client3
        .recv_timeout(Duration::from_secs(3))
        .expect("the relay must answer");
    match reply {
        SyncMsg::Push { snapshots, cues } => {
            assert_eq!(snapshots.len(), 1, "only the first project");
            assert_eq!(snapshots[0].hash, "a".repeat(64));
            assert_eq!(cues.len(), 1);
            assert_eq!(cues[0].id, "cue-one", "the first project's note");
        }
        other => panic!("unexpected reply: {:?}", other),
    }

    // And a client in the second folder only the second project.
    let client4 = SyncClient::connect(&addr).expect("connect");
    client4
        .hello_in("Track.als", vec![], "Album2", None)
        .expect("hello");
    let reply = client4
        .recv_timeout(Duration::from_secs(3))
        .expect("the relay must answer");
    match reply {
        SyncMsg::Push { snapshots, cues } => {
            assert_eq!(snapshots.len(), 1, "only the second project");
            assert_eq!(snapshots[0].hash, "b".repeat(64));
            assert_eq!(cues.len(), 1);
            assert_eq!(cues[0].id, "cue-two", "the second project's note");
        }
        other => panic!("unexpected reply: {:?}", other),
    }

    client.close();
    client2.close();
    client3.close();
    client4.close();
}

#[test]
fn an_ambiguous_name_is_served_to_nobody() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    // Two projects share a file name.
    let client = SyncClient::connect(&addr).expect("connect");
    client
        .hello_in("Track.als", vec![], "Album1", None)
        .expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));
    client
        .send(SyncMsg::Push {
            snapshots: vec![make_snap_for("a".repeat(64).as_str(), None, "Album1/Track.als")],
            cues: vec![],
        })
        .expect("push");

    let client2 = SyncClient::connect(&addr).expect("connect");
    client2
        .hello_in("Track.als", vec![], "Album2", None)
        .expect("hello");
    let _ = client2.recv_timeout(Duration::from_secs(3));
    client2
        .send(SyncMsg::Push {
            snapshots: vec![make_snap_for("b".repeat(64).as_str(), None, "Album2/Track.als")],
            cues: vec![],
        })
        .expect("push");

    // A client that names the file but sits in neither folder cannot be
    // told apart from the two. It gets nothing rather than the wrong
    // project's history.
    let client3 = SyncClient::connect(&addr).expect("connect");
    client3.hello_in("Track.als", vec![], "", None).expect("hello");
    let reply = client3
        .recv_timeout(Duration::from_secs(3))
        .expect("the relay must answer");
    match reply {
        SyncMsg::Push { snapshots, cues } => {
            assert!(snapshots.is_empty(), "no project may be guessed at");
            assert!(cues.is_empty(), "no notes may be guessed at");
        }
        other => panic!("unexpected reply: {:?}", other),
    }

    client.close();
    client2.close();
    client3.close();
}

#[test]
fn a_refused_push_is_not_stored_by_the_relay() {
    let tmp = TempDir::new().unwrap();
    let addr = start_real_relay(&tmp);

    // The first chain is accepted.
    let client = SyncClient::connect(&addr).expect("connect");
    client.hello_in("Split.als", vec![], "Album", None).expect("hello");
    let _ = client.recv_timeout(Duration::from_secs(3));
    client
        .send(SyncMsg::Push {
            snapshots: vec![make_snap_for("a".repeat(64).as_str(), None, "Album/Split.als")],
            cues: vec![],
        })
        .expect("push");
    let _ = client.recv_timeout(Duration::from_secs(3));

    // A different chain with the same parent is refused.
    let client2 = SyncClient::connect(&addr).expect("connect");
    client2.hello_in("Split.als", vec![], "Album", None).expect("hello");
    let _ = client2.recv_timeout(Duration::from_secs(3));
    client2
        .send(SyncMsg::Push {
            snapshots: vec![make_snap_for("b".repeat(64).as_str(), None, "Album/Split.als")],
            cues: vec![],
        })
        .expect("push");

    // The relay must say so.
    let mut saw_conflict = false;
    for _ in 0..4 {
        match client2.recv_timeout(Duration::from_secs(2)) {
            Some(SyncMsg::Conflict { .. }) => {
                saw_conflict = true;
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert!(saw_conflict, "the relay must report the conflict");

    // And it must not have kept the refused snapshot.
    let snaps: Vec<serde_json::Value> = {
        let data = std::fs::read_to_string(tmp.path().join("snapshots.json")).expect("snapshots");
        serde_json::from_str(&data).expect("readable")
    };
    let hashes: Vec<String> = snaps
        .iter()
        .filter_map(|s| s.get("hash").and_then(|m| m.as_str()).map(String::from))
        .collect();
    assert!(
        !hashes.contains(&"b".repeat(64)),
        "the refused snapshot must not be stored: {:?}",
        hashes
    );
    assert!(
        hashes.contains(&"a".repeat(64)),
        "the accepted snapshot must be stored: {:?}",
        hashes
    );

    client.close();
    client2.close();
}

#[test]
fn an_older_relay_keeps_its_history_after_the_upgrade() {
    let tmp = TempDir::new().unwrap();

    // A relay state file written before projects were told apart by
    // their folder: the head and the notes are filed under the bare
    // file name.
    let snap = make_snap_for("a".repeat(64).as_str(), None, "Album1/Track.als");
    std::fs::write(
        tmp.path().join("snapshots.json"),
        serde_json::to_string(&vec![snap]).expect("write snapshots"),
    )
    .expect("write snapshots");
    std::fs::write(
        tmp.path().join("heads.json"),
        serde_json::to_string(&serde_json::json!({ "Track.als": "a".repeat(64) }))
            .expect("write heads"),
    )
    .expect("write heads");
    let mut cue = make_cue_for("old-cue", "Track.als");
    cue.id = "old-cue".to_string();
    std::fs::write(
        tmp.path().join("cues.json"),
        serde_json::to_string(&serde_json::json!({ "Track.als": vec![cue] }))
            .expect("write cues"),
    )
    .expect("write cues");

    // The relay must still serve the history and the notes.
    let state = clavus_relay::relay::RelayState::new(tmp.path().to_path_buf()).expect("state");
    let lineage = state.resolve_lineage("Track.als", "Album1", &[], None);
    assert_eq!(
        lineage,
        Some("Track.als::Album1".to_string()),
        "the history must still be found"
    );
    let missing = state.find_missing("Track.als::Album1", &[]);
    assert_eq!(missing.len(), 1, "the snapshot must still be there");
    let cues = state.cues.get("Track.als::Album1").expect("the notes");
    assert_eq!(cues.len(), 1, "the notes must still be there");
    assert_eq!(cues[0].id, "old-cue");
}
