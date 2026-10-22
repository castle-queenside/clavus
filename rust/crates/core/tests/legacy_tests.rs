//! Bringing an old Clavus store into the current one.
//!
//! The older version of Clavus kept its library in `~/Clavus/Projects`
//! and its history in `~/.clavus`. These tests build a store of that
//! shape and check that the history, the notes and the bytes all come
//! across — and that what cannot be brought across is named rather
//! than dropped in silence.

use std::fs;
use std::path::{Path, PathBuf};

use clavus_core::blob::BlobStore;
use clavus_core::cue::CueStore;
use clavus_core::legacy::LegacyMigration;
use clavus_core::project::ProjectId;
use clavus_core::snapshot::SnapshotStore;
use clavus_core::types::{CueStatus, Snapshot};

/// A temporary directory that goes away with the test.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let unique = format!(
            "clavus-legacy-{}-{}-{}-{}",
            label,
            std::process::id(),
            nanos,
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }

    fn join(&self, part: &str) -> PathBuf {
        self.path.join(part)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A hash made of `c`, repeated — valid hex, and easy to recognise.
fn hash_of(letter: &str) -> String {
    letter.repeat(64)
}

/// A snapshot record in the shape the old version wrote.
fn old_meta(hash: &str, parent: Option<&str>, message: &str, project_path: &str) -> String {
    serde_json::json!({
        "hash": hash,
        "timestamp": 1_700_000_000.5_f64,
        "message": message,
        "parent": parent,
        "project_path": project_path,
        "track_count": 4,
        "bpm": 128.0,
        "tags": [],
        "als_hash": hash,
        "content_hash": hash_of("d"),
        "sample_hashes": [],
        "sample_paths": {},
        "conflict_message": null,
        "notes": "",
    })
    .to_string()
}

/// Build an old store holding one project with one snapshot and a note.
fn build_old_store(root: &Path, project_name: &str, with_project_file: bool) -> String {
    let store = root.join("old-clavus");
    let objects = store.join("objects");
    let hash = hash_of("a");

    // The project's own bytes, and the record beside them.
    fs::create_dir_all(objects.join(&hash[..2])).unwrap();
    fs::write(objects.join(&hash[..2]).join(&hash), b"gzip project bytes").unwrap();
    fs::write(
        objects.join(&hash[..2]).join(format!("{}.meta", hash)),
        old_meta(
            &hash,
            None,
            "initial",
            "/old/machine/Clavus/Projects/x/x.als",
        ),
    )
    .unwrap();

    fs::create_dir_all(store.join("cues").join(project_name)).unwrap();
    fs::write(
        store.join("cues").join(project_name).join("1.json"),
        serde_json::json!({
            "id": "1",
            "position": "1.1.1",
            "text": "mixdown the drums",
            "author": "someone",
            "timestamp": 1_700_000_000.0_f64,
            "status": "pending",
            "snapshot_hash": "",
            "track_name": "Drums",
            "replies": [],
            "assignee": "",
            "in_progress": false,
            "conflict": null,
        })
        .to_string(),
    )
    .unwrap();

    fs::write(
        store.join("index.json"),
        serde_json::json!({
            project_name: {
                "name": project_name,
                "root_als": format!(
                    "/old/machine/Clavus/Projects/{0}/{0} Project/{0}.als",
                    project_name
                ),
                "head": hash,
            }
        })
        .to_string(),
    )
    .unwrap();

    // The library, where the sets live now.
    let library = root.join("library");
    let folder = library
        .join(project_name)
        .join(format!("{} Project", project_name));
    fs::create_dir_all(&folder).unwrap();
    if with_project_file {
        fs::write(folder.join(format!("{}.als", project_name)), b"<xml/>").unwrap();
    }

    hash
}

/// Run the migration and return the report.
fn migrate(
    old_store: &Path,
    new_store: &Path,
    library: Option<&Path>,
) -> clavus_core::legacy::MigrationReport {
    let migration = LegacyMigration {
        old_store,
        new_store,
        projects_root: library,
        home: None,
        dry_run: false,
    };
    migration.run().unwrap()
}

#[test]
fn a_project_s_history_comes_across() {
    let tmp = TempDir::new("history");
    let hash = build_old_store(&tmp.path, "Gigalo", true);
    let new_store = tmp.join("new-clavus");

    let report = migrate(
        &tmp.join("old-clavus"),
        &new_store,
        Some(&tmp.join("library")),
    );

    assert_eq!(report.imported.len(), 1, "one project imported");
    assert_eq!(report.imported[0].snapshots, 1);
    assert_eq!(report.imported[0].cues, 1);
    assert!(report.skipped.is_empty());

    // The chain is readable by the current store.
    let project = ProjectId::new_in(
        &tmp.join("library")
            .join("Gigalo")
            .join("Gigalo Project")
            .join("Gigalo.als"),
        None,
    );
    let store = BlobStore::open(new_store.join("blobs"));
    let snapshots = SnapshotStore::new(store, &project);
    let chain = snapshots.walk_chain(None);
    assert_eq!(chain.len(), 1, "the snapshot is in the chain");
    assert_eq!(chain[0].hash, hash);
    assert_eq!(chain[0].message, "initial");
}

#[test]
fn the_record_is_pointed_at_where_the_project_lives_now() {
    let tmp = TempDir::new("path");
    build_old_store(&tmp.path, "Gigalo", true);
    let new_store = tmp.join("new-clavus");

    migrate(
        &tmp.join("old-clavus"),
        &new_store,
        Some(&tmp.join("library")),
    );

    let project = ProjectId::new_in(
        &tmp.join("library")
            .join("Gigalo")
            .join("Gigalo Project")
            .join("Gigalo.als"),
        None,
    );
    let store = BlobStore::open(new_store.join("blobs"));
    let snapshots = SnapshotStore::new(store, &project);
    let chain = snapshots.walk_chain(None);

    // The old record named a path on the old machine. A restore has to
    // write where the project is now.
    assert_eq!(chain[0].project_path, project.path.to_string_lossy());
}

#[test]
fn the_notes_come_across_and_belong_to_the_project() {
    let tmp = TempDir::new("cues");
    build_old_store(&tmp.path, "Gigalo", true);
    let new_store = tmp.join("new-clavus");

    migrate(
        &tmp.join("old-clavus"),
        &new_store,
        Some(&tmp.join("library")),
    );

    let project = ProjectId::new_in(
        &tmp.join("library")
            .join("Gigalo")
            .join("Gigalo Project")
            .join("Gigalo.als"),
        None,
    );
    let cues = CueStore::open(&new_store, &project);
    let list = cues.list(None).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].text, "mixdown the drums");
    assert_eq!(list[0].project, project.relay_key());
}

