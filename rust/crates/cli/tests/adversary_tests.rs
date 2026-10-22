//! Regression tests for the adversary's findings.
//!
//! Each test here reproduces a defect an AI beta agent found in the
//! compiled binary: data loss, a lie about what was checked, and a
//! broken history reported as a clean one.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clavus_cli::{cmd_cues, cmd_log, cmd_pull, cmd_collect, cmd_status};
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

fn als_xml(sample: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tempo Value="128" />
    <Tracks>
      <Track Type="Audio" Index="0">
        <Name Value="Kick" />
        <DeviceChain>
          <AudioEffectChain>
            <Devices>
              <Device>
                <Name Value="Simpler" />
                <SampleRef><Path Value="{}" /></SampleRef>
              </Device>
            </Devices>
          </AudioEffectChain>
        </DeviceChain>
      </Track>
    </Tracks>
    <MasterTrack><DeviceChain /></MasterTrack>
  </LiveSet>
</Ableton>"#,
        sample
    )
}

/// The text of a project file, whether it is compressed or not.
fn read_als_text(path: &Path) -> String {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let data = fs::read(path).expect("the project");
    let mut out = String::new();
    match GzDecoder::new(&data[..]).read_to_string(&mut out) {
        Ok(_) => out,
        Err(_) => String::from_utf8_lossy(&data).to_string(),
    }
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
    /// A folder outside the project, holding the user's library.
    library: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp.path().join(".clavus");
        let dir = tmp.path().join("projects");
        fs::create_dir_all(&dir).expect("projects dir");
        let library = tmp.path().join("library");
        fs::create_dir_all(&library).expect("library dir");
        Workspace {
            _tmp: tmp,
            root,
            dir,
            library,
        }
    }
}

// ─── A1: collect must never overwrite the user's own sample ───────────────
//
// Regression: `collect` copied a sample from outside the project into
// `Samples/<name>`, overwriting a file that was already there. The
// user's own audio was destroyed, and two tracks ended up silently
// sharing one file.

#[test]
fn collect_never_overwrites_a_sample_that_is_already_in_the_project() {
    let ws = Workspace::new();

    // A library sample, outside the project.
    let library_kick = ws.library.join("kick.wav");
    fs::write(&library_kick, b"RIFF-LIBRARY-KICK").expect("write the library sample");

    // The project, and the user's own sample of the same name.
    let als = ws.dir.join("Set.als");
    write_als(&als, &als_xml(&library_kick.to_string_lossy()));
    let samples = ws.dir.join("Samples");
    fs::create_dir_all(&samples).expect("samples dir");
    let own_kick = samples.join("kick.wav");
    fs::write(&own_kick, b"PROJECT-OWN-KICK").expect("write the project's sample");

    let (out, result) = run(|o, e| cmd_collect(&als, &ws.root, o, e));
    assert!(result.is_ok(), "collect must work: {}", out.all());

    // The user's file must be exactly as it was.
    let after = fs::read(&own_kick).expect("the project's sample");
    assert_eq!(
        after,
        b"PROJECT-OWN-KICK".to_vec(),
        "the user's own sample must not be overwritten"
    );

    // The collected sample must have a name of its own, and the
    // reference must point at it — so the two tracks do not share a
    // file.
    let collected: Vec<PathBuf> = fs::read_dir(&samples)
        .expect("samples dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p != &own_kick)
        .collect();
    assert_eq!(
        collected.len(),
        1,
        "the sample must be collected under a name of its own: {:?}",
        collected
    );

    // The project must point at the new file.
    let xml = read_als_text(&als);
    let collected_name = collected[0]
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    assert!(
        xml.contains(&format!("Samples/{}", collected_name)),
        "the reference must point at the collected file '{}': {}",
        collected_name,
        xml
    );
    assert!(
        !xml.contains("Samples/kick.wav"),
        "the reference must not point at the user's own sample: {}",
        xml
    );
}

#[test]
fn collect_does_not_copy_a_sample_it_already_has() {
    let ws = Workspace::new();

    // A library sample, outside the project.
    let library_kick = ws.library.join("kick.wav");
    fs::write(&library_kick, b"RIFF-LIBRARY-KICK").expect("write the library sample");

    // The project already carries the very same audio.
    let als = ws.dir.join("Set.als");
    write_als(&als, &als_xml(&library_kick.to_string_lossy()));
    let samples = ws.dir.join("Samples");
    fs::create_dir_all(&samples).expect("samples dir");
    fs::write(samples.join("kick.wav"), b"RIFF-LIBRARY-KICK").expect("write the sample");

    let (out, result) = run(|o, e| cmd_collect(&als, &ws.root, o, e));
    assert!(result.is_ok(), "collect must work: {}", out.all());

    // No copy is made: the project already has this audio.
    let files: Vec<PathBuf> = fs::read_dir(&samples)
        .expect("samples dir")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(
        files.len(),
        1,
        "the same audio must not be copied twice: {:?}",
        files
    );
}

// ─── A2: a pull that checked nothing must not say it is up to date ───────
//
// Regression: a relay that answered with an acknowledgement and nothing
// else made the pull print both "✗ nothing was checked" and "✓ up to
// date", and exit 0. A script could not tell success from failure.

#[test]
fn a_pull_that_checked_nothing_fails() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let ws = Workspace::new();
    let als = ws.dir.join("Set.als");
    write_als(&als, &als_xml("/nowhere/kick.wav"));

    // A relay that answers with one frame that says nothing about the
    // project's history, and then keeps quiet.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let relay = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Read the client's hello.
            let mut header = [0u8; 4];
            if stream.read_exact(&mut header).is_ok() {
                let len = u32::from_be_bytes(header) as usize;
                let mut body = vec![0u8; len];
                let _ = stream.read_exact(&mut body);
            }
            // Answer with a bare acknowledgement.
            let msg = br#"{"type":"synced"}"#;
            let mut frame = Vec::new();
            frame.extend_from_slice(&(msg.len() as u32).to_be_bytes());
            frame.extend_from_slice(msg);
            let _ = stream.write_all(&frame);
            let _ = stream.flush();
            // Then say nothing at all.
            std::thread::sleep(Duration::from_secs(12));
        }
    });

    let (out, result) = run(|o, e| cmd_pull(&addr, &als, &ws.root, o, e));
    assert!(
        result.is_err(),
        "a pull that checked nothing must fail: {}",
        out.all()
    );
    assert!(
        !out.all().contains("Up to date"),
        "the pull must not claim it is up to date: {}",
        out.all()
    );
    assert!(
        out.all().contains("nothing was checked"),
        "the pull must say that nothing was checked: {}",
        out.all()
    );

    drop(relay);
}

