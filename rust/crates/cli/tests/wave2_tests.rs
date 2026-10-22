//! Regression tests for the second beta wave's findings.
//!
//! Each test here reproduces a defect an AI beta agent found in the
//! compiled binary. They drive the real command implementations, a real
//! relay and real files — nothing is re-implemented on the test side.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clavus_cli::{
    cmd_cue, cmd_cues, cmd_log, cmd_pull, cmd_push, cmd_restore, cmd_snapshot, cmd_status,
};
use tempfile::TempDir;

// ─── Helpers ──────────────────────────────────────────────────────────────

struct Output {
    stdout: String,
    stderr: String,
}

impl Output {
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

fn run<F>(f: F) -> (Output, anyhow::Result<()>)
where
    F: FnOnce(&mut dyn std::io::Write, &mut dyn std::io::Write) -> anyhow::Result<()>,
{
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = f(&mut out, &mut err);
    (
        Output {
            stdout: String::from_utf8_lossy(&out).to_string(),
            stderr: String::from_utf8_lossy(&err).to_string(),
        },
        result,
    )
}

fn als_xml(name: &str) -> String {
    als_version(name, 128)
}

/// A set whose content differs from version to version.
///
/// A snapshot is a hash of the project's bytes, so a second snapshot of
/// an unchanged file is not a new snapshot at all. The tests therefore
/// change the tempo to make a new version.
fn als_version(name: &str, bpm: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tempo Value="{}" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack><Name Value="{}" /><DeviceChain /></AudioTrack>
</LiveSet>"#,
        bpm, name
    )
}

fn write_als(path: &Path, xml: &str) {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let file = fs::File::create(path).expect("create als");
    let mut enc = GzEncoder::new(file, Compression::default());
    use std::io::Write;
    enc.write_all(xml.as_bytes()).expect("write als");
    enc.finish().expect("finish als");
}

struct Workspace {
    _tmp: TempDir,
    root: PathBuf,
    dir: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path().join(".clavus");
        // The projects live one folder below a named root, so the
        // project's identity — the path of its folder — is the same on
        // every machine. A project outside the home keeps the last two
        // folders of its path, and the temporary directory's own name
        // would otherwise become part of it.
        let dir = tmp.path().join("clavus").join("projects");
        fs::create_dir_all(&dir).expect("projects dir");
        Workspace {
            _tmp: tmp,
            root,
            dir,
        }
    }

    /// A project file in a named folder.
    fn als_in(&self, folder: &str, name: &str) -> PathBuf {
        let dir = self.dir.join(folder);
        fs::create_dir_all(&dir).expect("folder");
        let path = dir.join(name);
        write_als(&path, &als_xml(name));
        path
    }

    fn als(&self, name: &str) -> PathBuf {
        let path = self.dir.join(name);
        write_als(&path, &als_xml(name));
        path
    }

    /// Change the project so the next snapshot is a new one.
    fn revise(&self, path: &Path, bpm: u32) {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        write_als(path, &als_version(&name, bpm));
    }
}

/// Start a real relay on a free port.
///
/// The state directory is handed back so a test can look at what the
/// relay actually kept.
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

// ─── B1: two projects that share a file name stay apart ──────────────────
//
// Regression: the relay filed a project under its file name alone, so
// `Album1/Track.als` and `Album2/Track.als` shared one bucket. A pull
// for one of them handed over the other's snapshots and notes.