#[test]
fn a_project_whose_file_is_gone_is_named_not_dropped() {
    let tmp = TempDir::new("missing");
    build_old_store(&tmp.path, "Gigalo", false);
    let new_store = tmp.join("new-clavus");

    let report = migrate(
        &tmp.join("old-clavus"),
        &new_store,
        Some(&tmp.join("library")),
    );

    assert!(report.imported.is_empty());
    assert_eq!(report.skipped.len(), 1, "it is reported, not dropped");
    assert_eq!(report.skipped[0].name, "Gigalo");
    assert!(
        report.skipped[0].reason.contains("Gigalo"),
        "the reason names the project: {}",
        report.skipped[0].reason
    );
}

#[test]
fn a_record_that_cannot_be_read_is_named() {
    let tmp = TempDir::new("corrupt");
    let hash = build_old_store(&tmp.path, "Gigalo", true);
    // Damage the record.
    fs::write(
        tmp.join("old-clavus")
            .join("objects")
            .join(&hash[..2])
            .join(format!("{}.meta", hash)),
        "{ this is not json",
    )
    .unwrap();
    let new_store = tmp.join("new-clavus");

    let report = migrate(
        &tmp.join("old-clavus"),
        &new_store,
        Some(&tmp.join("library")),
    );

    assert_eq!(report.unreadable_snapshots.len(), 1, "it is named");
    assert!(
        report.unreadable_snapshots[0].contains(&hash[..8]),
        "the path is given: {:?}",
        report.unreadable_snapshots
    );
}

