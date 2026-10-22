//! Tests for clavus-core: snapshot chain, conflict detection, sample audit.

use clavus_core::{
    types::{Marker, Project, ProjectType, SampleAuditReport, SampleRef, Snapshot},
    BlobStore, ClavusError, SnapshotStore,
};
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

// ─── Snapshot chain invariants ──────────────────────────────────────────────

fn make_snap(hash: &str, parent: Option<&str>) -> Snapshot {
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
        project_path: "/tmp/test.als".to_string(),
        project_folder: String::new(),
    }
}

#[test]
fn test_snapshot_chain_single() {
    let snap = make_snap("abc", None);
    assert!(snap.parent.is_none());
    assert_eq!(snap.hash, "abc");
}

#[test]
fn test_snapshot_chain_multiple() {
    let s1 = make_snap("s1", None);
    let s2 = make_snap("s2", Some("s1"));
    let s3 = make_snap("s3", Some("s2"));

    assert!(s1.parent.is_none());
    assert_eq!(s2.parent.as_deref(), Some("s1"));
    assert_eq!(s3.parent.as_deref(), Some("s2"));
}

// ─── Conflict detection ─────────────────────────────────────────────────────

/// Build a relay-state-like structure and test is_ancestor
fn make_chain<'a>(pairs: &[(&str, Option<&str>)]) -> HashMap<String, Snapshot> {
    pairs
        .iter()
        .map(|(h, p)| (h.to_string(), make_snap(h, *p)))
        .collect()
}

#[test]
fn test_ancestor_self() {
    let store = make_chain(&[("a", None)]);
    // is_ancestor would need to be public — test indirectly via hash equality
    assert_eq!(store.get("a").map(|s| s.hash.clone()), Some("a".to_string()));
}

#[test]
fn test_chain_hash_deduplication() {
    let (store, _tmp) = temp_store();
    let h1 = store.put(b"test data");
    let h2 = store.put(b"test data");
    assert_eq!(h1, h2, "same content must produce same hash");
}

#[test]
fn test_different_content_different_hash() {
    let (store, _tmp) = temp_store();
    let h1 = store.put(b"content a");
    let h2 = store.put(b"content b");
    assert_ne!(h1, h2, "different content must produce different hashes");
}

// ─── Sample audit ──────────────────────────────────────────────────────────

#[test]
fn test_audit_report_is_clean() {
    let report = SampleAuditReport {
        collected: vec![],
        missing: vec![],
        outside_project: vec![],
        embedded: vec![],
    };
    assert!(report.is_clean());
}

#[test]
fn test_audit_report_missing_not_clean() {
    let report = SampleAuditReport {
        collected: vec![],
        missing: vec![SampleRef {
            path: PathBuf::from("/missing/kick.wav"),
            filename: "kick.wav".to_string(),
            resolved_path: None,
            relpath_from_project: None,
            hash: None,
            size_bytes: 0,
            archive_entry: None,
        }],
        outside_project: vec![],
    
        embedded: vec![],};
    assert!(!report.is_clean());
    assert_eq!(report.total(), 1);
}

#[test]
fn test_audit_report_outside_project_not_clean() {
    let report = SampleAuditReport {
        collected: vec![],
        missing: vec![],
        outside_project: vec![SampleRef {
            path: PathBuf::from("/Users/external/kick.wav"),
            filename: "kick.wav".to_string(),
            resolved_path: Some(PathBuf::from("/Users/external/kick.wav")),
            relpath_from_project: None,
            hash: Some("deadbeef".to_string()),
            size_bytes: 1024,
            archive_entry: None,
        }],
    
        embedded: vec![],};
    assert!(!report.is_clean());
    assert_eq!(report.total(), 1);
}

#[test]
fn test_audit_report_all_present() {
    let report = SampleAuditReport {
        collected: vec![
            SampleRef {
                path: PathBuf::from("Samples/kick.wav"),
                filename: "kick.wav".to_string(),
                resolved_path: Some(PathBuf::from("/tmp/proj/Samples/kick.wav")),
                relpath_from_project: Some("Samples/kick.wav".to_string()),
                hash: Some("abc123".to_string()),
                size_bytes: 4096,
                archive_entry: None,
            },
        ],
        missing: vec![],
        outside_project: vec![],
    
        embedded: vec![],};
    assert!(report.is_clean());
    assert_eq!(report.total(), 1);
}

