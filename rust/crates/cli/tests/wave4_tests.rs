//! Regression tests for the fourth beta wave's findings.
//!
//! Each test here reproduces a defect an AI beta agent found in the
//! compiled binary. They drive the real command implementations, real
//! files and a real relay — nothing is re-implemented on the test side.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clavus_cli::{
    cmd_audit, cmd_collect, cmd_cue, cmd_cues, cmd_log, cmd_pull, cmd_push, cmd_restore,
    cmd_snapshot,
};
use common::{als_version, run, write_als};
use tempfile::TempDir;

/// A workspace whose projects live under the home directory.
///
/// The folder path of a project — the second half of its identity on
/// the wire — is the path below the home directory, so a test that
/// wants the behaviour a user's projects have has to put them there.
struct HomeWorkspace {
    root: PathBuf,
    dir: PathBuf,
    home: PathBuf,
}

impl HomeWorkspace {
    fn new() -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let unique = format!(
            ".clavus-wave4-{}-{}",
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
            root: base.join(".clavus"),
            dir,
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

/// A workspace outside the home directory.
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

    /// A folder outside the project folder.
    fn outside(&self) -> PathBuf {
        self._tmp.path().join("outside")
    }
}

/// Make a directory read-only, or writable again.
fn set_read_only(dir: &Path, read_only: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if read_only { 0o555 } else { 0o755 };
    fs::set_permissions(dir, fs::Permissions::from_mode(mode)).expect("chmod");
}

/// Start a real relay on a free port.
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

// ─── A1: a restore lands in the file the symlink points at ──────────────
//
// Regression: the restore wrote a temporary file and renamed it over
// the project path. A project that is a symlink was replaced by a
// regular copy, and the file the user actually keeps — the target —
// was never restored. The tool said it had restored the project.

#[test]
fn a_restore_lands_in_the_file_a_symlink_points_at() {
    let ws = Workspace::new();
    let real = ws.als_in("links", "Real.als");
    let link = ws.dir.join("links").join("Sym.als");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    let (out, result) = run(|o, e| {
        cmd_snapshot(&link, &Some("v1".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());

    // The project changes after the snapshot.
    write_als(&real, &als_version("newer", 140));

    let (out, result) = run(|o, e| cmd_restore(&link, "HEAD", &ws.root, o, e));
    assert!(result.is_ok(), "restore: {}", out.all());

    // The link must still be a link.
    let meta = fs::symlink_metadata(&link).expect("the link");
    assert!(
        meta.file_type().is_symlink(),
        "the restore must not replace the user's symlink with a copy"
    );

    // And the file the link points at must hold the restored bytes.
    let restored = fs::read(&real).expect("the target");
    let expected = fs::read(ws.dir.join("links").join("Real.als")).expect("the target");
    assert_eq!(
        restored, expected,
        "the target must hold the restored bytes"
    );
    let text = String::from_utf8_lossy(&decompressed(&restored)).to_string();
    assert!(
        !text.contains("newer"),
        "the target must hold the snapshot, not the newer file: {}",
        text
    );
}

/// The plain XML of a gzipped `.als`.
fn decompressed(bytes: &[u8]) -> Vec<u8> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut out = Vec::new();
    GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .expect("the snapshot is gzipped");
    out
}

// ─── A2: a pull must not hand over another project's history ────────────
//
// Regression: the relay fell back to "the only project with that
// name", so a project in `sep/p2/studio` was served the history and
// the private notes of the project in `sep/p1/studio`.

#[test]
fn a_pull_does_not_hand_over_another_projects_history() {
    let (addr, _dir, relay) = start_relay();
    let ws = HomeWorkspace::new();

    // ── The first project: its own history and its own note ──
    let p1 = ws.als_in("sep/p1/studio", "Song.als");
    let (out, result) = run(|o, e| {
        cmd_snapshot(&p1, &Some("P1 ONLY".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());
    let (out, result) = run(|o, e| cmd_cue(&p1, "p1 private note", "1.1.1", None, None, &ws.root, o, e));
    assert!(result.is_ok(), "cue: {}", out.all());
    let (out, result) = run(|o, e| cmd_push(&addr, &p1, &ws.root, o, e));
    assert!(result.is_ok(), "push: {}", out.all());

    // ── The second project: same file name, different folder, and
    //    different bytes — a different project, not a copy ──
    let p2 = ws.als_in("sep/p2/studio", "Song.als");
    write_als(&p2, &als_version("the other song", 140));
    let (out, result) = run(|o, e| cmd_pull(&addr, &p2, &ws.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());
    assert!(
        !out.all().contains("P1 ONLY"),
        "the other project's history must not be handed over: {}",
        out.all()
    );

    // The log must stay empty.
    let (out, result) = run(|o, e| cmd_log(&p2, &ws.root, o, e));
    assert!(result.is_ok(), "log: {}", out.all());
    assert!(
        !out.all().contains("P1 ONLY"),
        "the other project's snapshot must not be in the log: {}",
        out.all()
    );

    // And the other project's note must not be in the cue list.
    let (out, result) = run(|o, e| cmd_cues(&p2, None, &ws.root, o, e));
    assert!(result.is_ok(), "cues: {}", out.all());
    assert!(
        !out.all().contains("p1 private note"),
        "the other project's note must not be imported: {}",
        out.all()
    );

    drop(relay);
}

// ─── A3: a push reports the relay's counts for its own project ──────────
//
// Regression: the counts came from the relay's answer, which the relay
// had produced for a *different* project.

#[test]
fn a_push_reports_the_relays_counts_for_its_own_project() {
    let (addr, _dir, relay) = start_relay();
    let ws = HomeWorkspace::new();

    let p1 = ws.als_in("sep/p1/studio", "Song.als");
    let (out, result) = run(|o, e| {
        cmd_snapshot(&p1, &Some("P1 ONLY".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());
    let (out, result) = run(|o, e| cmd_cue(&p1, "p1 private note", "1.1.1", None, None, &ws.root, o, e));
    assert!(result.is_ok(), "cue: {}", out.all());
    let (out, result) = run(|o, e| cmd_push(&addr, &p1, &ws.root, o, e));
    assert!(result.is_ok(), "push: {}", out.all());

    // A different project with the same file name pushes nothing yet.
    let p2 = ws.als_in("sep/p2/studio", "Song.als");
    write_als(&p2, &als_version("the other song", 140));
    let (out, result) = run(|o, e| {
        cmd_snapshot(&p2, &Some("P2".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());
    let (out, result) = run(|o, e| cmd_push(&addr, &p2, &ws.root, o, e));
    assert!(result.is_ok(), "push: {}", out.all());
    assert!(
        out.all().contains("relay has 0 snapshot(s), 0 cue(s)"),
        "the counts must be this project's, not another one's: {}",
        out.all()
    );

    drop(relay);
}

// ─── A4: a path with an XML entity is rewritten ─────────────────────────
//
// Regression: the rewrite looked for the raw path, but the XML holds
// it escaped (`R&amp;B/kick.wav`). Nothing was rewritten, and the
// tool still said the paths were rewritten.

#[test]
fn a_collect_rewrites_a_reference_whose_path_has_an_xml_entity() {
    let ws = Workspace::new();
    let outside = ws.outside().join("amp out").join("R&B");
    fs::create_dir_all(&outside).expect("outside folder");
    let sample = outside.join("kick.wav");
    fs::write(&sample, b"RIFF....fake wave data").expect("the sample");

    let project = ws.dir.join("Amp.als");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <AudioTrack><Name Value="Drums" /><DeviceChain /></AudioTrack>
  <SampleRef><Path Value="{}" /></SampleRef>
</LiveSet>"#,
        sample.to_string_lossy().replace('&', "&amp;")
    );
    write_als(&project, &xml);

    // The audit must find the sample: the path is escaped in the XML
    // and has to be decoded to be looked for.
    let (out, result) = run(|o, e| cmd_audit(&project, false, o, e));
    assert!(result.is_ok(), "audit: {}", out.all());
    assert!(
        out.all().contains("R&B/kick.wav"),
        "the escaped path must be decoded: {}",
        out.all()
    );
    assert!(
        !out.all().contains("R&amp;B"),
        "the raw entity must never be shown as a path: {}",
        out.all()
    );

    let (out, result) = run(|o, e| cmd_collect(&project, &ws.root, o, e));
    assert!(result.is_ok(), "collect: {}", out.all());

    // The project must now point at the copy inside the project.
    let after = decompressed(&fs::read(&project).expect("the project"));
    let text = String::from_utf8_lossy(&after).to_string();
    assert!(
        text.contains("Samples/kick.wav"),
        "the reference must be rewritten: {}",
        text
    );
    assert!(
        !text.contains("R&amp;B"),
        "the outside path must be gone: {}",
        text
    );

    // And the audit must be clean afterwards.
    let (out, result) = run(|o, e| cmd_audit(&project, false, o, e));
    assert!(result.is_ok(), "audit: {}", out.all());
    assert!(
        out.all().contains("accounted for"),
        "the project must be clean afterwards: {}",
        out.all()
    );
}

// ─── A5: a Bitwig sample path is decoded like an Ableton one ────────────
//
// Regression: the Ableton parser decoded the XML entities and the
// Bitwig parser did not, so a sample that was really on disk was
// reported missing and the collect refused it.

#[test]
fn a_bitwig_sample_path_with_an_xml_entity_is_found_and_collected() {
    let ws = Workspace::new();
    let outside = ws.outside().join("amp out").join("R&B");
    fs::create_dir_all(&outside).expect("outside folder");
    let sample = outside.join("kick.wav");
    fs::write(&sample, b"RIFF....fake wave data").expect("the sample");

    let project = ws.dir.join("Amp.bwproject");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" VersionMinor="1" Name="Amp">
  <Tracks><Track Type="Audio" Index="0"><Name Value="Drums" /></Track></Tracks>
  <Tempo Bpm="128.0" />
  <AudioFiles><File Path="{}" /></AudioFiles>
</Project>"#,
        sample.to_string_lossy().replace('&', "&amp;")
    );
    {
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;
        use std::io::Write;
        let file = fs::File::create(&project).expect("the bundle");
        let mut zip = ZipWriter::new(file);
        zip.start_file("project.xml", SimpleFileOptions::default())
            .expect("start");
        zip.write_all(xml.as_bytes()).expect("write");
        zip.finish().expect("finish");
    }

    let (out, result) = run(|o, e| cmd_audit(&project, false, o, e));
    assert!(result.is_ok(), "audit: {}", out.all());
    assert!(
        !out.all().contains("missing"),
        "the sample is on disk and must not be reported missing: {}",
        out.all()
    );
    assert!(
        out.all().contains("R&B/kick.wav"),
        "the escaped path must be decoded: {}",
        out.all()
    );

    // The collect must succeed: the sample goes into the bundle.
    let (out, result) = run(|o, e| cmd_collect(&project, &ws.root, o, e));
    assert!(result.is_ok(), "collect: {}", out.all());
    assert!(
        out.all().contains("into the project bundle"),
        "the sample must go into the bundle: {}",
        out.all()
    );

    // The bundle carries the sample and points at it.
    let names = {
        use zip::ZipArchive;
        let file = fs::File::open(&project).expect("the bundle");
        let mut archive = ZipArchive::new(file).expect("zip");
        let mut names = Vec::new();
        for i in 0..archive.len() {
            names.push(archive.by_index(i).expect("entry").name().to_string());
        }
        names
    };
    assert!(
        names.iter().any(|n| n == "samples/kick.wav"),
        "the bundle must carry the sample: {:?}",
        names
    );

    // And no folder was created next to the bundle.
    assert!(
        !ws.dir.join("Samples").exists(),
        "the bundle must stay self-contained"
    );
}

// ─── A6: a relay that cannot write its state says so ───────────────────
//
// Regression: the relay accepted a push, told the client it was
// saved, and lost the history on the next restart.

#[test]
fn a_relay_refuses_to_start_with_a_state_directory_it_cannot_write() {
    use std::net::TcpListener;
    let tmp = TempDir::new().expect("temp dir");
    let state = tmp.path().join("state");
    fs::create_dir_all(&state).expect("state dir");
    set_read_only(&state, true);

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let result = clavus_relay::run_with_listener(listener, state.clone());

    // Make the directory writable again so the temp dir can be removed.
    set_read_only(&state, false);

    assert!(
        result.is_err(),
        "a relay that cannot write its state must not start"
    );
}

#[test]
fn a_relay_that_cannot_save_reports_the_failure_instead_of_acknowledging() {
    let (addr, dir, relay) = start_relay();
    let ws = Workspace::new();
    let project = ws.als_in("p", "Song.als");
    let (out, result) = run(|o, e| {
        cmd_snapshot(&project, &Some("v1".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());

    // The relay's state directory becomes read-only: it can no longer
    // write what it accepts.
    let state = dir.path().to_path_buf();
    set_read_only(&state, true);

    let (out, result) = run(|o, e| cmd_push(&addr, &project, &ws.root, o, e));
    assert!(
        result.is_err(),
        "a push that was not saved must not be reported as a success: {}",
        out.all()
    );
    assert!(
        out.all().to_lowercase().contains("the relay reported a problem"),
        "the failure must be reported: {}",
        out.all()
    );
    assert!(
        !out.all().contains("✓ Pushed"),
        "the push must not be reported as sent: {}",
        out.all()
    );

    // Make the directory writable again so the temp dir can be removed.
    set_read_only(&state, false);

    drop(relay);
}

// ─── A7: a note that cannot be read is named, not swallowed ─────────────
//
// Regression: one unreadable note file made the push send *no* notes
// and print "✓ Pushed".

#[test]
fn a_push_names_the_note_it_could_not_read() {
    let (addr, _dir, relay) = start_relay();
    let ws = Workspace::new();
    let project = ws.als_in("p", "Song.als");
    let (out, result) = run(|o, e| {
        cmd_snapshot(&project, &Some("v1".to_string()), false, &ws.root, o, e)
    });
    assert!(result.is_ok(), "snapshot: {}", out.all());
    let (out, result) = run(|o, e| cmd_cue(&project, "my note", "1.1.1", None, None, &ws.root, o, e));
    assert!(result.is_ok(), "cue: {}", out.all());

    // A second, unreadable note file lands in the same folder.
    let id = clavus_core::project::ProjectId::new(&project);
    let cues_dir = ws.root.join("cues").join(&id.key);
    fs::create_dir_all(&cues_dir).expect("cues dir");
    fs::write(cues_dir.join("broken.json"), b"{ this is not json").expect("the broken note");

    let (out, result) = run(|o, e| cmd_push(&addr, &project, &ws.root, o, e));
    assert!(
        result.is_err(),
        "a note that stayed on the machine must not be reported as sent: {}",
        out.all()
    );
    assert!(
        out.all().contains("broken"),
        "the unreadable note must be named: {}",
        out.all()
    );
    // The notes that *could* be read still left the machine.
    let other = Workspace::new();
    let other_project = other.als_in("p", "Song.als");
    let (out, result) = run(|o, e| cmd_pull(&addr, &other_project, &other.root, o, e));
    assert!(result.is_ok(), "pull: {}", out.all());
    let (out, result) = run(|o, e| cmd_cues(&other_project, None, &other.root, o, e));
    assert!(result.is_ok(), "cues: {}", out.all());
    assert!(
        out.all().contains("my note"),
        "the readable note must be on the relay: {}",
        out.all()
    );
    assert!(
        !out.all().contains("broken"),
        "the broken note must not be on the relay: {}",
        out.all()
    );

    drop(relay);
}

// ─── A8: an ambiguous prefix is refused wherever it is found ────────────
//
// Regression: a prefix that matched two snapshots of the chain was
// resolved silently, while the same prefix off the chain was refused.

#[test]
fn an_ambiguous_prefix_on_the_chain_is_refused() {
    use clavus_core::blob::BlobStore;
    use clavus_core::project::ProjectId;
    use clavus_core::snapshot::SnapshotStore;
    use clavus_core::types::Snapshot;

    let ws = Workspace::new();
    let project = ws.als_in("p", "A.als");
    let id = ProjectId::new(&project);
    let store = BlobStore::new(ws.root.join("blobs")).expect("blob store");
    let snap_store = SnapshotStore::new(store, &id);

    // A chain of three snapshots whose hashes share their first
    // character — the two a user would type as "d".
    let hashes = [
        format!("d{:063x}", 0x5c12a79u64),
        format!("d{:063x}", 0x7431b56u64),
        format!("d{:063x}", 0xa11ce77u64),
    ];
    let mut parent: Option<String> = None;
    for (index, hash) in hashes.iter().enumerate() {
        let snapshot = Snapshot {
            hash: hash.to_string(),
            parent: parent.clone(),
            timestamp: 1_700_000_000.0 + index as f64,
            message: format!("s{}", index + 1),
            track_count: 1,
            bpm: 120.0,
            tags: vec![],
            notes: String::new(),
            sample_hashes: vec![],
            sample_paths: std::collections::HashMap::new(),
            project_path: project.to_string_lossy().to_string(),
            project_folder: id.folder_path().to_string(),
        };
        snap_store.save_raw(snapshot).expect("save");
        parent = Some(hash.to_string());
    }
    snap_store.set_head(&hashes[2], "test").expect("head");

    // The prefix "d" matches two snapshots of the chain: it must be
    // refused, exactly as it is off the chain.
    let (out, result) = run(|o, e| cmd_restore(&project, "d", &ws.root, o, e));
    assert!(
        result.is_err(),
        "an ambiguous prefix must be refused: {}",
        out.all()
    );
    let message = format!("{:?}", result.expect_err("the restore must fail"));
    assert!(
        message.to_lowercase().contains("ambiguous"),
        "the refusal must say the prefix is ambiguous: {}",
        message
    );
}

// ─── The relay's own project identity ───────────────────────────────────

#[test]
fn a_project_outside_the_home_keeps_the_folder_it_lives_in() {
    let path = Path::new("/Volumes/Studio/Album1/Song.als");
    let home = Path::new("/Users/someone");
    let folder = clavus_core::project::folder_path_of(path, Some(home));
    assert_eq!(
        folder, "Album1",
        "a project outside the home keeps the name of its folder"
    );
}

// ─── The CLI ergonomics agent's findings ───────────────────────────────
//
// The fifth wave's CLI ergonomics persona reported these against the
// compiled binary. Each one is reproduced here through the real
// command implementations.

// ─── B2: a restore brings the audio back ───────────────────────────────
//
// The snapshot recorded the samples it saw and their hashes, but the
// bytes of the audio were never kept. Deleting a collected sample and
// restoring the project left the project pointing at missing audio,
// and the command said it had restored the project — with no warning
// at all.

#[test]
fn a_restore_brings_the_audio_back() {
    let ws = HomeWorkspace::new();
    let project = ws.als_in("p1", "Song.als");

    // A sample inside the project, referenced by the set.
    let samples = project.parent().unwrap().join("Samples");
    fs::create_dir_all(&samples).expect("samples dir");
    let kick = samples.join("kick.wav");
    fs::write(&kick, b"RIFFfake").expect("sample");

    let als = als_version("Song.als", 128).replace(
        "<DeviceChain />",
        "<DeviceChain /><SampleRef><FileRef><Path Value=\"Samples/kick.wav\" /></FileRef></SampleRef>",
    );
    write_als(&project, &als);

    let (_, res) = run(|out, err| {
        cmd_snapshot(
            &project,
            &Some("with audio".to_string()),
            false,
            &ws.root,
            out,
            err,
        )
    });
    assert!(res.is_ok(), "the snapshot must be taken: {:?}", res);

    // The audio goes away.
    fs::remove_file(&kick).expect("remove sample");
    assert!(!kick.exists());

    let (out, res) = run(|out, err| cmd_restore(&project, "HEAD", &ws.root, out, err));
    assert!(res.is_ok(), "the restore must succeed: {:?}", res);

    assert!(
        kick.exists(),
        "the restore must bring the audio back"
    );
    assert_eq!(
        fs::read(&kick).expect("the sample"),
        b"RIFFfake".to_vec(),
        "the restored audio must be the audio the snapshot recorded"
    );
    assert!(
        out.all().contains("Restored 1 sample"),
        "the restore must say the audio came back: {}",
        out.all()
    );

    fs::remove_dir_all(&ws.home).ok();
}

// ─── B2b: a restore says when the audio is not there ─────────────────────
//
// A snapshot that holds no audio cannot bring it back, and the user
// has to hear it from the tool rather than from Ableton refusing to
// play.

#[test]
fn a_restore_says_when_the_audio_is_not_there() {
    let ws = HomeWorkspace::new();
    let project = ws.als_in("p1", "Song.als");

    // A snapshot with no audio at all: the set points at nothing.
    let (_, res) = run(|out, err| {
        cmd_snapshot(
            &project,
            &Some("no audio".to_string()),
            false,
            &ws.root,
            out,
            err,
        )
    });
    assert!(res.is_ok(), "the snapshot must be taken: {:?}", res);

    // A snapshot whose audio was captured, but whose bytes are gone
    // from the store — the case of a snapshot pulled from a relay that
    // never had them.
    let (out, res) = run(|out, err| cmd_restore(&project, "HEAD", &ws.root, out, err));
    assert!(res.is_ok(), "the restore must succeed: {:?}", res);
    assert!(
        !out.all().contains("are not on disk"),
        "a snapshot with no audio must not claim the audio is missing: {}",
        out.all()
    );

    fs::remove_dir_all(&ws.home).ok();
}

// ─── B4: the collect report matches what it did ────────────────────────
//
// A Bitwig bundle whose samples were already inside it, but whose
// references pointed elsewhere, was collected and the tool said "the
// paths already point at the copies inside it" — right after it had
// rewritten them.

#[test]
fn a_collect_does_not_claim_it_changed_nothing_when_it_rewrote_paths() {
    let ws = HomeWorkspace::new();
    let folder = ws.dir.join("p1");
    fs::create_dir_all(&folder).expect("folder");

    // The bundle carries the sample, but the project description still
    // points at a file that is not there any more — an old path from
    // another machine.
    let gone = ws.home.join("gone").join("kick.wav");
    let project = folder.join("Bundle.bwproject");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" VersionMinor="1" Name="Bundle">
  <Tracks><Track Type="Audio" Index="0"><Name Value="Drums" /></Track></Tracks>
  <Tempo Bpm="128.0" />
  <AudioFiles><File Path="{}" /></AudioFiles>
</Project>"#,
        gone.to_string_lossy()
    );
    {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;
        let file = fs::File::create(&project).expect("the bundle");
        let mut zip = ZipWriter::new(file);
        zip.start_file("project.xml", SimpleFileOptions::default())
            .expect("start");
        zip.write_all(xml.as_bytes()).expect("write");
        zip.start_file("samples/kick.wav", SimpleFileOptions::default())
            .expect("start");
        zip.write_all(b"RIFF....fake wave data").expect("write");
        zip.finish().expect("finish");
    }

    let (out, res) = run(|out, err| cmd_collect(&project, &ws.root, out, err));
    assert!(res.is_ok(), "the collect must succeed: {:?}", res);
    let all = out.all();
    assert!(
        all.contains("already inside the project bundle"),
        "the sample is already inside the bundle: {}",
        all
    );
    assert!(
        !all.contains("already point at the copies inside it"),
        "the report must not claim the paths were already right: {}",
        all
    );
    assert!(
        all.contains("rewritten"),
        "the report must say the paths were rewritten: {}",
        all
    );

    fs::remove_dir_all(&ws.home).ok();
}

// ─── B6: a pull that brings a project writes it ────────────────────────
//
// The pull said the snapshots "can be restored here" while the project
// file was not on the machine at all — and the restore refuses to
// touch a project that does not exist.

#[test]
fn a_pull_writes_the_project_file_when_it_was_not_here() {
    let ws = HomeWorkspace::new();
    let (addr, _relay_dir, _handle) = start_relay();

    // The first machine: a project with a snapshot and its bytes.
    let source = ws.als_in("p1", "Song.als");
    let (_, res) = run(|out, err| {
        cmd_snapshot(&source, &Some("first".to_string()), false, &ws.root, out, err)
    });
    assert!(res.is_ok(), "the snapshot must be taken: {:?}", res);
    let (_, res) = run(|out, err| cmd_push(&addr, &source, &ws.root, out, err));
    assert!(res.is_ok(), "the push must succeed: {:?}", res);

    // The second machine: the same project, in a folder of its own.
    // The folder is not below the first project's folder — a project
    // in the same tree is a different project, however identical the
    // two are.
    let target = ws
        .home
        .parent()
        .unwrap()
        .join(format!("other-machine-{}", std::process::id()))
        .join("elsewhere")
        .join("Song.als");
    fs::create_dir_all(target.parent().unwrap()).expect("folder");
    write_als(&target, &als_version("Song.als", 128));
    fs::remove_file(&target).expect("remove the local copy");

    let (out, res) = run(|out, err| cmd_pull(&addr, &target, &ws.root, out, err));
    assert!(res.is_ok(), "the pull must succeed: {:?}", res);
    let all = out.all();
    assert!(
        all.contains("Wrote the project file") || all.contains("wrote the project file"),
        "the pull must say it wrote the project: {}",
        all
    );
    assert!(
        target.exists(),
        "the project file must exist after the pull"
    );

    fs::remove_dir_all(&ws.home).ok();
    if let Some(parent) = target.parent().and_then(|p| p.parent()) {
        fs::remove_dir_all(parent).ok();
    }
}

// ─── B7: the audit's help says what --strict does ──────────────────────
//
// The help said "--strict: Fail fast if any sample is missing". It
// does not fail fast, and it also fails when a sample lies outside
// the project folder.

#[test]
fn the_audit_help_says_what_strict_does() {
    use clap::CommandFactory;
    let mut command = clavus_cli::Cli::command();
    let audit = command
        .get_subcommands_mut()
        .find(|c| c.get_name() == "audit")
        .expect("the audit command");
    let help = audit.render_long_help().to_string();
    assert!(
        help.contains("outside"),
        "the help must say that --strict also fails on outside samples: {}",
        help
    );
    assert!(
        !help.contains("Fail fast"),
        "the help must not promise to fail fast: {}",
        help
    );
}

// ─── B9: a status that cannot read the project says nothing ─────────────
//
// The status printed its header and the project name, and only then
// failed to parse the file. The output looked like a report of a
// project that had been read.

#[test]
fn a_status_that_cannot_read_the_project_prints_no_report() {
    let ws = HomeWorkspace::new();
    let project = ws.dir.join("p1").join("Broken.als");
    fs::create_dir_all(project.parent().unwrap()).expect("folder");
    fs::write(&project, b"this is not a Live set").expect("write");

    let (out, res) = run(|out, err| {
        clavus_cli::cmd_status(&project, &ws.root, out, err)
    });
    assert!(res.is_err(), "the status must fail: {:?}", res);
    assert!(
        !out.stdout.contains("Ableton Live"),
        "the status must not print a report it could not produce: {}",
        out.stdout
    );

    fs::remove_dir_all(&ws.home).ok();
}

// ─── B3: the projects directory finds the project ───────────────────────
//
// `clavus --dir ~/projects log Alpha.als` did not find the project:
// the flag was ignored for every command that takes a project.

#[test]
fn the_projects_directory_finds_the_project() {
    let ws = HomeWorkspace::new();
    let project = ws.als_in("p1", "Song.als");

    let resolved = clavus_cli::resolve_project(Path::new("Song.als"), Some(&ws.dir));
    assert_eq!(
        resolved,
        project,
        "the project must be found inside the projects directory"
    );

    // Two projects with the same name: none of them is chosen.
    let twin = ws.als_in("p2", "Song.als");
    let resolved = clavus_cli::resolve_project(Path::new("Song.als"), Some(&ws.dir));
    assert_eq!(
        resolved,
        Path::new("Song.als"),
        "an ambiguous name must not pick one of the projects"
    );
    assert!(twin.exists());

    // A path that exists is used exactly as it was given.
    let explicit = Path::new("/tmp/does-not-matter.als");
    let resolved = clavus_cli::resolve_project(explicit, Some(&ws.dir));
    assert_eq!(resolved, explicit);

    fs::remove_dir_all(&ws.home).ok();
}

// ─── B1: a snapshot notices that the audio changed ──────────────────────
//
// The "unchanged" check compared the project file's bytes only. A
// project whose audio was replaced — the same set, different samples
// under it — was called unchanged, the new message was thrown away,
// and the snapshot kept a manifest that described audio that no longer
// existed.

#[test]
fn a_snapshot_notices_that_the_audio_changed() {
    let ws = HomeWorkspace::new();
    let project = ws.als_in("p1", "Song.als");

    let samples = project.parent().unwrap().join("Samples");
    fs::create_dir_all(&samples).expect("samples dir");
    let kick = samples.join("kick.wav");
    fs::write(&kick, b"RIFFkick").expect("sample");

    let als = als_version("Song.als", 128).replace(
        "<DeviceChain />",
        "<DeviceChain /><SampleRef><FileRef><Path Value=\"Samples/kick.wav\" /></FileRef></SampleRef>",
    );
    write_als(&project, &als);

    let (out, res) = run(|out, err| {
        cmd_snapshot(
            &project,
            &Some("first".to_string()),
            false,
            &ws.root,
            out,
            err,
        )
    });
    assert!(res.is_ok(), "the snapshot must be taken: {:?}", res);
    assert!(
        out.all().contains("snapshot"),
        "the snapshot must be recorded: {}",
        out.all()
    );

    // The audio is replaced. The project file does not change.
    fs::write(&kick, b"RIFFsnare").expect("replace the sample");

    let (out, res) = run(|out, err| {
        cmd_snapshot(
            &project,
            &Some("second".to_string()),
            false,
            &ws.root,
            out,
            err,
        )
    });
    assert!(res.is_ok(), "the second snapshot must be taken: {:?}", res);
    assert!(
        !out.all().contains("no new snapshot was created"),
        "the audio changed — a new snapshot must be recorded: {}",
        out.all()
    );
    assert!(
        out.all().contains("the audio it points at changed"),
        "the snapshot must say the audio changed: {}",
        out.all()
    );

    // And the log carries the new message, not the old one.
    let (out, res) = run(|out, err| cmd_log(&project, &ws.root, out, err));
    assert!(res.is_ok(), "the log must be readable: {:?}", res);
    assert!(
        out.all().contains("second"),
        "the new message must be recorded: {}",
        out.all()
    );

    fs::remove_dir_all(&ws.home).ok();
}

// ─── B11: a note is always one line ─────────────────────────────────────
//
// A newline in a note broke the table apart: the second line looked
// like another row and the columns no longer lined up. An escape
// sequence was handed straight to the terminal.

#[test]
fn a_note_with_a_newline_is_still_one_row() {
    let ws = HomeWorkspace::new();
    let project = ws.als_in("p1", "Song.als");

    let (_, res) = run(|out, err| {
        cmd_cue(
            &project,
            "line1\nline2",
            "6.6.6",
            None,
            None,
            &ws.root,
            out,
            err,
        )
    });
    assert!(res.is_ok(), "the cue must be recorded: {:?}", res);

    let (out, res) = run(|out, err| cmd_cues(&project, None, &ws.root, out, err));
    assert!(res.is_ok(), "the cues must be readable: {:?}", res);
    let all = out.all();
    assert!(
        !all.contains("line1\n"),
        "the note must not break the table: {:?}",
        all
    );
    assert!(
        all.contains("line1 line2"),
        "the note must be flattened into one line: {:?}",
        all
    );

    fs::remove_dir_all(&ws.home).ok();
}

#[test]
fn a_note_with_an_escape_sequence_does_not_reach_the_terminal() {
    let ws = HomeWorkspace::new();
    let project = ws.als_in("p1", "Song.als");

    let (_, res) = run(|out, err| {
        cmd_cue(
            &project,
            "\u{1b}[31mRED\u{1b}[0m",
            "7.7.7",
            None,
            None,
            &ws.root,
            out,
            err,
        )
    });
    assert!(res.is_ok(), "the cue must be recorded: {:?}", res);

    let (out, res) = run(|out, err| cmd_cues(&project, None, &ws.root, out, err));
    assert!(res.is_ok(), "the cues must be readable: {:?}", res);
    assert!(
        !out.all().contains('\u{1b}'),
        "the escape sequence must not reach the terminal: {:?}",
        out.all()
    );
    assert!(
        out.all().contains("RED"),
        "the note's text must still be there: {:?}",
        out.all()
    );

    fs::remove_dir_all(&ws.home).ok();
}

// ─── B12: the data directory is documented ──────────────────────────────
//
// The environment variable was honoured but no help text mentioned it,
// so a user reading --help had no way of knowing it exists.

#[test]
fn the_data_directory_help_mentions_the_environment_variable() {
    use clap::CommandFactory;
    let help = clavus_cli::Cli::command().render_long_help().to_string();
    assert!(
        help.contains("CLAVUS_DATA"),
        "the help must mention CLAVUS_DATA: {}",
        help
    );
}
