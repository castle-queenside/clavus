//! Integration tests for `clavus diff`.
//!
//! These drive the real command against real files: a set is written,
//! snapshotted, changed, and the difference is read back. Nothing here
//! reimplements the diff — it is the command under test that has to
//! know what changed.

use std::fs;
use std::path::{Path, PathBuf};

use clavus_cli::{cmd_diff, cmd_snapshot};
use tempfile::TempDir;

// ─── Helpers ──────────────────────────────────────────────────────────────

struct Workspace {
    _tmp: TempDir,
    root: PathBuf,
    dir: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path().join(".clavus");
        let dir = tmp.path().join("Project");
        fs::create_dir_all(&dir).expect("project dir");
        Workspace {
            _tmp: tmp,
            root,
            dir,
        }
    }

    fn als_path(&self) -> PathBuf {
        self.dir.join("MyProject.als")
    }

    /// Write a set. `body` is the tracks of the LiveSet.
    fn write_als(&self, body: &str) -> PathBuf {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <MasterTrack>
    <Name Value="Master" />
    <DeviceChain><TempoEnvelope><Events><AudioPoint Time="0.0" Value="128" /></Events></TempoEnvelope></DeviceChain>
  </MasterTrack>
  {}
</LiveSet>"#,
            body
        );
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut enc, xml.as_bytes()).expect("gzip");
        let path = self.als_path();
        fs::write(&path, enc.finish().expect("gzip finish")).expect("write als");
        path
    }
}

/// A track with a name and a device.
fn track(name: &str, device: &str) -> String {
    format!(
        r#"<AudioTrack><Name Value="{}" /><DeviceChain><Devices><Device><Name Value="{}" /></Device></Devices></DeviceChain></AudioTrack>"#,
        name, device
    )
}

/// The result of running a command.
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

/// Take a snapshot and return the hash it was given.
///
/// The hash is the only way to name a snapshot, so a test that wants
/// to compare two of them has to get it from the same place the user
/// does — the output of the snapshot command.
fn snapshot(ws: &Workspace, als: &Path, message: &str) -> String {
    let (out, result) = run(|o, e| cmd_snapshot(als, &Some(message.to_string()), false, &ws.root, o, e));
    assert!(result.is_ok(), "the snapshot must succeed: {:?}", result);
    let text = out.all();
    let hash = text
        .split("snapshot ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("the output should name the snapshot: {}", text))
        .to_string();
    assert_eq!(hash.len(), 8, "the short hash should be eight characters: {}", text);
    hash
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[test]
fn diff_of_a_project_with_no_snapshots_says_so() {
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));
    let (out, result) = run(|o, e| cmd_diff(&als, false, &None, &None, &ws.root, o, e));
    assert!(result.is_ok(), "diff with no snapshots should not fail");
    assert!(
        out.all().contains("No snapshots yet"),
        "should say there are no snapshots: {}",
        out.all()
    );
}

#[test]
fn diff_walks_the_chain_and_says_what_changed() {
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));

    let (_, result) = run(|o, e| cmd_snapshot(&als, &Some("first".to_string()), false, &ws.root, o, e));
    assert!(result.is_ok(), "the first snapshot must succeed: {:?}", result);

    // Rename the track and add another one.
    let als = ws.write_als(&format!(
        "{}{}",
        track("Kick Drum", "Compressor"),
        track("Snare", "EQ Eight")
    ));
    let (_, result) = run(|o, e| cmd_snapshot(&als, &Some("second".to_string()), false, &ws.root, o, e));
    assert!(result.is_ok(), "the second snapshot must succeed: {:?}", result);

    let (out, result) = run(|o, e| cmd_diff(&als, false, &None, &None, &ws.root, o, e));
    assert!(result.is_ok(), "diff should succeed: {:?}", result);
    let text = out.all();
    assert!(text.contains("2 snapshot"), "should show the chain: {}", text);
    assert!(
        text.contains("renamed from Kick"),
        "the rename should be reported: {}",
        text
    );
    assert!(text.contains("Snare"), "the added track should be reported: {}", text);
    assert!(text.contains("added"), "the added track should be marked: {}", text);
}

#[test]
fn diff_live_says_what_changed_since_the_last_snapshot() {
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));

    let (_, result) = run(|o, e| cmd_snapshot(&als, &Some("first".to_string()), false, &ws.root, o, e));
    assert!(result.is_ok(), "the first snapshot must succeed: {:?}", result);

    // Change the set on disk.
    let als = ws.write_als(&format!(
        "{}{}",
        track("Kick", "Compressor"),
        track("Snare", "EQ Eight")
    ));

    let (out, result) = run(|o, e| cmd_diff(&als, true, &None, &None, &ws.root, o, e));
    assert!(result.is_ok(), "diff --live should succeed: {:?}", result);
    let text = out.all();
    assert!(text.contains("Snare"), "the added track should be reported: {}", text);
    assert!(text.contains("Tracks"), "the track count should be reported: {}", text);
}

#[test]
fn diff_live_says_nothing_changed_when_the_set_is_the_same() {
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));
    let (_, result) = run(|o, e| cmd_snapshot(&als, &Some("first".to_string()), false, &ws.root, o, e));
    assert!(result.is_ok(), "the first snapshot must succeed: {:?}", result);

    let (out, result) = run(|o, e| cmd_diff(&als, true, &None, &None, &ws.root, o, e));
    assert!(result.is_ok(), "diff --live should succeed: {:?}", result);
    assert!(
        out.all().contains("Nothing has changed"),
        "should say nothing changed: {}",
        out.all()
    );
}

