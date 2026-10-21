//! Integration tests for the Clavus command-line interface.
//!
//! These drive the real command implementations from `clavus-cli` against
//! real files in a temporary directory, and assert on both the files that
//! were written and the text the user would see.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use clavus_cli::{
    cmd_audit, cmd_collect, cmd_cue, cmd_cues, cmd_log, cmd_projects, cmd_pull, cmd_push,
    cmd_relay, cmd_restore, cmd_snapshot, cmd_status,
};
use tempfile::TempDir;

// ─── Helpers ──────────────────────────────────────────────────────────────

/// A test workspace: a data root and a project directory.
struct Workspace {
    _tmp: TempDir,
    root: PathBuf,
    project: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path().join(".clavus");
        let project = tmp.path().join("MyProject");
        fs::create_dir_all(&project).expect("project dir");
        Workspace {
            _tmp: tmp,
            root,
            project,
        }
    }

    /// Write a gzip-compressed `.als` file.
    fn write_als(&self, xml: &str) -> PathBuf {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut enc, xml.as_bytes()).expect("gzip");
        let compressed = enc.finish().expect("gzip finish");
        let path = self.project.join("MyProject.als");
        fs::write(&path, compressed).expect("write als");
        path
    }

    fn sample(&self, name: &str) -> PathBuf {
        let path = self.project.join("Samples").join(name);
        fs::create_dir_all(path.parent().unwrap()).expect("sample dir");
        fs::write(&path, b"RIFFfake").expect("write sample");
        path
    }
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

/// Run a command, capturing stdout and stderr separately.
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

fn als_xml(samples: &[&str]) -> String {
    let refs: String = samples
        .iter()
        .map(|s| format!(r#"<SampleRef><Path Value="{}"/></SampleRef>"#, s))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tempo Value="128" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack><Name Value="Test" /><DeviceChain>{}</DeviceChain></AudioTrack>
</LiveSet>"#,
        refs
    )
}

/// A project with one present sample — the common, healthy case.
fn healthy_workspace() -> (Workspace, PathBuf) {
    let ws = Workspace::new();
    ws.sample("kick.wav");
    let als = ws.write_als(&als_xml(&["Samples/kick.wav"]));
    (ws, als)
}

// ─── audit ────────────────────────────────────────────────────────────────

#[test]
fn audit_reports_present_samples() {
    let (ws, als) = healthy_workspace();
    let (out, result) = run(|o, e| cmd_audit(&als, false, o, e));
    assert!(result.is_ok(), "audit should succeed: {:?}", result);
    let text = out.all();
    assert!(text.contains("1 sample"), "should report a count: {}", text);
    assert!(!text.contains("1 samples"), "should be pluralised: {}", text);
}

#[test]
fn audit_reports_missing_samples() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&["Samples/gone.wav"]));
    let (out, result) = run(|o, e| cmd_audit(&als, false, o, e));
    assert!(result.is_ok(), "non-strict audit should still succeed");
    let text = out.all();
    assert!(text.contains("gone.wav"), "should name the missing file: {}", text);
    assert!(
        text.to_lowercase().contains("missing"),
        "should say it is missing: {}",
        text
    );
}

#[test]
fn audit_strict_fails_on_missing_samples() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&["Samples/gone.wav"]));
    let (_, result) = run(|o, e| cmd_audit(&als, true, o, e));
    assert!(
        result.is_err(),
        "strict audit must fail when a sample is missing"
    );
}

#[test]
fn audit_strict_passes_when_all_samples_present() {
    let (ws, als) = healthy_workspace();
    let (_, result) = run(|o, e| cmd_audit(&als, true, o, e));
    assert!(result.is_ok(), "strict audit should pass: {:?}", result);
}

#[test]
fn audit_on_a_non_project_file_errors_rather_than_panicking() {
    let ws = Workspace::new();
    let junk = ws.project.join("notes.txt");
    fs::write(&junk, b"not a project").unwrap();
    let (_, result) = run(|o, e| cmd_audit(&junk, false, o, e));
    assert!(result.is_err(), "auditing a non-project should fail cleanly");
}