// ─── BlobStore ─────────────────────────────────────────────────────────────

fn temp_store() -> (BlobStore, TempDir) {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
    (store, tmp)
}

#[test]
fn test_blob_roundtrip() {
    let (store, _tmp) = temp_store();
    let data = b"hello world";
    let hash = store.put(data);
    assert_eq!(store.get(&hash), Some(data.to_vec()));
}

#[test]
fn test_blob_deduplication() {
    let (store, _tmp) = temp_store();
    let h1 = store.put(b"foo");
    let h2 = store.put(b"foo");
    assert_eq!(h1, h2);
}

#[test]
fn test_blob_short_hash() {
    let (store, _tmp) = temp_store();
    let hash = store.put(b"test data");
    assert!(store.has(&hash[..8]));
    assert!(store.has(&hash[..4]));
}

#[test]
fn test_blob_not_found() {
    let (store, _tmp) = temp_store();
    assert!(store.get("notthere").is_none());
    assert!(!store.has("notthere"));
}

#[test]
fn test_sha256_deterministic() {
    let hash1 = BlobStore::sha256(b"test");
    let hash2 = BlobStore::sha256(b"test");
    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 64);
}

#[test]
fn test_sha256_known_values() {
    assert_eq!(
        BlobStore::sha256(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        BlobStore::sha256(b"hello"),
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn test_refs_lifecycle() {
    let (store, _tmp) = temp_store();
    store.update_ref("main", "abc123").unwrap();
    assert_eq!(store.read_ref("main"), Some("abc123".to_string()));
    store.delete_ref("main").unwrap();
    assert_eq!(store.read_ref("main"), None);
}

#[test]
fn test_meta_write_load() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
    let hash = "a".repeat(64);
    let snap = make_snap(&hash, None);
    store.write_meta("test", &snap).unwrap();
    let loaded = store.load_meta("test", &hash);
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.hash, hash);
    // tmp stays alive here
}

#[test]
fn test_meta_load_not_found() {
    let (store, _tmp) = temp_store();
    assert!(store.load_meta("test", "nonexistent").is_none());
}

#[test]
fn test_index_roundtrip() {
    let (store, _tmp) = temp_store();
    let mut idx: HashMap<String, serde_json::Value> = HashMap::new();
    idx.insert("key".to_string(), serde_json::json!({"v": 1}));
    store.write_index(&idx).unwrap();
    let loaded = store.read_index();
    assert_eq!(loaded.get("key").and_then(|v| v.get("v").and_then(|x| x.as_i64())), Some(1));
}

#[test]
fn test_index_empty_when_missing() {
    let (store, _tmp) = temp_store();
    let idx = store.read_index();
    assert!(idx.is_empty());
}

#[test]
fn test_blob_unicode_filename() {
    let (store, _tmp) = temp_store();
    // UTF-8 content should work fine
    let data = "こんにちは世界 🌍".as_bytes();
    let hash = store.put(data);
    assert_eq!(store.get(&hash), Some(data.to_vec()));
}

#[test]
fn test_blob_large_content() {
    let (store, _tmp) = temp_store();
    let data: Vec<u8> = (0..=255).cycle().take(1024 * 1024).collect();
    let hash = store.put(&data);
    assert_eq!(store.get(&hash).map(|d| d.len()), Some(1024 * 1024));
}

// ─── Project ───────────────────────────────────────────────────────────────

#[test]
fn test_project_defaults() {
    // Project has no Default — check field existence by construction
    let p = Project {
        path: PathBuf::from("/tmp/test.als"),
        project_type: ProjectType::Ableton,
        name: "test".to_string(),
        head: None,
        track_count: 0,
        bpm: 120.0,
        time_signature: "4/4".to_string(),
        ableton_version: None,
        schema_version: None,
        tracks: vec![],
        return_tracks: vec![],
        master_track: None,
        markers: vec![],
        tempo_events: vec![],
    };
    assert_eq!(p.track_count, 0);
    assert_eq!(p.bpm, 120.0);
    assert!(p.tracks.is_empty());
    assert!(p.markers.is_empty());
}

#[test]
fn test_project_type_variants() {
    use clavus_core::types::ProjectType;
    assert!(matches!(ProjectType::Ableton, ProjectType::Ableton));
    assert!(matches!(ProjectType::Bitwig, ProjectType::Bitwig));
}

#[test]
fn test_marker_default() {
    let m = Marker {
        time: "1.1.1".to_string(),
        name: "Intro".to_string(),
    };
    assert_eq!(m.time, "1.1.1");
    assert_eq!(m.name, "Intro");
}

// ─── SnapshotStore.has / save_raw (relay pull support) ──────────────────────

#[test]
fn test_snapshot_store_has_false_for_unknown() {
    let (store, _tmp) = temp_store();
    let snap_store = SnapshotStore::new(store, &clavus_core::project::ProjectId::new(std::path::Path::new("test")));
    assert!(!snap_store.has("nonexistent"));
}

#[test]
fn test_snapshot_store_save_raw_and_has() {
    let (store, _tmp) = temp_store();
    let snap_store = SnapshotStore::new(store, &clavus_core::project::ProjectId::new(std::path::Path::new("test")));

    let mut snap = clavus_core::types::Snapshot {
        hash: "abc123def456".to_string(),
        parent: None,
        timestamp: 1700000000.0,
        message: "relay pull".to_string(),
        track_count: 4,
        bpm: 120.0,
        tags: vec![],
        notes: String::new(),
        sample_hashes: vec![],
        sample_paths: std::collections::HashMap::new(),
        project_path: "/tmp/test.als".to_string(),
        project_folder: String::new(),
    };

    let hash = "b".repeat(64);
    snap.hash = hash.clone();
    assert!(!snap_store.has(&hash));
    snap_store.save_raw(snap).unwrap();
    assert!(snap_store.has(&hash));

    // save_raw does not update HEAD
    assert!(snap_store.head().is_none());
}

// ─── Hash and ref name safety ─────────────────────────────────────────────
//
// Hashes and ref names arrive from remote peers over the sync protocol and
// are used to build file paths. A crafted value must never be able to
// escape the store, and a short value must never panic.

#[test]
fn test_valid_hashes_are_accepted() {
    assert!(BlobStore::is_valid_hash(&"a".repeat(64)));
    assert!(BlobStore::is_valid_hash(&"0".repeat(64)));
    assert!(BlobStore::is_valid_hash("ab"));
    assert!(BlobStore::is_valid_hash("0123456789abcdef"));
}

#[test]
fn test_invalid_hashes_are_rejected() {
    for bad in [
        "",
        "a",
        "../../etc/passwd",
        "..",
        "ABCDEF0123456789",           // uppercase
        "0123456789abcdefg",          // 'g' is not hex
        "0123456789abcdef.meta",      // a suffix
        "0123456789abcdef ../../x",   // embedded path
        "0123456789abcdef/../x",
        "0123456789abcdef\\x",
    ] {
        assert!(
            !BlobStore::is_valid_hash(bad),
            "hash {:?} should be rejected",
            bad
        );
    }
}

#[test]
fn test_a_too_long_hash_is_rejected() {
    assert!(!BlobStore::is_valid_hash(&"a".repeat(65)));
}

#[test]
fn test_meta_path_rejects_a_traversing_hash() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
    assert!(store.meta_path("../../escaped").is_none());
    assert!(store.sample_meta_path("../../escaped").is_none());
    assert!(store.meta_path("ab").is_none(), "a prefix is not a full hash");
}

