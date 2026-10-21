//! Regression tests for the defects the beta agents found.
//!
//! Each test here corresponds to a finding from the AI beta wave. The
//! tests drive the real command implementations against real files.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clavus_cli::{
    cmd_cue, cmd_cues, cmd_log, cmd_pull, cmd_push, cmd_relay, cmd_restore, cmd_snapshot,
    cmd_status,
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
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tempo Value="128" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack><Name Value="{}" /><DeviceChain /></AudioTrack>
</LiveSet>"#,
        name
    )
}

/// Write a gzip-compressed `.als` file.
fn write_als(path: &Path, xml: &str) {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut enc, xml.as_bytes()).expect("gzip");
    fs::write(path, enc.finish().expect("gzip finish")).expect("write");
}

/// A project with one present sample.
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

    fn als(&self, name: &str) -> PathBuf {
        let path = self.dir.join(name);
        write_als(&path, &als_xml(name));
        path
    }
}

// ─── A nonexistent project is an error, not an empty project ──────────────
//
// Regression: `log` said "No snapshots yet", `cues` said "No cues" and
// `cue` cheerfully created a cue for a path that did not exist, while
// `audit`, `status` and `snapshot` reported the missing file as an error.

#[test]
fn log_on_a_missing_project_is_an_error() {
    let ws = Workspace::new();
    let missing = ws.dir.join("nope.als");
    let (out, result) = run(|o, e| cmd_log(&missing, &ws.root, o, e));
    assert!(result.is_err(), "log must fail: {}", out.all());
    assert!(
        result.unwrap_err().to_string().contains("does not exist"),
        "the error must say the project is missing"
    );
}

#[test]
fn cues_on_a_missing_project_is_an_error() {
    let ws = Workspace::new();
    let missing = ws.dir.join("nope.als");
    let (out, result) = run(|o, e| cmd_cues(&missing, None, &ws.root, o, e));
    assert!(result.is_err(), "cues must fail: {}", out.all());
}

#[test]
fn cue_on_a_missing_project_is_an_error() {
    let ws = Workspace::new();
    let missing = ws.dir.join("nope.als");
    let (out, result) = run(|o, e| cmd_cue(&missing, "note", "1.1.1", None, None, &ws.root, o, e));
    assert!(result.is_err(), "cue must fail: {}", out.all());
    // No cue store may be created for a project that does not exist.
    let cue_dir = ws.root.join("cues");
    assert!(
        !cue_dir.exists(),
        "no cue store may be created: {}",
        cue_dir.display()
    );
}

// ─── A cue position must be a position ────────────────────────────────────
//
// Regression: `cue -p notaposition` stored the nonsense and reported
// success. A position that cannot be parsed is a cue that can never be
// found in the DAW.

#[test]
fn cue_with_a_nonsense_position_is_rejected() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");
    for bad in ["notaposition", "1.1", "1.1.1.1", "a.b.c", ""] {
        let (out, result) = run(|o, e| cmd_cue(&als, "note", bad, None, None, &ws.root, o, e));
        assert!(
            result.is_err(),
            "the position {:?} must be refused: {}",
            bad,
            out.all()
        );
    }
}

#[test]
fn cue_with_a_good_position_is_accepted() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");
    let (out, result) = run(|o, e| cmd_cue(&als, "note", "12.3.5", None, None, &ws.root, o, e));
    assert!(result.is_ok(), "a valid position must work: {}", out.all());
}

// ─── A bad status filter names the argument, not the project ──────────────

#[test]
fn a_bad_cue_status_names_the_filter() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");
    let (out, result) = run(|o, e| cmd_cues(&als, Some("bogus"), &ws.root, o, e));
    assert!(result.is_err(), "a bad filter must fail: {}", out.all());
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("unknown cue status"),
        "the error must name the status: {}",
        message
    );
    assert!(
        !message.contains("Failed to parse project file"),
        "the error must not blame the project: {}",
        message
    );
}

// ─── A failed push must fail ──────────────────────────────────────────────
//
// Regression: a push that timed out or hit a conflict printed a warning
// and exited 0, so a script could not tell that nothing was pushed.