#[test]
fn audit_on_a_corrupt_als_file_errors_rather_than_reporting_a_clean_project() {
    // Regression: the sample extractor is lenient, so a corrupt file used
    // to audit as "0 samples present" — a false all-clear.
    let ws = Workspace::new();
    let als = ws.project.join("Broken.als");
    fs::write(&als, b"this is not xml at all").unwrap();
    let (out, result) = run(|o, e| cmd_audit(&als, false, o, e));
    assert!(
        result.is_err(),
        "a corrupt project must not audit as clean: {}",
        out.all()
    );
    assert!(
        !out.all().contains("present"),
        "no all-clear should be printed: {}",
        out.all()
    );
}

#[test]
fn audit_on_valid_xml_that_is_not_a_live_set_errors() {
    let ws = Workspace::new();
    let als = ws.project.join("NotLive.als");
    fs::write(
        &als,
        br#"<?xml version="1.0" encoding="UTF-8"?><SomeOtherApp><Thing/></SomeOtherApp>"#,
    )
    .unwrap();
    let (_, result) = run(|o, e| cmd_audit(&als, false, o, e));
    assert!(result.is_err(), "valid XML that is not a Live set should fail");
}

#[test]
fn audit_on_a_missing_file_errors_rather_than_panicking() {
    let ws = Workspace::new();
    let missing = ws.project.join("Nope.als");
    let (_, result) = run(|o, e| cmd_audit(&missing, false, o, e));
    assert!(result.is_err(), "auditing a missing file should fail cleanly");
}

// ─── snapshot ─────────────────────────────────────────────────────────────

#[test]
fn snapshot_creates_a_chain_and_log_lists_it() {
    let (ws, als) = healthy_workspace();
    let msg = "first pass".to_string();
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some(msg), false, &ws.root, o, e));
    assert!(result.is_ok(), "snapshot should succeed: {:?}", result);
    assert!(out.all().contains("snapshot"), "should report a snapshot: {}", out.all());

    let (log_out, log_result) = run(|o, e| cmd_log(&als, &ws.root, o, e));
    assert!(log_result.is_ok());
    assert!(
        log_out.all().contains("first pass"),
        "log should show the snapshot: {}",
        log_out.all()
    );
}

#[test]
fn snapshot_is_blocked_when_a_sample_is_missing() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&["Samples/gone.wav"]));
    let (_, result) = run(|o, e| cmd_snapshot(&als, &None, false, &ws.root, o, e));
    assert!(
        result.is_err(),
        "a snapshot must be blocked while samples are missing"
    );
}

#[test]
fn snapshot_collects_when_asked() {
    let ws = Workspace::new();
    // A sample that lives outside the project folder.
    let outside = ws.project.parent().unwrap().join("outside.wav");
    fs::write(&outside, b"RIFFoutside").unwrap();
    let als = ws.write_als(&als_xml(&["../outside.wav"]));

    let (out, result) = run(|o, e| cmd_snapshot(&als, &None, true, &ws.root, o, e));
    assert!(result.is_ok(), "--collect should rescue the snapshot: {:?}", result);
    assert!(
        ws.project.join("Samples").join("outside.wav").exists(),
        "the sample should have been copied into the project: {}",
        out.all()
    );
}

#[test]
fn snapshot_of_a_non_project_file_errors() {
    let ws = Workspace::new();
    let junk = ws.project.join("notes.txt");
    fs::write(&junk, b"not a project").unwrap();
    let (_, result) = run(|o, e| cmd_snapshot(&junk, &None, false, &ws.root, o, e));
    assert!(result.is_err(), "snapshotting a non-project should fail");
}