#[test]
fn test_write_meta_rejects_a_traversing_hash() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
    let mut snap = make_snap(&"a".repeat(64), None);
    snap.hash = "../../escaped".to_string();
    let err = store
        .write_meta("test", &snap)
        .expect_err("a traversing hash must be rejected");
    assert!(err.to_string().contains("invalid blob hash"), "{}", err);
    assert!(
        !tmp.path().join("escaped.meta").exists(),
        "nothing may be written outside the store"
    );
}

#[test]
fn test_read_ref_rejects_a_traversing_name() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
    // A file that a traversal would reach.
    let victim = tmp.path().join("victim");
    std::fs::write(&victim, b"secret").unwrap();

    assert_eq!(store.read_ref("../victim"), None);
    assert_eq!(store.read_ref("a/b"), None);
    assert_eq!(store.read_ref(".."), None);
    assert!(store.update_ref("../victim", "abc").is_err());
    assert!(store.delete_ref("../victim").is_err());
    assert!(victim.exists(), "the file outside the refs dir must survive");
}

#[test]
fn test_ordinary_refs_still_work() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
    store.update_ref("HEAD", "abc123").unwrap();
    assert_eq!(store.read_ref("HEAD"), Some("abc123".to_string()));
    store.delete_ref("HEAD").unwrap();
    assert_eq!(store.read_ref("HEAD"), None);
}