// ─── A3: a broken history must not be reported as a clean one ───────────
//
// Regression: a snapshot record that could not be parsed made `log` say
// "No snapshots yet" and `status` say "1 snapshot(s) (HEAD: )". The two
// commands disagreed, and both hid a damaged history.

#[test]
fn a_corrupt_snapshot_record_is_reported_as_corrupt() {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;

    let ws = Workspace::new();
    let als = ws.dir.join("Set.als");
    write_als(&als, &als_xml("/nowhere/kick.wav"));

    // Take a snapshot so that there is a record to damage.
    let store = BlobStore::new(ws.root.join("blobs")).expect("blob store");
    let id = clavus_core::project::ProjectId::new(&als);
    let snap_store = SnapshotStore::new(store, &id);
    let outcome = snap_store
        .save(
            &als,
            "first",
            None,
            1,
            128.0,
            vec![],
            std::collections::HashMap::new(),
        )
        .expect("the snapshot must be taken");
    let hash = match outcome {
        clavus_core::snapshot::SaveOutcome::Created(s) => s.hash,
        clavus_core::snapshot::SaveOutcome::Unchanged(s) => s.hash,
        clavus_core::snapshot::SaveOutcome::Updated(s) => s.hash,
    };

    // Damage the record.
    let key = clavus_core::project::ProjectId::new(&als).key;
    let record = ws
        .root
        .join("blobs")
        .join("projects")
        .join(key)
        .join("snapshots")
        .join(format!("{}.json", hash));
    fs::write(&record, b"{\"hash\": \"abc").expect("damage the record");

    // `log` must not say "no snapshots".
    let (out, result) = run(|o, e| cmd_log(&als, &ws.root, o, e));
    assert!(result.is_err(), "log must fail: {}", out.all());
    assert!(
        !out.all().contains("No snapshots yet"),
        "a broken history must not be reported as an empty one: {}",
        out.all()
    );
    let message = result.unwrap_err().to_string().to_lowercase();
    assert!(
        message.contains("corrupt"),
        "the error must say the record is corrupt: {}",
        message
    );

    // `status` must say the same thing.
    let (out, result) = run(|o, e| cmd_status(&als, &ws.root, o, e));
    assert!(result.is_ok(), "status must still run: {}", out.all());
    assert!(
        out.all().to_lowercase().contains("corrupt"),
        "status must report the damaged history: {}",
        out.all()
    );
    assert!(
        !out.all().contains("No snapshots yet"),
        "status must not report a clean empty history: {}",
        out.all()
    );
}

#[test]
fn a_corrupt_note_is_reported_as_corrupt() {
    let ws = Workspace::new();
    let als = ws.dir.join("Set.als");
    write_als(&als, &als_xml("/nowhere/kick.wav"));

    let (out, result) = run(|o, e| {
        clavus_cli::cmd_cue(
            &als,
            "a note",
            "1.1.1",
            None,
            None,
            &ws.root,
            o,
            e,
        )
    });
    assert!(result.is_ok(), "the note must be made: {}", out.all());

    // Damage the note.
    let key = clavus_core::project::ProjectId::new(&als).key;
    let cues_dir = ws.root.join("cues").join(key);
    let file = fs::read_dir(&cues_dir)
        .expect("cues dir")
        .flatten()
        .find(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .expect("a note file")
        .path();
    fs::write(&file, b"{\"id\": \"abc").expect("damage the note");

    let (out, result) = run(|o, e| cmd_cues(&als, None, &ws.root, o, e));
    assert!(result.is_err(), "the cues must fail: {}", out.all());
    assert!(
        !out.all().contains("No cues"),
        "a broken note must not be reported as an empty list: {}",
        out.all()
    );
    let message = result.unwrap_err().to_string().to_lowercase();
    assert!(
        message.contains("corrupt"),
        "the error must say the note is corrupt: {}",
        message
    );
}