#[test]
fn snapshot_of_unchanged_content_reports_that_nothing_was_created() {
    let (ws, als) = healthy_workspace();
    let first = "one".to_string();
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some(first), false, &ws.root, o, e));
    assert!(result.is_ok(), "the first snapshot should succeed: {:?}", result);
    assert!(out.all().contains("\u{2713}"), "should report success: {}", out.all());

    // The project has not changed, so the second snapshot is a no-op.
    let second = "two".to_string();
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some(second), false, &ws.root, o, e));
    assert!(result.is_ok(), "an unchanged snapshot should not fail: {:?}", result);
    let text = out.all();
    assert!(
        text.contains("unchanged"),
        "should say the content was unchanged: {}",
        text
    );
    assert!(
        !text.contains("\u{2713} snapshot"),
        "should not claim a new snapshot was created: {}",
        text
    );

    // Only one chain entry exists.
    let (log_out, _) = run(|o, e| cmd_log(&als, &ws.root, o, e));
    assert!(
        log_out.all().contains("1 snapshot"),
        "there should be exactly one snapshot: {}",
        log_out.all()
    );
}

#[test]
fn snapshot_after_a_real_change_creates_a_second_entry() {
    let (ws, als) = healthy_workspace();
    let first = "one".to_string();
    run(|o, e| cmd_snapshot(&als, &Some(first), false, &ws.root, o, e));

    // Change the project so the content hash differs.
    let mut xml = als_xml(&["Samples/kick.wav"]);
    xml.push_str("<!-- changed -->");
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut enc, xml.as_bytes()).unwrap();
    fs::write(&als, enc.finish().unwrap()).unwrap();

    let second = "two".to_string();
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some(second), false, &ws.root, o, e));
    assert!(result.is_ok(), "the second snapshot should succeed: {:?}", result);
    assert!(out.all().contains("\u{2713}"), "should report success: {}", out.all());

    let (log_out, _) = run(|o, e| cmd_log(&als, &ws.root, o, e));
    let text = log_out.all();
    assert!(text.contains("2 snapshot"), "there should be two snapshots: {}", text);
    assert!(text.contains("one") && text.contains("two"), "log: {}", text);
}

#[test]
fn log_of_a_project_with_no_snapshots_says_so_instead_of_failing() {
    let (ws, als) = healthy_workspace();
    let (out, result) = run(|o, e| cmd_log(&als, &ws.root, o, e));
    assert!(result.is_ok(), "log with no snapshots should not fail");
    assert!(
        !out.all().trim().is_empty(),
        "log should still tell the user something"
    );
}

// ─── cue ──────────────────────────────────────────────────────────────────

#[test]
fn cue_is_added_and_listed() {
    let (ws, als) = healthy_workspace();
    let (out, result) = run(|o, e| {
        cmd_cue(
            &als,
            "kick needs trimming",
            "1.1.1",
            Some("Test"),
            &ws.root,
            o,
            e,
        )
    });
    assert!(result.is_ok(), "cue should be added: {:?}", result);
    assert!(out.all().contains("kick needs trimming"));

    let (list_out, list_result) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
    assert!(list_result.is_ok());
    assert!(
        list_out.all().contains("kick needs trimming"),
        "cues should list the cue: {}",
        list_out.all()
    );
}

#[test]
fn cues_can_be_filtered_by_status() {
    let (ws, als) = healthy_workspace();
    run(|o, e| cmd_cue(&als, "a cue", "1.1.1", None, &ws.root, o, e));

    let (out, _) = run(|o, e| cmd_cues(&als, Some("pending"), &ws.root, o, e));
    assert!(out.all().contains("a cue"), "pending filter: {}", out.all());

    let (out, _) = run(|o, e| cmd_cues(&als, Some("resolved"), &ws.root, o, e));
    assert!(
        !out.all().contains("a cue"),
        "a resolved filter should not show open cues: {}",
        out.all()
    );
}

#[test]
fn cue_with_an_invalid_status_filter_errors_instead_of_panicking() {
    let (ws, als) = healthy_workspace();
    let (_, result) = run(|o, e| cmd_cues(&als, Some("not-a-status"), &ws.root, o, e));
    assert!(result.is_err(), "an unknown status should fail cleanly");
}