#[test]
fn a_push_that_gets_no_answer_fails() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some("v1".into()), false, &ws.root, o, e));
    assert!(result.is_ok(), "snapshot: {}", out.all());

    // A port that nothing listens on.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let relay = format!("127.0.0.1:{}", port);

    let (out, result) = run(|o, e| cmd_push(&relay, &als, &ws.root, o, e));
    assert!(
        result.is_err(),
        "a push that was never acknowledged must fail: {}",
        out.all()
    );
}

// ─── A pull that got no answer must fail ──────────────────────────────────

#[test]
fn a_pull_that_gets_no_answer_fails() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some("v1".into()), false, &ws.root, o, e));
    assert!(result.is_ok(), "snapshot: {}", out.all());

    // A server that accepts the connection and never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((sock, _)) = listener.accept() {
            // Accept the connection and never answer.
            std::thread::sleep(Duration::from_secs(3));
            drop(sock);
        }
    });

    let relay = format!("127.0.0.1:{}", port);
    let (out, result) = run(|o, e| cmd_pull(&relay, &als, &ws.root, o, e));
    assert!(
        result.is_err(),
        "a pull that was never answered must fail: {}",
        out.all()
    );
    assert!(
        !out.stdout.contains("Already up to date"),
        "the tool must not claim to be up to date: {}",
        out.stdout
    );
}

// ─── The relay says it is listening only when it is ───────────────────────

#[test]
fn a_relay_that_cannot_bind_does_not_claim_to_listen() {
    let ws = Workspace::new();
    // Hold the port so the relay cannot have it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bind = format!("127.0.0.1:{}", port);

    let (out, result) = run(|o, e| cmd_relay(&bind, None, &ws.root, o, e));
    assert!(result.is_err(), "the relay must fail: {}", out.all());
    assert!(
        !out.stdout.contains("Listening on"),
        "the tool must not claim to be listening: {}",
        out.stdout
    );
}

// ─── A read-only command creates nothing ──────────────────────────────────

#[test]
fn a_read_only_command_creates_no_data_directory() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("newdata");
    let project = tmp.path().join("Alpha.als");
    write_als(&project, &als_xml("Alpha"));

    let (out, result) = run(|o, e| cmd_log(&project, &root, o, e));
    assert!(result.is_ok(), "log: {}", out.all());
    assert!(
        !root.exists(),
        "a read must not create the data directory: {}",
        root.display()
    );

    let (out, result) = run(|o, e| cmd_cues(&project, None, &root, o, e));
    assert!(result.is_ok(), "cues: {}", out.all());
    assert!(
        !root.exists(),
        "a read must not create the data directory: {}",
        root.display()
    );

    let (out, result) = run(|o, e| cmd_status(&project, &root, o, e));
    assert!(result.is_ok(), "status: {}", out.all());
    assert!(
        !root.exists(),
        "a read must not create the data directory: {}",
        root.display()
    );
}

// ─── Two projects with the same file name are two projects ────────────────