#[test]
fn diff_compares_two_snapshots() {
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));
    let first = snapshot(&ws, &als, "first");

    let als = ws.write_als(&format!(
        "{}{}",
        track("Kick", "Compressor"),
        track("Snare", "EQ Eight")
    ));
    let second = snapshot(&ws, &als, "second");

    let (out, result) = run(|o, e| {
        cmd_diff(
            &als,
            false,
            &Some(first.clone()),
            &Some(second.clone()),
            &ws.root,
            o,
            e,
        )
    });
    assert!(result.is_ok(), "diff should succeed: {:?}", result);
    let text = out.all();
    assert!(text.contains("Comparing"), "should say what it compared: {}", text);
    assert!(text.contains(&first), "should name the first snapshot: {}", text);
    assert!(text.contains(&second), "should name the second snapshot: {}", text);
    assert!(text.contains("Snare"), "the added track should be reported: {}", text);
}

#[test]
fn diff_of_a_snapshot_that_is_not_there_fails() {
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));
    let (_, result) = run(|o, e| cmd_snapshot(&als, &Some("first".to_string()), false, &ws.root, o, e));
    assert!(result.is_ok(), "the first snapshot must succeed: {:?}", result);

    let (_out, result) = run(|o, e| {
        cmd_diff(
            &als,
            false,
            &Some("nosuchhash".to_string()),
            &None,
            &ws.root,
            o,
            e,
        )
    });
    assert!(result.is_err(), "an unknown snapshot must be an error");
    let message = format!("{:?}", result.unwrap_err());
    assert!(
        message.contains("not found") || message.contains("not in"),
        "the error should be explained: {}",
        message
    );
}

#[test]
fn diff_of_a_missing_project_fails() {
    let ws = Workspace::new();
    let missing = ws.dir.join("NoSuchSet.als");
    let (_, result) = run(|o, e| cmd_diff(&missing, false, &None, &None, &ws.root, o, e));
    assert!(result.is_err(), "a missing project must be an error");
}

#[test]
fn diff_live_fails_when_the_bytes_of_the_last_snapshot_are_missing() {
    // The record is there but the set it took is gone. That must not
    // be reported as "nothing has changed".
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));
    snapshot(&ws, &als, "first");

    remove_the_project_bytes(&ws, &als);

    let (out, result) = run(|o, e| cmd_diff(&als, true, &None, &None, &ws.root, o, e));
    assert!(result.is_err(), "missing bytes must be an error");
    let text = out.all();
    assert!(
        text.contains("not in the store") || text.contains("missing"),
        "the error should say the bytes are missing: {}",
        text
    );
    assert!(
        !text.contains("Nothing has changed"),
        "a broken store must never be reported as unchanged: {}",
        text
    );
}

#[test]
fn diff_chain_says_a_step_could_not_be_compared() {
    // One step of the chain cannot be compared. The walk must say so
    // rather than claim that nothing differs.
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));
    snapshot(&ws, &als, "first");

    let als = ws.write_als(&format!(
        "{}{}",
        track("Kick", "Compressor"),
        track("Snare", "EQ Eight")
    ));
    snapshot(&ws, &als, "second");

    remove_the_project_bytes(&ws, &als);

    let (out, result) = run(|o, e| cmd_diff(&als, false, &None, &None, &ws.root, o, e));
    assert!(result.is_ok(), "the walk should not fail: {:?}", result);
    let text = out.all();
    assert!(
        text.contains("could not be compared"),
        "the missing step should be named: {}",
        text
    );
    assert!(
        !text.contains("Nothing in this chain differs"),
        "an uncompared step must not be reported as no difference: {}",
        text
    );
}

#[test]
fn diff_of_a_corrupt_snapshot_record_fails() {
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));
    snapshot(&ws, &als, "first");

    // Write rubbish over the snapshot record.
    let records = ws.root.join("blobs").join("projects");
    let mut found = false;
    for entry in walk(&records) {
        if entry.extension().map(|e| e == "json").unwrap_or(false) {
            fs::write(&entry, b"{ this is not json").expect("corrupt the record");
            found = true;
        }
    }
    assert!(found, "there should be a snapshot record to corrupt");

    let (_, result) = run(|o, e| cmd_diff(&als, false, &None, &None, &ws.root, o, e));
    assert!(result.is_err(), "a corrupt record must be an error");
    let message = format!("{:?}", result.unwrap_err());
    assert!(
        message.contains("corrupt"),
        "the error should say the record is corrupt: {}",
        message
    );
}

/// Every file under a directory.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// Remove the stored bytes of the newest snapshot of a project.
///
/// The snapshot's hash is the hash of the set it took, so the file to
/// remove is the one named after that hash.
fn remove_the_project_bytes(ws: &Workspace, als: &Path) {
    let _ = als;
    let mut removed = false;
    for path in walk(&ws.root.join("blobs").join("objects")) {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.ends_with(".sample") {
            continue;
        }
        fs::remove_file(&path).expect("remove the stored bytes");
        removed = true;
    }
    assert!(removed, "there should be stored bytes to remove");
}

#[test]
fn diff_does_not_write_to_the_set() {
    let ws = Workspace::new();
    let als = ws.write_als(&track("Kick", "Compressor"));
    let (_, result) = run(|o, e| cmd_snapshot(&als, &Some("first".to_string()), false, &ws.root, o, e));
    assert!(result.is_ok(), "the first snapshot must succeed: {:?}", result);

    let before = fs::read(&als).expect("read als");
    let (out, result) = run(|o, e| cmd_diff(&als, true, &None, &None, &ws.root, o, e));
    assert!(result.is_ok(), "diff --live should succeed: {:?}", result);
    let after = fs::read(&als).expect("read als");
    assert_eq!(before, after, "the diff changed the set");
    assert!(!out.all().is_empty(), "the diff should say something");
}