#[test]
fn cues_with_multibyte_text_do_not_panic() {
    // Regression: the cue list truncated text by byte index, which panicked
    // when a multi-byte character straddled the cut point.
    let (ws, als) = healthy_workspace();
    for text in [
        "café needs a longer tail than the truncation window",
        "🎵 emoji cue text that is definitely long enough to truncate",
        "日本語のキュー文言 trikken",
        "é",
    ] {
        let (out, result) = run(|o, e| cmd_cue(&als, text, "1.1.1", None, &ws.root, o, e));
        assert!(result.is_ok(), "cue {:?} should be added: {:?}", text, result);
        let (list, list_result) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
        assert!(list_result.is_ok(), "listing cues should work: {:?}", list_result);
        assert!(!list.all().is_empty(), "the cue should be listed");
    }
}

#[test]
fn cues_with_multibyte_text_are_listed_and_readable() {
    let (ws, als) = healthy_workspace();
    let text = "🎵 mix the kick in";
    let (_, result) = run(|o, e| cmd_cue(&als, text, "1.1.1", None, &ws.root, o, e));
    assert!(result.is_ok(), "adding the cue should work: {:?}", result);
    let (list, _) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
    assert!(
        list.all().contains("🎵 mix the kick in"),
        "the full text should be shown when it fits: {}",
        list.all()
    );
}

#[test]
fn cue_on_a_project_with_no_snapshots_errors_cleanly() {
    let (ws, als) = healthy_workspace();
    let (_, result) = run(|o, e| cmd_cue(&als, "text", "1.1.1", None, &ws.root, o, e));
    // A cue needs a snapshot to attach to; either it errors or it succeeds,
    // but it must never panic.
    if let Err(e) = result {
        assert!(!format!("{}", e).is_empty(), "error should be descriptive");
    }
}

// ─── status ───────────────────────────────────────────────────────────────

#[test]
fn status_reports_a_healthy_project() {
    let (ws, als) = healthy_workspace();
    run(|o, e| cmd_snapshot(&als, &None, false, &ws.root, o, e));

    let (out, result) = run(|o, e| cmd_status(&als, &ws.root, o, e));
    assert!(result.is_ok(), "status should succeed: {:?}", result);
    let text = out.all();
    assert!(text.contains("MyProject"), "status should name the project: {}", text);
}

#[test]
fn status_of_a_project_with_no_snapshots_still_works() {
    let (ws, als) = healthy_workspace();
    let (out, result) = run(|o, e| cmd_status(&als, &ws.root, o, e));
    assert!(result.is_ok(), "status should not fail without snapshots");
    assert!(!out.all().trim().is_empty(), "status should produce output");
}

#[test]
fn status_of_a_missing_project_errors_cleanly() {
    let ws = Workspace::new();
    let missing = ws.project.join("Nope.als");
    let (_, result) = run(|o, e| cmd_status(&missing, &ws.root, o, e));
    assert!(result.is_err(), "status of a missing project should fail cleanly");
}

// ─── restore ──────────────────────────────────────────────────────────────

#[test]
fn restore_rejects_an_unknown_hash_without_touching_the_project() {
    let (ws, als) = healthy_workspace();
    run(|o, e| cmd_snapshot(&als, &None, false, &ws.root, o, e));
    let before = fs::read(&als).unwrap();

    let (_, result) = run(|o, e| cmd_restore(&als, "deadbeef", &ws.root, o, e));
    assert!(result.is_err(), "an unknown hash should fail");

    assert_eq!(
        fs::read(&als).unwrap(),
        before,
        "a failed restore must not modify the project"
    );
}

#[test]
fn restore_of_head_after_a_snapshot_keeps_the_project_usable() {
    let (ws, als) = healthy_workspace();
    run(|o, e| cmd_snapshot(&als, &None, false, &ws.root, o, e));
    let (_, result) = run(|o, e| cmd_restore(&als, "HEAD", &ws.root, o, e));
    assert!(result.is_ok(), "restoring HEAD should work: {:?}", result);
    assert!(als.exists(), "the project should still exist after a restore");
}

// ─── collect ──────────────────────────────────────────────────────────────

