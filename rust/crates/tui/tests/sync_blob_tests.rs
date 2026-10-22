//! The TUI's sync round trip: push a project, pull it, restore it.
//!
//! The TUI sends the project bytes with its snapshots and fetches them
//! again on a pull, so a collaborator can restore what they were sent
//! and not only read the history. These tests drive the real TUI
//! command path against a real relay.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clavus_core::project::ProjectId;
use clavus_tui::app::{App, SyncCmd};
use tempfile::TempDir;

/// A minimal but valid Ableton project.
const MINIMAL_ALS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tempo Value="128" />
    <Tracks>
      <Track Type="Audio" Index="0">
        <Name Value="Drums"/>
      </Track>
    </Tracks>
    <MasterTrack>
      <DeviceChain>
        <AudioEffectChain>
          <Devices/>
        </AudioEffectChain>
      </DeviceChain>
    </MasterTrack>
  </LiveSet>
</Ableton>
"#;

fn write_als(path: &Path, xml: &str) {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let file = fs::File::create(path).expect("create als");
    let mut enc = GzEncoder::new(file, Compression::default());
    use std::io::Write;
    enc.write_all(xml.as_bytes()).expect("write als");
    enc.finish().expect("finish als");
}

/// A machine: a projects folder, a data folder and a TUI.
struct Machine {
    _tmp: TempDir,
    projects: PathBuf,
    root: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temp dir");
        let projects = tmp.path().join("clavus").join("projects");
        fs::create_dir_all(&projects).expect("projects dir");
        let root = tmp.path().join(".clavus");
        Machine {
            _tmp: tmp,
            projects,
            root,
        }
    }

    fn project(&self, name: &str) -> PathBuf {
        let path = self.projects.join(name);
        write_als(&path, MINIMAL_ALS);
        path
    }

    fn app(&self) -> App {
        let mut app = App::new(self.projects.clone());
        app.clavus_root = self.root.clone();
        app.refresh_projects().expect("the projects must be found");
        app
    }
}

/// Start a real relay on a free port.
fn start_relay() -> (String, TempDir) {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let dir = TempDir::new().expect("relay dir");
    let dir_path = dir.path().to_path_buf();
    std::thread::spawn(move || {
        let _ = clavus_relay::run_with_listener(listener, dir_path);
    });
    for _ in 0..200 {
        if std::net::TcpStream::connect(&addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    (addr, dir)
}

/// Wait for the TUI's background sync thread to report something.
fn wait_for<F: FnMut() -> bool>(mut check: F) -> bool {
    for _ in 0..400 {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn the_tui_pulls_the_project_bytes_and_can_restore_them() {
    let (addr, _relay_dir) = start_relay();

    // ── Machine A: take a snapshot and push it ──
    let a = Machine::new();
    let als_a = a.project("Shared.als");
    let mut app_a = a.app();
    app_a.load_project(0).expect("the project must load");

    // Take a snapshot through the real core store.
    {
        use clavus_core::blob::BlobStore;
        use clavus_core::snapshot::SnapshotStore;
        let store = BlobStore::new(a.root.join("blobs")).expect("blob store");
        let id = ProjectId::new(&als_a);
        let snap_store = SnapshotStore::new(store, &id);
        let outcome = snap_store
            .save(
                &als_a,
                "shared v1",
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
        assert!(snap_store.has(&hash));
    }

    app_a.sync_connect(&addr);
    assert!(
        wait_for(|| {
            app_a.poll_sync();
            app_a.sync_state.is_connected()
        }),
        "the TUI must connect to the relay: {:?}",
        app_a.sync_state.relay_addr
    );

    // Push the local chain, with the project bytes.
    let snapshots = app_a.selected_snapshots();
    assert!(!snapshots.is_empty(), "there is a snapshot to push");
    let cues = app_a.project_cues();
    let blobs = app_a.selected_snapshot_bytes();
    assert!(
        !blobs.is_empty(),
        "the project bytes must travel with the snapshot"
    );
    let project = app_a.selected_project_sync_name();
    assert!(
        app_a
            .sync_state
            .send_cmd(SyncCmd::Push {
                snapshots,
                cues,
                blobs,
                project,
            }),
        "the push must be sent"
    );

    // Give the relay time to take the bytes and the snapshot.
    std::thread::sleep(Duration::from_millis(500));

    // ── Machine B: pull, and check the bytes arrived ──
    let b = Machine::new();
    let als_b = b.project("Shared.als");
    let mut app_b = b.app();
    app_b.load_project(0).expect("the project must load");
    app_b.sync_connect(&addr);
    assert!(
        wait_for(|| {
            app_b.poll_sync();
            app_b.sync_state.is_connected()
        }),
        "the TUI must connect to the relay"
    );

    let known_heads = app_b.known_heads();
    let clavus_root = b.root.clone();
    assert!(
        app_b.sync_state.send_cmd(SyncCmd::Pull {
            known_heads,
            project_path: als_b.clone(),
            clavus_root,
        }),
        "the pull must be sent"
    );

    // The pulled snapshot must show up in the TUI's own list.
    assert!(
        wait_for(|| {
            app_b.poll_sync();
            let _ = app_b.load_snapshots();
            !app_b.snapshots.is_empty()
        }),
        "the pulled snapshot must appear in the TUI"
    );

    // And the bytes must be here, so the snapshot can be restored.
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;
    let store = BlobStore::new(b.root.join("blobs")).expect("blob store");
    let id = ProjectId::new(&als_b);
    let snap_store = SnapshotStore::new(store, &id);
    let pulled = app_b.snapshots[0].hash.clone();
    assert!(
        snap_store.has(&pulled),
        "the pulled snapshot must be in the store"
    );

    let original = fs::read(&als_a).expect("the original file");
    snap_store
        .restore(&pulled, Some(&als_b))
        .expect("the snapshot must restore");
    let restored = fs::read(&als_b).expect("the restored file");
    assert_eq!(
        restored, original,
        "the restored project must be the collaborator's file, byte for byte"
    );
}