#[test]
fn two_projects_with_the_same_file_name_have_separate_chains() {
    let ws = Workspace::new();
    let a_dir = ws.dir.join("A");
    let b_dir = ws.dir.join("B");
    fs::create_dir_all(&a_dir).unwrap();
    fs::create_dir_all(&b_dir).unwrap();
    let a = a_dir.join("Song.als");
    let b = b_dir.join("Song.als");
    write_als(&a, &als_xml("KickA"));
    write_als(&b, &als_xml("KickB"));

    let (out, result) = run(|o, e| cmd_snapshot(&a, &Some("A1".into()), false, &ws.root, o, e));
    assert!(result.is_ok(), "snapshot A: {}", out.all());
    let (out, result) = run(|o, e| cmd_snapshot(&b, &Some("B1".into()), false, &ws.root, o, e));
    assert!(result.is_ok(), "snapshot B: {}", out.all());

    // Each project sees only its own snapshot.
    let (out, _) = run(|o, e| cmd_log(&a, &ws.root, o, e));
    let log_a = out.stdout.clone();
    assert!(log_a.contains("A1"), "A's log: {}", log_a);
    assert!(!log_a.contains("B1"), "A must not see B's snapshot: {}", log_a);

    let (out, _) = run(|o, e| cmd_log(&b, &ws.root, o, e));
    let log_b = out.stdout.clone();
    assert!(log_b.contains("B1"), "B's log: {}", log_b);
    assert!(!log_b.contains("A1"), "B must not see A's snapshot: {}", log_b);

    // Restoring A must not write B's content over A.
    let (out, result) = run(|o, e| cmd_restore(&a, "HEAD", &ws.root, o, e));
    assert!(result.is_ok(), "restore A: {}", out.all());
    let restored = fs::read(&a).unwrap();
    let (xml, _) = clavus_core::sample::decompress_als_bytes(&restored).unwrap();
    let xml = String::from_utf8_lossy(&xml);
    assert!(
        xml.contains("KickA"),
        "A must keep its own content: {}",
        xml
    );
}

// ─── A snapshot of one kind is not restored over another ──────────────────

#[test]
fn a_bwproject_snapshot_is_not_restored_over_an_als() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some("v1".into()), false, &ws.root, o, e));
    assert!(result.is_ok(), "snapshot: {}", out.all());

    // A Bitwig bundle with the same stem is a different project.
    let bundle = ws.dir.join("Alpha.bwproject");
    {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("project.xml", opts).unwrap();
        w.write_all(b"<Project Name=\"Test\"/>").unwrap();
        let cursor = w.finish().unwrap();
        fs::write(&bundle, cursor.into_inner()).unwrap();
    }

    let (out, result) = run(|o, e| cmd_restore(&bundle, "HEAD", &ws.root, o, e));
    assert!(
        result.is_err(),
        "a bundle must not be restored over a set with no history: {}",
        out.all()
    );
    assert!(
        fs::read(&bundle).unwrap().starts_with(b"PK"),
        "the bundle must be untouched"
    );
}

// ─── An unchanged snapshot says so ─────────────────────────────────────────

#[test]
fn an_unchanged_snapshot_says_so() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some("v1".into()), false, &ws.root, o, e));
    assert!(result.is_ok(), "snapshot: {}", out.all());
    assert!(out.stdout.contains("✓"), "the first snapshot is new: {}", out.stdout);

    // The same bytes again: nothing new is recorded.
    let (out, result) = run(|o, e| cmd_snapshot(&als, &Some("v2".into()), false, &ws.root, o, e));
    assert!(result.is_ok(), "snapshot: {}", out.all());
    assert!(
        out.stdout.contains("unchanged"),
        "an unchanged project must say so: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("✓ snapshot"),
        "nothing new was created: {}",
        out.stdout
    );

    // The chain still has one snapshot, and the message was not recorded.
    let (out, _) = run(|o, e| cmd_log(&als, &ws.root, o, e));
    assert!(out.stdout.contains("1 snapshot(s)"), "log: {}", out.stdout);
    assert!(!out.stdout.contains("v2"), "the message was not stored: {}", out.stdout);
}

// ─── A note is signed by its author, not by the tool ──────────────────────
//
// Regression: every cue was authored by the constant "cli", so after a
// sync a collaborator could not tell whose notes were whose — which is
// the whole point of sharing notes.

#[test]
fn a_cue_records_its_author() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");

    let (out, result) = run(|o, e| {
        cmd_cue(
            &als,
            "bring the bass in",
            "5.1.1",
            None,
            Some("dana"),
            &ws.root,
            o,
            e,
        )
    });
    assert!(result.is_ok(), "the cue must be added: {}", out.all());

    let (out, _) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
    assert!(
        out.all().contains("dana"),
        "the author must be shown: {}",
        out.all()
    );
}