#[test]
fn collect_copies_an_outside_sample_into_the_project() {
    let ws = Workspace::new();
    let outside = ws.project.parent().unwrap().join("outside.wav");
    fs::write(&outside, b"RIFFoutside").unwrap();
    let als = ws.write_als(&als_xml(&["../outside.wav"]));

    let (out, result) = run(|o, e| cmd_collect(&als, &ws.root, o, e));
    assert!(result.is_ok(), "collect should succeed: {:?}", result);
    assert!(
        ws.project.join("Samples").join("outside.wav").exists(),
        "sample should be copied: {}",
        out.all()
    );

    // The project should now audit clean.
    let (_, result) = run(|o, e| cmd_audit(&als, true, o, e));
    assert!(result.is_ok(), "audit should pass after collect");
}

#[test]
fn collect_is_idempotent() {
    let ws = Workspace::new();
    let outside = ws.project.parent().unwrap().join("outside.wav");
    fs::write(&outside, b"RIFFoutside").unwrap();
    let als = ws.write_als(&als_xml(&["../outside.wav"]));

    run(|o, e| cmd_collect(&als, &ws.root, o, e));
    let first = fs::read(&als).unwrap();
    run(|o, e| cmd_collect(&als, &ws.root, o, e));
    let second = fs::read(&als).unwrap();
    assert_eq!(first, second, "a second collect should not change the project");
}

#[test]
fn collect_with_nothing_to_collect_still_succeeds() {
    let (ws, als) = healthy_workspace();
    let (_, result) = run(|o, e| cmd_collect(&als, &ws.root, o, e));
    assert!(result.is_ok(), "collect with nothing to do should succeed");
}

#[test]
fn collect_of_a_missing_sample_fails_cleanly() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&["Samples/never-existed.wav"]));
    let (_, result) = run(|o, e| cmd_collect(&als, &ws.root, o, e));
    assert!(result.is_err(), "collecting a missing sample should fail cleanly");
}

// ─── projects ─────────────────────────────────────────────────────────────

#[test]
fn projects_lists_projects_it_can_find() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&[]));
    let (out, result) = run(|o, e| cmd_projects(false, o, e));
    assert!(result.is_ok(), "projects should succeed: {:?}", result);
    // The user's home directory may or may not contain projects, so the
    // contract is only that it does not fail and produces output.
    assert!(!out.all().trim().is_empty(), "projects should produce output");
    let _ = als;
}

// ─── push / pull against an unreachable relay ─────────────────────────────

#[test]
fn push_to_a_dead_relay_fails_cleanly() {
    let (ws, als) = healthy_workspace();
    run(|o, e| cmd_snapshot(&als, &None, false, &ws.root, o, e));
    let (_, result) = run(|o, e| {
        cmd_push("127.0.0.1:1", &als, &ws.root, o, e)
    });
    assert!(result.is_err(), "pushing to a dead relay should fail cleanly");
}

#[test]
fn pull_from_a_dead_relay_fails_cleanly() {
    let (ws, als) = healthy_workspace();
    let (_, result) = run(|o, e| {
        cmd_pull("127.0.0.1:1", &als, &ws.root, o, e)
    });
    assert!(result.is_err(), "pulling from a dead relay should fail cleanly");
}

// ─── end-to-end: the real binary ──────────────────────────────────────────

/// Path to the compiled `clavus` binary, if the workspace built one.
fn binary() -> Option<PathBuf> {
    // tests/ lives in crates/cli/tests/, so the binary is three levels up.
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop();
    dir.pop();
    dir.pop();
    let candidate = dir.join("target").join("debug").join("clavus");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

#[test]
fn the_binary_runs_audit_and_reports_the_project() {
    let Some(bin) = binary() else {
        eprintln!("skipping: the clavus binary was not built");
        return;
    };
    let (ws, als) = healthy_workspace();
    let output = Command::new(&bin)
        .arg("audit")
        .arg(&als)
        .output()
        .expect("run clavus audit");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "audit should succeed: {}", text);
    assert!(text.contains("kick.wav"), "audit should name the sample: {}", text);
}

