//! Regression tests for the CLI ergonomics wave's findings.
//!
//! Each test here reproduces a defect an AI beta agent found in the
//! compiled binary. They drive the real command implementations, real
//! files and a real relay — nothing is re-implemented on the test side.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clavus_cli::{
    cmd_audit, cmd_cue, cmd_cues, cmd_log, cmd_pull, cmd_push, cmd_restore,
    cmd_snapshot,
};
use common::{als_version, run, write_als};
use tempfile::TempDir;

struct Workspace {
    _tmp: TempDir,
    root: PathBuf,
    dir: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path().join(".clavus");
        let dir = tmp.path().join("projects");
        fs::create_dir_all(&dir).expect("projects dir");
        Workspace {
            _tmp: tmp,
            root,
            dir,
        }
    }

    fn als_in(&self, folder: &str, name: &str) -> PathBuf {
        let dir = self.dir.join(folder);
        fs::create_dir_all(&dir).expect("folder");
        let path = dir.join(name);
        write_als(&path, &als_version(name, 128));
        path
    }

    fn revise(&self, path: &Path, bpm: u32) {
        write_als(path, &als_version("changed", bpm));
    }
}

/// A workspace whose projects live under the home directory.
///
/// The folder path of a project — the second half of its identity on
/// the wire — is the path below the home directory, so a test that
/// wants the behaviour a user's projects have has to put them there.
struct HomeWorkspace {
    dir: PathBuf,
    root: PathBuf,
    home: PathBuf,
}

impl HomeWorkspace {
    fn new() -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let unique = format!(
            ".clavus-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let base = home.join(unique);
        let dir = base.join("notes");
        fs::create_dir_all(&dir).expect("projects dir");
        HomeWorkspace {
            dir,
            root: base.join(".clavus"),
            home: base,
        }
    }

    fn als_in(&self, folder: &str, name: &str) -> PathBuf {
        let dir = self.dir.join(folder);
        fs::create_dir_all(&dir).expect("folder");
        let path = dir.join(name);
        write_als(&path, &als_version(name, 128));
        path
    }
}

impl Drop for HomeWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn start_relay() -> (String, TempDir, std::thread::JoinHandle<()>) {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let dir = TempDir::new().expect("relay dir");
    let dir_path = dir.path().to_path_buf();
    let handle = std::thread::spawn(move || {
        let _ = clavus_relay::run_with_listener(listener, dir_path);
    });
    for _ in 0..200 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    (addr, dir, handle)
}

// ─── B1: two projects whose folders share a name are two projects ────────
//
// Regression: the storage key was built from the *name* of the
// project's folder, so `notes/p1/studio/Song.als` and
// `notes/p2/studio/Song.als` shared one chain, one cue list and one
// relay identity.