#[test]
fn a_cue_without_an_author_uses_the_user_name() {
    let ws = Workspace::new();
    let als = ws.als("Alpha.als");

    // The author falls back to the environment, and then to "cli".
    std::env::set_var("CLAVUS_AUTHOR", "river");
    let (out, result) = run(|o, e| cmd_cue(&als, "note", "1.1.1", None, None, &ws.root, o, e));
    assert!(result.is_ok(), "the cue must be added: {}", out.all());
    std::env::remove_var("CLAVUS_AUTHOR");

    let (out, _) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
    assert!(
        out.all().contains("river"),
        "the author must come from the environment: {}",
        out.all()
    );
}

// ─── A push sends the cues even when there is no snapshot ────────────────
//
// Regression: a push returned early when the project had no local
// snapshots, so a collaborator who only left notes never sent them.
// The note silently never left the machine.

#[test]
fn a_push_sends_the_cues_of_a_project_without_snapshots() {
    let ws = Workspace::new();
    let als = ws.als("Notes.als");
    let (out, result) = run(|o, e| cmd_cue(&als, "only a note", "1.1.1", None, None, &ws.root, o, e));
    assert!(result.is_ok(), "the cue must be added: {}", out.all());

    // A relay that accepts the connection but never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    std::thread::spawn(move || {
        if let Ok((sock, _)) = listener.accept() {
            std::thread::sleep(Duration::from_secs(3));
            drop(sock);
        }
    });

    let (out, result) = run(|o, e| cmd_push(&addr, &als, &ws.root, o, e));
    // The push itself cannot complete against a silent relay, but the
    // command must have tried to send the cue rather than bailing out
    // with "no snapshots to push".
    assert!(
        out.all().contains("pushing the cues only"),
        "the push must continue to the cues: {}",
        out.all()
    );
    assert!(
        out.all().contains("1 cue(s)"),
        "the cue must be sent: {}",
        out.all()
    );
    let _ = result;
}

// ─── A pulled snapshot can be restored on the other machine ───────────────
//
// Regression: the sync only carried metadata, so a collaborator could
// see that a snapshot existed but could not restore it on their own
// machine. The project bytes now travel with the snapshot.

#[test]
fn a_pulled_snapshot_can_be_restored_on_the_other_machine() {
    use std::net::TcpListener;

    // A relay on a free port.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let relay_dir = TempDir::new().expect("relay dir");

    let relay = std::thread::spawn(move || {
        let _ = clavus_relay::run_with_listener(listener, relay_dir.path().to_path_buf());
    });
    // Wait until the relay accepts connections.
    for _ in 0..200 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // ── Machine A: snapshot, push ──
    let ws_a = Workspace::new();
    let als_a = ws_a.als("Alpha.als");
    let (out, result) = run(|o, e| cmd_snapshot(&als_a, &Some("machine A v1".to_string()), false, &ws_a.root, o, e));
    assert!(result.is_ok(), "the snapshot must be taken: {}", out.all());

    let (out, result) = run(|o, e| cmd_push(&addr, &als_a, &ws_a.root, o, e));
    assert!(result.is_ok(), "the push must work: {}", out.all());
    assert!(
        out.all().contains("sent the project bytes"),
        "the push must send the bytes: {}",
        out.all()
    );

    // ── Machine B: pull, then restore ──
    let ws_b = Workspace::new();
    let als_b = ws_b.als("Alpha.als");

    let (out, result) = run(|o, e| cmd_pull(&addr, &als_b, &ws_b.root, o, e));
    assert!(result.is_ok(), "the pull must work: {}", out.all());
    assert!(
        out.all().contains("came with the project bytes"),
        "the pull must bring the bytes: {}",
        out.all()
    );

    // The pulled snapshot can be restored over B's own project.
    let (out, result) = run(|o, e| cmd_restore(&als_b, "HEAD", &ws_b.root, o, e));
    assert!(result.is_ok(), "the restore must work: {}", out.all());

    // And the restored file is A's file, byte for byte.
    let restored = fs::read(&als_b).expect("the restored file");
    let original = fs::read(&als_a).expect("the original file");
    assert_eq!(
        restored, original,
        "the restored project must be the collaborator's file"
    );

    drop(relay);
}