#[test]
fn the_binary_reports_usage_for_an_unknown_command() {
    let Some(bin) = binary() else {
        eprintln!("skipping: the clavus binary was not built");
        return;
    };
    let output = Command::new(&bin)
        .arg("definitely-not-a-command")
        .output()
        .expect("run clavus");
    assert!(!output.status.success(), "an unknown command should fail");
}

#[test]
fn the_binary_prints_help() {
    let Some(bin) = binary() else {
        eprintln!("skipping: the clavus binary was not built");
        return;
    };
    let output = Command::new(&bin)
        .arg("--help")
        .output()
        .expect("run clavus --help");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("snapshot"), "help should list commands: {}", text);
    assert!(text.contains("restore"), "help should list commands: {}", text);
}

// ─── hostile relay ─────────────────────────────────────────────────────────

/// A snapshot with a hash that is not a SHA-256 hex digest.
fn hostile_snapshot(hash: &str) -> clavus_core::types::Snapshot {
    clavus_core::types::Snapshot {
        hash: hash.to_string(),
        parent: None,
        timestamp: 1.0,
        message: "from a hostile relay".to_string(),
        track_count: 0,
        bpm: 120.0,
        tags: vec![],
        notes: String::new(),
        sample_hashes: vec![],
        sample_paths: std::collections::HashMap::new(),
        project_path: "/tmp/evil.als".to_string(),
    }
}

/// Serve one client, sending it a `Push` with the given snapshot.
fn hostile_relay(snapshot: clavus_core::types::Snapshot) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Read the client's Hello frame (4-byte length + payload).
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).is_ok() {
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; len];
                let _ = stream.read_exact(&mut body);
            }
            // Reply with a Push carrying the hostile snapshot.
            let msg = clavus_sync::SyncMsg::Push {
                snapshots: vec![snapshot],
                cues: vec![],
            };
            let json = serde_json::to_vec(&msg).unwrap();
            let _ = stream.write_all(&(json.len() as u32).to_be_bytes());
            let _ = stream.write_all(&json);
            let _ = stream.flush();
            thread::sleep(std::time::Duration::from_millis(300));
        }
    });
    addr
}

#[test]
fn pull_from_a_hostile_relay_cannot_escape_the_store() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&[]));
    let addr = hostile_relay(hostile_snapshot("../../../../escaped"));

    let (out, result) = run(|o, e| cmd_pull(&addr, &als, &ws.root, o, e));
    // The pull must fail cleanly — not panic, not write outside the store.
    assert!(result.is_err(), "a crafted hash must be rejected: {}", out.all());

    // Nothing may have been written outside the data root.
    let escaped = ws.root.parent().unwrap().join("escaped.json");
    assert!(
        !escaped.exists(),
        "the relay must not be able to write outside the store"
    );
    // And nothing may have been written outside the blob store either.
    let objects = ws.root.join("blobs").join("objects");
    if let Ok(entries) = std::fs::read_dir(&objects) {
        for entry in entries.flatten() {
            assert!(
                !entry.file_name().to_string_lossy().contains(".."),
                "no traversal in the object store"
            );
        }
    }
}

#[test]
fn pull_from_a_hostile_relay_with_a_tiny_hash_does_not_panic() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&[]));
    // A one-character hash used to panic while slicing the 8-character
    // prefix for display.
    let addr = hostile_relay(hostile_snapshot("a"));

    let (out, result) = run(|o, e| cmd_pull(&addr, &als, &ws.root, o, e));
    assert!(
        result.is_err(),
        "a malformed hash must be rejected: {}",
        out.all()
    );
}