#[test]
fn test_a_short_hash_does_not_panic() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().to_path_buf()).unwrap();
    // A one-character hash used to panic while slicing the prefix.
    assert!(!store.has("a"));
    assert!(store.get("a").is_none());
    assert!(store.load_meta("test", "a").is_none());
    assert!(store.sample_filename("a").is_none());
    assert!(store.sample_relpath("a").is_none());
}

#[test]
fn test_restore_refuses_a_non_project_destination_from_a_remote_snapshot() {
    // A snapshot's project_path can come from a remote peer, so restoring
    // to it must not be able to write project bytes anywhere on disk.
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().join("blobs")).unwrap();
    // The store's project matches the snapshot's file name, so the
    // snapshot is found and the destination rule is what refuses it.
    let project = clavus_core::project::ProjectId::new(std::path::Path::new(
        "/tmp/definitely-not-a-project.txt",
    ));
    let snap_store = SnapshotStore::new(store.clone(), &project);

    let bytes = b"<LiveSet/>".to_vec();
    let hash = store.put(&bytes);
    let mut snap = make_snap(&hash, None);
    snap.project_path = "/tmp/definitely-not-a-project.txt".to_string();
    store.write_meta(&project.key, &snap).unwrap();

    let err = snap_store
        .restore(&hash, None)
        .expect_err("restore must refuse a non-project destination");
    assert!(
        err.to_string().contains("refusing to restore"),
        "unexpected error: {}",
        err
    );
    assert!(
        !std::path::Path::new("/tmp/definitely-not-a-project.txt").exists(),
        "nothing may be written to the remote path"
    );
}

#[test]
fn test_restore_to_an_explicit_target_still_works() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().join("blobs")).unwrap();
    let project =
        clavus_core::project::ProjectId::new(std::path::Path::new("/tmp/whatever.als"));
    let snap_store = SnapshotStore::new(store.clone(), &project);

    let bytes = b"<LiveSet/>".to_vec();
    let hash = store.put(&bytes);
    let mut snap = make_snap(&hash, None);
    snap.project_path = "/tmp/whatever.als".to_string();
    store.write_meta(&project.key, &snap).unwrap();

    let target = tmp.path().join("Restored.als");
    snap_store.restore(&hash, Some(&target)).expect("restore");
    assert_eq!(std::fs::read(&target).unwrap(), bytes);
}

// ─── Per-project HEAD ──────────────────────────────────────────────────────
//
// The blob store is shared by every project, so each project needs its own
// HEAD. With a single shared ref, whichever project was snapshotted last
// owned the chain, and `log`, `push` and `restore` for the other project
// showed the wrong history.

#[test]
fn test_each_project_has_its_own_head() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().join("blobs")).unwrap();
    let alpha = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Alpha")));
    let beta = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Beta")));

    let a = tmp.path().join("Alpha.als");
    let b = tmp.path().join("Beta.als");
    std::fs::write(&a, b"<LiveSet id='alpha'/>").unwrap();
    std::fs::write(&b, b"<LiveSet id='beta'/>").unwrap();

    let snap_a = alpha.save(&a, "alpha work", None, 1, 120.0, vec![], Default::default()).unwrap();
    let snap_b = beta.save(&b, "beta work", None, 2, 130.0, vec![], Default::default()).unwrap();

    assert_ne!(snap_a.hash, snap_b.hash);

    // Each project sees its own HEAD.
    assert_eq!(
        alpha.head().expect("alpha should have a head").hash,
        snap_a.hash
    );
    assert_eq!(
        beta.head().expect("beta should have a head").hash,
        snap_b.hash
    );

    // And each chain has exactly one entry — beta's snapshot did not
    // become a child of alpha's.
    assert_eq!(alpha.chain_length(None), 1);
    assert_eq!(beta.chain_length(None), 1);
    assert!(alpha.walk_chain(None)[0].parent.is_none());
    assert!(beta.walk_chain(None)[0].parent.is_none());

    // The messages do not leak across projects.
    assert!(alpha.walk_chain(None)[0].message.contains("alpha"));
    assert!(beta.walk_chain(None)[0].message.contains("beta"));
}