#[test]
fn two_projects_with_the_same_file_name_do_not_mix() {
    let (addr, _dir, relay) = start_relay();

    // ── Machine A: two projects that share a file name ──
    let ws_a = Workspace::new();
    let album1 = ws_a.als_in("Album1", "Track.als");
    let album2 = ws_a.als_in("Album2", "Track.als");
    // The two albums hold different music — they are two projects,
    // not two copies of one.
    ws_a.revise(&album2, 140);

    let (out, result) = run(|o, e| {
        cmd_snapshot(&album1, &Some("album one v1".to_string()), false, &ws_a.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());
    let (out, result) = run(|o, e| {
        cmd_snapshot(&album2, &Some("album two v1".to_string()), false, &ws_a.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());

    // A note on the first project.
    let (out, result) = run(|o, e| {
        cmd_cue(
            &album1,
            "a note for album one",
            "1.1.1",
            None,
            None,
            &ws_a.root,
            o,
            e,
        )
    });
    assert!(result.is_ok(), "cue: {}", out.all());

    let (out, result) = run(|o, e| cmd_push(&addr, &album1, &ws_a.root, o, e));
    assert!(result.is_ok(), "push album one: {}", out.all());
    let (out, result) = run(|o, e| cmd_push(&addr, &album2, &ws_a.root, o, e));
    assert!(result.is_ok(), "push album two: {}", out.all());

    // ── Machine B: the same two projects ──
    let ws_b = Workspace::new();
    let _b_album1 = ws_b.als_in("Album1", "Track.als");
    let b_album2 = ws_b.als_in("Album2", "Track.als");

    // Pulling the second project must not bring the first project's
    // history or its notes.
    let (out, result) = run(|o, e| cmd_pull(&addr, &b_album2, &ws_b.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());
    assert!(
        !out.all().contains("album one v1"),
        "the pull of Album2/Track.als must not bring Album1's snapshots: {}",
        out.all()
    );

    let (out, result) = run(|o, e| cmd_cues(&b_album2, None, &ws_b.root, o, e));
    assert!(result.is_ok(), "cues: {}", out.all());
    assert!(
        !out.all().contains("a note for album one"),
        "the notes of Album1 must not appear under Album2: {}",
        out.all()
    );

    // And the log of the second project holds only its own snapshot.
    let (out, result) = run(|o, e| cmd_log(&b_album2, &ws_b.root, o, e));
    assert!(result.is_ok(), "log: {}", out.all());
    assert!(
        !out.all().contains("album one v1"),
        "the log of Album2 must not hold Album1's snapshots: {}",
        out.all()
    );
    assert!(
        out.all().contains("album two v1"),
        "the log of Album2 must hold its own snapshot: {}",
        out.all()
    );

    drop(relay);
}

// ─── B2: every pulled snapshot brings its bytes ───────────────────────────
//
// Regression: the relay's "the blob is finished" message for one
// snapshot was still in the queue when the next snapshot's bytes were
// asked for, so the second fetch failed and the relay looked as if it
// had no bytes for a snapshot it did have.

#[test]
fn every_pulled_snapshot_brings_its_own_bytes() {
    let (addr, _dir, relay) = start_relay();

    // ── Machine A: two snapshots of the same project ──
    let ws_a = Workspace::new();
    let als_a = ws_a.als("Beta.als");

    let (out, result) = run(|o, e| {
        cmd_snapshot(&als_a, &Some("v1".to_string()), false, &ws_a.root, o, e)
    });
    assert!(result.is_ok(), "snapshot 1: {}", out.all());
    ws_a.revise(&als_a, 130);
    let (out, result) = run(|o, e| {
        cmd_snapshot(&als_a, &Some("v2".to_string()), false, &ws_a.root, o, e)
    });
    assert!(result.is_ok(), "snapshot 2: {}", out.all());

    let (out, result) = run(|o, e| cmd_push(&addr, &als_a, &ws_a.root, o, e));
    assert!(result.is_ok(), "push: {}", out.all());

    // ── Machine B: pull both snapshots ──
    let ws_b = Workspace::new();
    let als_b = ws_b.als("Beta.als");

    let (out, result) = run(|o, e| cmd_pull(&addr, &als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());

    // Both snapshots must be here.
    let (out, result) = run(|o, e| cmd_log(&als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "log: {}", out.all());
    assert!(out.all().contains("v1"), "the first snapshot: {}", out.all());
    assert!(out.all().contains("v2"), "the second snapshot: {}", out.all());

    // And the bytes of both must have arrived, so both can be restored.
    let (out, result) = run(|o, e| cmd_pull(&addr, &als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "second pull: {}", out.all());
    assert!(
        !out.all().contains("no bytes"),
        "the relay must not claim it has no bytes for a snapshot it holds: {}",
        out.all()
    );

    // The older snapshot is still restorable: its bytes came too.
    let (out, result) = run(|o, e| cmd_restore(&als_b, "HEAD", &ws_b.root, o, e));
    assert!(result.is_ok(), "restore: {}", out.all());

    drop(relay);
}

// ─── B3: a fresh client's head lands on the newest snapshot ──────────────
//
// Regression: a first pull put the head on the *oldest* snapshot of the
// chain, so the newer ones were missing from the log and the status.

#[test]
fn a_fresh_clients_head_lands_on_the_newest_snapshot() {
    let (addr, _dir, relay) = start_relay();

    // ── Machine A: three snapshots ──
    let ws_a = Workspace::new();
    let als_a = ws_a.als("Chain.als");
    for (index, message) in ["one", "two", "three"].iter().enumerate() {
        ws_a.revise(&als_a, 120 + index as u32);
        let (out, result) = run(|o, e| {
            cmd_snapshot(&als_a, &Some(message.to_string()), false, &ws_a.root, o, e)
        });
        assert!(result.is_ok(), "snapshot {}: {}", message, out.all());
    }
    let (out, result) = run(|o, e| cmd_push(&addr, &als_a, &ws_a.root, o, e));
    assert!(result.is_ok(), "push: {}", out.all());

    // ── Machine B: a first pull ──
    let ws_b = Workspace::new();
    let als_b = ws_b.als("Chain.als");
    let (out, result) = run(|o, e| cmd_pull(&addr, &als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());

    // The log must show all three, and the head must be the newest.
    let (out, result) = run(|o, e| cmd_log(&als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "log: {}", out.all());
    for message in ["one", "two", "three"] {
        assert!(
            out.all().contains(message),
            "the log must hold '{}': {}",
            message,
            out.all()
        );
    }
    assert!(
        out.all().contains("three"),
        "the newest snapshot must be in the log: {}",
        out.all()
    );

    // The status must show the newest snapshot as the head.
    let (out, result) = run(|o, e| cmd_status(&als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "status: {}", out.all());
    assert!(
        out.all().contains("three"),
        "the head must be the newest snapshot: {}",
        out.all()
    );

    drop(relay);
}

// ─── B4: a refused push is not stored ─────────────────────────────────────
//
// Regression: a push that the relay reported as a conflict was stored
// anyway, and the relay's head moved onto it. The client was told to
// pull and merge, but the relay had already taken the change.

#[test]
fn a_refused_push_is_not_stored_by_the_relay() {
    let (addr, relay_dir, relay) = start_relay();

    // ── Machine A: the first version ──
    let ws_a = Workspace::new();
    let als_a = ws_a.als("Split.als");
    let (out, result) = run(|o, e| {
        cmd_snapshot(&als_a, &Some("a1".to_string()), false, &ws_a.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());
    let (out, result) = run(|o, e| cmd_push(&addr, &als_a, &ws_a.root, o, e));
    assert!(result.is_ok(), "push: {}", out.all());

    // ── Machine B: a different version of the same project ──
    let ws_b = Workspace::new();
    let als_b = ws_b.als("Split.als");
    ws_b.revise(&als_b, 140);
    let (out, result) = run(|o, e| {
        cmd_snapshot(&als_b, &Some("b1".to_string()), false, &ws_b.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());

    // B's push has a different parent than the relay's head, so the
    // relay refuses it.
    let (out, result) = run(|o, e| cmd_push(&addr, &als_b, &ws_b.root, o, e));
    assert!(result.is_err(), "the push must be refused: {}", out.all());
    assert!(
        out.all().to_lowercase().contains("conflict"),
        "the refusal must be a conflict: {}",
        out.all()
    );

    // The relay must not have kept the refused snapshot, and its head
    // must still be A's snapshot. A refused push that was stored
    // anyway would silently overwrite the relay's history.
    let relay_snapshots: Vec<serde_json::Value> = {
        let path = relay_dir.path().join("snapshots.json");
        let data = fs::read_to_string(&path).expect("the relay keeps its snapshots");
        serde_json::from_str(&data).expect("the relay's snapshots are readable")
    };
    let messages: Vec<String> = relay_snapshots
        .iter()
        .filter_map(|s| s.get("message").and_then(|m| m.as_str()).map(String::from))
        .collect();
    assert!(
        messages.contains(&"a1".to_string()),
        "the relay must still hold A's snapshot: {:?}",
        messages
    );
    assert!(
        !messages.contains(&"b1".to_string()),
        "the refused snapshot must not be on the relay: {:?}",
        messages
    );

    // And the head must not have moved onto the refused push.
    let heads: serde_json::Value = {
        let path = relay_dir.path().join("heads.json");
        let data = fs::read_to_string(&path).expect("the relay keeps its heads");
        serde_json::from_str(&data).expect("the relay's heads are readable")
    };
    let head_values: Vec<String> = heads
        .as_object()
        .map(|o| o.values().filter_map(|v| v.as_str()).map(String::from).collect())
        .unwrap_or_default();
    assert_eq!(head_values.len(), 1, "one head: {:?}", head_values);

    drop(relay);
}

// ─── A collaborator in another folder finds the right project ────────────
//
// The folder tells two projects that share a file name apart, but a
// collaborator usually keeps the project in a folder of their own. The
// hash of the file says which project it is, so the pull finds the
// right history.

#[test]
fn a_collaborator_in_another_folder_pulls_the_right_project() {
    let (addr, _dir, _relay) = start_relay();

    // ── Machine A: one project, in a folder of its own ──
    let ws_a = Workspace::new();
    let als_a = ws_a.als_in("Album2", "Track.als");
    ws_a.revise(&als_a, 140);

    let (out, result) = run(|o, e| {
        cmd_snapshot(&als_a, &Some("album two v1".to_string()), false, &ws_a.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());
    let (out, result) = run(|o, e| cmd_push(&addr, &als_a, &ws_a.root, o, e));
    assert!(result.is_ok(), "push: {}", out.all());

    // ── Machine B: the same file, in a folder of its own ──
    let ws_b = Workspace::new();
    let als_b = ws_b.als_in("MyMusic", "Track.als");
    ws_b.revise(&als_b, 140);

    let (out, result) = run(|o, e| cmd_pull(&addr, &als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());
    assert!(
        out.all().contains("album two v1"),
        "the collaborator must find the project: {}",
        out.all()
    );

    // And the bytes must come too, so the snapshot can be restored.
    let (out, result) = run(|o, e| cmd_restore(&als_b, "HEAD", &ws_b.root, o, e));
    assert!(result.is_ok(), "restore: {}", out.all());
    let restored = fs::read(&als_b).expect("the restored file");
    let original = fs::read(&als_a).expect("the original file");
    assert_eq!(
        restored, original,
        "the restored project must be the collaborator's file"
    );
}

// ─── The same bytes in two folders are still two projects ────────────────
//
// Two folders can hold the very same file. The relay must not treat
// them as one project, or one of them would get the other's history.

#[test]
fn the_same_bytes_in_two_folders_are_two_projects() {
    let (addr, _dir, relay) = start_relay();

    // ── Machine A: two folders with the same file in them ──
    let ws_a = Workspace::new();
    let one = ws_a.als_in("Album1", "Track.als");
    let two = ws_a.als_in("Album2", "Track.als");

    let (out, result) = run(|o, e| {
        cmd_snapshot(&one, &Some("first copy".to_string()), false, &ws_a.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());
    let (out, result) = run(|o, e| {
        cmd_snapshot(&two, &Some("second copy".to_string()), false, &ws_a.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());

    let (out, result) = run(|o, e| cmd_push(&addr, &one, &ws_a.root, o, e));
    assert!(result.is_ok(), "push one: {}", out.all());
    let (out, result) = run(|o, e| cmd_push(&addr, &two, &ws_a.root, o, e));
    assert!(result.is_ok(), "push two: {}", out.all());

    // ── Machine B: the same two folders ──
    let ws_b = Workspace::new();
    let _b_one = ws_b.als_in("Album1", "Track.als");
    let b_two = ws_b.als_in("Album2", "Track.als");

    let (out, result) = run(|o, e| cmd_pull(&addr, &b_two, &ws_b.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());
    let (out, result) = run(|o, e| cmd_log(&b_two, &ws_b.root, o, e));
    assert!(result.is_ok(), "log: {}", out.all());
    assert!(
        !out.all().contains("first copy"),
        "the other folder's snapshot must not turn up here: {}",
        out.all()
    );
    assert!(
        out.all().contains("second copy"),
        "this folder's snapshot must be here: {}",
        out.all()
    );

    drop(relay);
}