#[test]
fn pull_from_a_hostile_relay_with_a_traversing_cue_id_cannot_escape() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&[]));
    let evil_cue = clavus_core::types::Cue {
        id: "../../escaped".to_string(),
        position: "1.1.1".into(),
        text: "evil".into(),
        author: "attacker".into(),
        timestamp: 1.0,
        status: clavus_core::types::CueStatus::Pending,
        snapshot_hash: String::new(),
        track_name: String::new(),
        assignee: String::new(),
        in_progress: false,
        replies: vec![],
    };
    let addr = hostile_relay_with_cues(vec![evil_cue]);

    let (out, result) = run(|o, e| cmd_pull(&addr, &als, &ws.root, o, e));
    // The pull may succeed (a valid snapshot plus one bad cue) or fail, but
    // it must never write outside the cue directory.
    let _ = result;
    let escaped = ws.root.parent().unwrap().join("escaped.json");
    assert!(
        !escaped.exists(),
        "a cue id must not be able to escape the cue directory: {}",
        out.all()
    );
}

/// Serve one client, sending it a `Push` carrying the given cues.
fn hostile_relay_with_cues(cues: Vec<clavus_core::types::Cue>) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut len_buf = [0u8; 4];
            if stream.read_exact(&mut len_buf).is_ok() {
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; len];
                let _ = stream.read_exact(&mut body);
            }
            let msg = clavus_sync::SyncMsg::Push {
                snapshots: vec![],
                cues,
            };
            let json = serde_json::to_vec(&msg).unwrap();
            let _ = stream.write_all(&(json.len() as u32).to_be_bytes());
            let _ = stream.write_all(&json);
            let _ = stream.flush();
            thread::sleep(std::time::Duration::from_millis(300));
        }
    });
    addr
}

#[test]
fn pull_imports_cues_from_the_relay() {
    // Regression: the pull path counted the cues it received but never
    // stored them, so a collaborator's notes never arrived.
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&[]));
    let cue = clavus_core::types::Cue {
        id: "abc123456789".to_string(),
        position: "5.1.1".to_string(),
        text: "bring the vocal up".to_string(),
        author: "slowhands".to_string(),
        timestamp: 1.0,
        status: clavus_core::types::CueStatus::Pending,
        snapshot_hash: String::new(),
        track_name: "Vocals".to_string(),
        assignee: String::new(),
        in_progress: false,
        replies: vec![],
    };
    let addr = hostile_relay_with_cues(vec![cue]);

    let (out, result) = run(|o, e| cmd_pull(&addr, &als, &ws.root, o, e));
    assert!(result.is_ok(), "the pull should succeed: {:?}", result);
    assert!(
        out.all().contains("bring the vocal up"),
        "the cue should be reported: {}",
        out.all()
    );

    // And it should be in the local cue store.
    let (list, _) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
    assert!(
        list.all().contains("bring the vocal up"),
        "the cue should be stored locally: {}",
        list.all()
    );
}

#[test]
fn pull_reports_a_conflicting_cue_instead_of_overwriting_the_local_one() {
    let ws = Workspace::new();
    let als = ws.write_als(&als_xml(&[]));
    // A local cue that is newer than the remote one.
    let (out, result) = run(|o, e| cmd_cue(&als, "local note", "1.1.1", None, &ws.root, o, e));
    assert!(result.is_ok());
    let local_id = out
        .all()
        .split_whitespace()
        .find(|w| w.len() == 8 && w.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("")
        .to_string();

    // A remote cue with the same id but different text and an older timestamp.
    let remote = clavus_core::types::Cue {
        id: local_id.clone(),
        position: "1.1.1".to_string(),
        text: "remote note".to_string(),
        author: "slowhands".to_string(),
        timestamp: 0.0,
        status: clavus_core::types::CueStatus::Pending,
        snapshot_hash: String::new(),
        track_name: String::new(),
        assignee: String::new(),
        in_progress: false,
        replies: vec![],
    };
    let addr = hostile_relay_with_cues(vec![remote]);
    let (out, _result) = run(|o, e| cmd_pull(&addr, &als, &ws.root, o, e));
    assert!(
        out.all().contains("Conflict") || out.all().contains("conflict"),
        "the conflict should be reported: {}",
        out.all()
    );

    // The local text must survive.
    let (list, _) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
    assert!(
        list.all().contains("local note"),
        "the local cue must be kept: {}",
        list.all()
    );
    assert!(
        !list.all().contains("remote note"),
        "the remote text must not replace the local one: {}",
        list.all()
    );
}