#[test]
fn two_projects_in_folders_of_the_same_name_keep_their_own_history() {
    let ws = Workspace::new();
    let p1 = ws.als_in("p1/studio", "Song.als");
    let p2 = ws.als_in("p2/studio", "Song.als");
    ws.revise(&p2, 140);

    let (out, result) = run(|o, e| {
        cmd_snapshot(&p1, &Some("P1 song".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot p1: {}", out.all());
    let (out, result) = run(|o, e| {
        cmd_snapshot(&p2, &Some("P2 song".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot p2: {}", out.all());
    // A snapshot of an unchanged file is not a new snapshot, so the
    // second project's own change is what makes it a snapshot at all.
    assert!(
        !out.stdout.contains("already in this project's history"),
        "the second project must not be told it was snapshotted before: {}",
        out.all()
    );

    // Each log holds only its own snapshot.
    let (out, result) = run(|o, e| cmd_log(&p1, &ws.root, o, e));
    assert!(result.is_ok(), "log p1: {}", out.all());
    assert!(out.stdout.contains("P1 song"), "p1's log: {}", out.all());
    assert!(
        !out.stdout.contains("P2 song"),
        "p2's snapshot must not be in p1's log: {}",
        out.all()
    );

    let (out, result) = run(|o, e| cmd_log(&p2, &ws.root, o, e));
    assert!(result.is_ok(), "log p2: {}", out.all());
    assert!(out.stdout.contains("P2 song"), "p2's log: {}", out.all());
    assert!(
        !out.stdout.contains("P1 song"),
        "p1's snapshot must not be in p2's log: {}",
        out.all()
    );

    // And the notes do not cross over either.
    let (out, result) = run(|o, e| {
        cmd_cue(&p1, "a note for p1", "1.1.1", None, None, &ws.root, o, e)
    });
    assert!(result.is_ok(), "cue p1: {}", out.all());

    let (out, result) = run(|o, e| cmd_cues(&p2, None, &ws.root, o, e));
    assert!(result.is_ok(), "cues p2: {}", out.all());
    assert!(
        !out.stdout.contains("a note for p1"),
        "p1's note must not appear under p2: {}",
        out.all()
    );
}

#[test]
fn two_projects_in_folders_of_the_same_name_keep_their_own_relay_identity() {
    let ws = HomeWorkspace::new();
    let p1 = ws.als_in("p1/studio", "Song.als");
    let p2 = ws.als_in("p2/studio", "Song.als");
    // The two projects hold different music: they are two projects,
    // not two copies of one.
    write_als(&p2, &als_version("changed", 140));

    for (project, message) in [(&p1, "P1 song"), (&p2, "P2 song")] {
        let (out, result) = run(|o, e| {
            cmd_snapshot(project, &Some(message.to_string()), false, &ws.root, o, e)
        });
        assert!(result.is_ok(), "snapshot {}: {}", message, out.all());
    }

    let (addr, relay_dir, relay) = start_relay();
    for project in [&p1, &p2] {
        let (out, result) = run(|o, e| cmd_push(&addr, project, &ws.root, o, e));
        assert!(result.is_ok(), "push: {}", out.all());
    }

    // The relay must hold two projects, not one.
    let heads: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(relay_dir.path().join("heads.json")).expect("the relay's heads"),
    )
    .expect("the heads are readable");
    let keys = heads.as_object().expect("the heads are a map").len();
    assert_eq!(
        keys, 2,
        "the relay must keep two projects apart: {:?}",
        heads
    );
    drop(relay);
}

// ─── B3: the audit's report belongs on stdout ────────────────────────────
//
// Regression: the missing/outside report went to stderr, so a script
// that reads stdout found an empty file exactly when there was
// something to say.

#[test]
fn the_audit_report_is_on_stdout() {
    let tmp = TempDir::new().expect("temp dir");
    let missing = tmp.path().join("gone.wav");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="Vox" />
    <DeviceChain />
    <SampleRef><Path Value="{}" /></SampleRef>
  </AudioTrack>
</LiveSet>"#,
        missing.to_string_lossy()
    );
    let als = tmp.path().join("Beta.als");
    write_als(&als, &xml);

    let (out, result) = run(|o, e| cmd_audit(&als, false, o, e));
    assert!(result.is_ok(), "audit: {}", out.all());
    assert!(
        out.stdout.contains("missing"),
        "the report must be on stdout: {}",
        out.all()
    );
    assert!(
        out.stderr.is_empty(),
        "the report must not be on stderr: {}",
        out.stderr
    );
}

// ─── B5: a note is signed by somebody ────────────────────────────────────

#[test]
fn a_note_is_never_signed_by_the_word_cli() {
    // A machine with no user name — a container, a cron job — still
    // needs a name. "cli" says nothing and looks like a person.
    let author = clavus_cli::default_author();
    assert_ne!(
        author, "cli",
        "the fallback author must name a machine or a person"
    );
}

// ─── B6: the pull must not contradict itself ─────────────────────────────

#[test]
fn the_pull_summary_does_not_contradict_the_cue_import() {
    let ws = Workspace::new();
    let als = ws.als_in("Album", "Track.als");

    let (out, result) = run(|o, e| {
        cmd_snapshot(&als, &Some("first".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());

    let (addr, _dir, relay) = start_relay();
    let (out, result) = run(|o, e| cmd_push(&addr, &als, &ws.root, o, e));
    assert!(result.is_ok(), "push: {}", out.all());

    // A note that is already on the relay: it is pulled back, and the
    // pull must not claim that there was nothing new.
    let (out, result) = run(|o, e| {
        cmd_cue(&als, "shared note", "1.1.1", None, None, &ws.root, o, e)
    });
    assert!(result.is_ok(), "cue: {}", out.all());
    let (out, result) = run(|o, e| cmd_push(&addr, &als, &ws.root, o, e));
    assert!(result.is_ok(), "push the note: {}", out.all());

    // A second machine with the same project pulls.
    let ws_b = Workspace::new();
    let als_b = ws_b.als_in("Album", "Track.als");
    let (out, result) = run(|o, e| cmd_pull(&addr, &als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());
    let all = out.all();
    assert!(
        !all.contains("the relay had nothing new for this project\n✓ Imported"),
        "the summary must not say there was nothing new and then import one: {}",
        all
    );

    drop(relay);
}

// ─── B8: a cue shows the track it belongs to ─────────────────────────────

#[test]
fn the_cue_list_shows_the_track() {
    let ws = Workspace::new();
    let als = ws.als_in("Album", "Track.als");

    let (out, result) = run(|o, e| {
        cmd_cue(
            &als,
            "drop bass at bar 17",
            "17.1.0",
            Some("Bass"),
            None,
            &ws.root,
            o,
            e,
        )
    });
    assert!(result.is_ok(), "cue: {}", out.all());

    let (out, result) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
    assert!(result.is_ok(), "cues: {}", out.all());
    assert!(
        out.stdout.contains("Bass"),
        "the track must be shown: {}",
        out.all()
    );
}

// ─── B9: a folder of projects is not a project ───────────────────────────

#[test]
fn a_directory_is_not_a_project() {
    let ws = Workspace::new();
    let dir = ws.dir.join("Album");
    fs::create_dir_all(&dir).expect("folder");

    let (_out, result) = run(|o, e| cmd_log(&dir, &ws.root, o, e));
    let error = result.expect_err("log of a folder must fail").to_string();
    assert!(
        error.to_lowercase().contains("directory"),
        "the message must say it is a directory: {}",
        error
    );
    assert!(
        !error.contains("No snapshots yet"),
        "a folder is not an empty project: {}",
        error
    );

    let (_out, result) = run(|o, e| cmd_restore(&dir, "HEAD", &ws.root, o, e));
    let error = result.expect_err("restore of a folder must fail").to_string();
    assert!(
        error.to_lowercase().contains("directory"),
        "the message must say it is a directory: {}",
        error
    );
}

// ─── A8: a colon in a name cannot forge another project's key ───────────

#[test]
fn a_colon_in_a_name_cannot_forge_another_project() {
    let ws = Workspace::new();
    // "A.als" in the folder "B::C", and "A.als::B" in the folder "C".
    let one = ws.als_in("B::C", "A.als");
    let two = ws.als_in("C", "A.als::B");
    write_als(&two, &als_version("changed", 140));

    for project in [&one, &two] {
        let (out, result) = run(|o, e| {
            cmd_snapshot(project, &Some("a version".to_string()), false, &ws.root, o, e)
        });
        assert!(result.is_ok(), "snapshot: {}", out.all());
    }

    let (addr, relay_dir, relay) = start_relay();
    for project in [&one, &two] {
        let (out, result) = run(|o, e| cmd_push(&addr, project, &ws.root, o, e));
        assert!(
            result.is_ok(),
            "the push must not be refused: {}",
            out.all()
        );
    }

    let heads: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(relay_dir.path().join("heads.json")).expect("the relay's heads"),
    )
    .expect("the heads are readable");
    assert_eq!(
        heads.as_object().expect("the heads are a map").len(),
        2,
        "the relay must keep two projects apart: {:?}",
        heads
    );
    drop(relay);
}

// ─── A9: a relay that re-offers what we have is not a divergence ────────

#[test]
fn a_relay_that_re_offers_our_head_is_not_a_divergence() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let ws = Workspace::new();
    let als = ws.als_in("Album", "Track.als");
    let (out, result) = run(|o, e| {
        cmd_snapshot(&als, &Some("first".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());

    // The hash of the snapshot the client already has.
    let head = {
        let (out, result) = run(|o, e| cmd_log(&als, &ws.root, o, e));
        assert!(result.is_ok(), "log: {}", out.all());
        out.stdout
            .split_whitespace()
            .find(|w| w.len() == 8 && w.chars().all(|c| c.is_ascii_hexdigit()))
            .expect("a hash in the log")
            .to_string()
    };

    // A relay that always offers the snapshot the client already has.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let head_hash = head.clone();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Read the client's hello.
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).is_ok() {
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; len];
                let _ = stream.read_exact(&mut body);
            }
            // Offer the snapshot the client already has.
            let snapshot = serde_json::json!({
                "hash": head_hash,
                "parent": null,
                "timestamp": 1.0,
                "message": "already yours",
                "track_count": 1,
                "bpm": 120.0,
                "tags": [],
                "notes": "",
                "sample_hashes": [],
                "sample_paths": {},
                "project_path": "",
                "project_folder": "",
            });
            let frame = serde_json::json!({
                "type": "push",
                "snapshots": [snapshot],
                "cues": [],
            });
            let payload = frame.to_string();
            let _ = stream.write_all(&(payload.len() as u32).to_be_bytes());
            let _ = stream.write_all(payload.as_bytes());
            let synced = serde_json::json!({ "type": "synced" }).to_string();
            let _ = stream.write_all(&(synced.len() as u32).to_be_bytes());
            let _ = stream.write_all(synced.as_bytes());
            let _ = stream.flush();
            std::thread::sleep(Duration::from_millis(300));
        }
    });
    std::thread::sleep(Duration::from_millis(150));

    let (out, result) = run(|o, e| cmd_pull(&addr, &als, &ws.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());
    assert!(
        !out.all().contains("does not extend yours"),
        "the relay offered the snapshot we already have — nothing has diverged: {}",
        out.all()
    );
}