#[test]
fn the_bytes_come_across_and_can_be_replaced() {
    let tmp = TempDir::new("bytes");
    let hash = build_old_store(&tmp.path, "Gigalo", true);
    let new_store = tmp.join("new-clavus");

    migrate(
        &tmp.join("old-clavus"),
        &new_store,
        Some(&tmp.join("library")),
    );

    let blob = new_store
        .join("blobs")
        .join("objects")
        .join(&hash[..2])
        .join(&hash);
    assert!(blob.exists(), "the project's bytes came across");
    assert_eq!(fs::read(&blob).unwrap(), b"gzip project bytes");

    // A blob that cannot be replaced would break the store the next
    // time the same bytes were written. A backup's files are often
    // read-only, and carry an access list that forbids deleting them.
    fs::write(&blob, b"new bytes").unwrap();
    assert_eq!(fs::read(&blob).unwrap(), b"new bytes");
}

#[test]
fn a_dry_run_writes_nothing() {
    let tmp = TempDir::new("dry");
    build_old_store(&tmp.path, "Gigalo", true);
    let new_store = tmp.join("new-clavus");

    let migration = LegacyMigration {
        old_store: &tmp.join("old-clavus"),
        new_store: &new_store,
        projects_root: Some(&tmp.join("library")),
        home: None,
        dry_run: true,
    };
    let report = migration.run().unwrap();

    assert_eq!(report.imported.len(), 1, "it says what would happen");
    assert!(report.blobs_copied > 0, "and counts the bytes");
    assert!(
        !new_store.join("blobs").join("objects").exists(),
        "but nothing was written"
    );
    assert!(
        !new_store.join("cues").exists(),
        "and no notes were written"
    );
}

#[test]
fn the_project_the_old_store_named_is_the_one_that_is_used() {
    // A library can hold two files of the same name — one of them a
    // copy left in the folder. The old store named the right one, and
    // the part of its path after the library folder is still true.
    let tmp = TempDir::new("twins");
    let hash = build_old_store(&tmp.path, "Me And You", true);
    let library = tmp.join("library");

    // A second file of the same name, at the top of the folder.
    fs::write(library.join("Me And You").join("Me And You.als"), b"<xml/>").unwrap();

    let new_store = tmp.join("new-clavus");
    let report = migrate(&tmp.join("old-clavus"), &new_store, Some(&library));

    assert_eq!(report.imported.len(), 1);
    assert_eq!(
        report.imported[0].project_path,
        library
            .join("Me And You")
            .join("Me And You Project")
            .join("Me And You.als"),
        "the file the old store named is the one used"
    );
    let _ = hash;
}

#[test]
fn a_note_written_by_the_old_version_is_readable() {
    // The old version wrote `pending` in lower case; this one writes
    // `Pending`. A store written years ago still has to be readable.
    assert_eq!(CueStatus::parse("pending"), Some(CueStatus::Pending));
    assert_eq!(CueStatus::parse("Pending"), Some(CueStatus::Pending));
    assert_eq!(CueStatus::parse("RESOLVED"), Some(CueStatus::Resolved));
    assert_eq!(CueStatus::parse("nonsense"), None);

    let snapshot: Snapshot = serde_json::from_str(&old_meta(&hash_of("a"), None, "m", "/p"))
        .expect("a record from the old version still reads");
    assert_eq!(snapshot.message, "m");
    assert_eq!(snapshot.track_count, 4);
}