#[test]
fn test_a_second_project_does_not_become_a_child_of_the_first() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().join("blobs")).unwrap();
    let alpha = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Alpha")));
    let beta = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Beta")));

    let a = tmp.path().join("Alpha.als");
    let b = tmp.path().join("Beta.als");
    std::fs::write(&a, b"<LiveSet id='alpha'/>").unwrap();
    std::fs::write(&b, b"<LiveSet id='beta'/>").unwrap();

    alpha.save(&a, "alpha one", None, 1, 120.0, vec![], Default::default()).unwrap();
    beta.save(&b, "beta one", None, 1, 120.0, vec![], Default::default()).unwrap();


    // A second snapshot of alpha must chain from alpha's first snapshot,
    // not from beta's. The file has to change first, or the content hash
    // is the same and the snapshot is deduplicated.
    std::fs::write(&a, b"<LiveSet id='alpha' version='2'/>").unwrap();
    // The parent is the current HEAD of *this* project, as the CLI does.
    let alpha_head = alpha.head().expect("alpha head").hash;
    let a2 = alpha
        .save(&a, "alpha two", Some(&alpha_head), 1, 120.0, vec![], Default::default())
        .unwrap();
    assert_eq!(alpha.chain_length(None), 2);
    assert_eq!(beta.chain_length(None), 1);

    let chain = alpha.walk_chain(None);
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].hash, a2.hash);
    assert_eq!(
        chain[0].parent,
        chain[1].hash.clone().into(),
        "the second alpha snapshot should chain from the first"
    );
}

#[test]
fn test_the_legacy_shared_head_is_only_used_by_its_own_project() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().join("blobs")).unwrap();
    let alpha = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Alpha")));
    let beta = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Beta")));

    let a = tmp.path().join("Alpha.als");
    std::fs::write(&a, b"<LiveSet/>").unwrap();
    let snap = alpha.save(&a, "alpha work", None, 1, 120.0, vec![], Default::default()).unwrap();

    // Simulate the old layout: a single shared HEAD ref.
    store.update_ref("HEAD", &snap.hash).unwrap();

    // Alpha may use it — the snapshot belongs to it.
    assert_eq!(alpha.head().expect("alpha head").hash, snap.hash);
    // Beta must not: that is another project's chain.
    assert!(
        beta.head().is_none(),
        "beta must not inherit alpha's chain"
    );
}

// ─── Cross-project restore ──────────────────────────────────────────────────
//
// The blob store is shared by every project, so a lookup that falls back
// to "every snapshot we know about" would find another project's
// snapshot — and restoring it would overwrite this project's file with
// another project's contents.

#[test]
fn a_snapshot_of_another_project_cannot_be_resolved() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().join("blobs")).unwrap();
    let alpha = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Alpha")));
    let beta = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Beta")));

    let a = tmp.path().join("Alpha.als");
    std::fs::write(&a, b"<LiveSet id='alpha'/>").unwrap();
    let snap = alpha.save(&a, "alpha work", None, 1, 120.0, vec![], Default::default()).unwrap();

    // Alpha can find its own snapshot.
    assert_eq!(
        alpha.resolve(&snap.hash).expect("alpha should resolve its own"),
        snap.hash
    );

    // Beta cannot — that is another project's file, and restoring it
    // would overwrite Beta.als with Alpha's contents.
    let err = beta.resolve(&snap.hash).expect_err("beta must not resolve alpha's snapshot");
    assert!(
        matches!(err, ClavusError::SnapshotNotFound(_)),
        "unexpected error: {:?}",
        err
    );
}

#[test]
fn a_prefix_of_another_project_cannot_be_resolved() {
    let tmp = TempDir::new().unwrap();
    let store = BlobStore::new(tmp.path().join("blobs")).unwrap();
    let alpha = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Alpha")));
    let beta = SnapshotStore::new(store.clone(), &clavus_core::project::ProjectId::new(std::path::Path::new("Beta")));

    let a = tmp.path().join("Alpha.als");
    std::fs::write(&a, b"<LiveSet id='alpha'/>").unwrap();
    let snap = alpha.save(&a, "alpha work", None, 1, 120.0, vec![], Default::default()).unwrap();

    // Even a short prefix must not reach across.
    let prefix = &snap.hash[..8];
    assert!(beta.resolve(prefix).is_err());
    // But it still works for the project that owns it.
    assert_eq!(alpha.resolve(prefix).expect("alpha resolves its own"), snap.hash);
}
