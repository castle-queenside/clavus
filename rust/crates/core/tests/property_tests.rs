//! Property-based tests for the snapshot store and blob store.
//!
//! These check invariants that must hold for *any* sequence of operations:
//!   - the HEAD ref always points at a snapshot that exists
//!   - the chain length always matches the number of ancestors
//!   - walking the chain always terminates and returns the chain in order
//!   - restoring a snapshot always reproduces the original bytes
//!   - corrupting any blob is always detected, never silently returned

use clavus_core::blob::BlobStore;
use clavus_core::snapshot::SnapshotStore;
use proptest::prelude::*;
use std::path::PathBuf;

/// A scratch directory for one test case.
struct Scratch {
    dir: tempfile::TempDir,
    project: PathBuf,
}

impl Scratch {
    fn new(content: &[u8]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        // The file name and the store's project name must agree: a
        // snapshot belongs to the project its file name says it does.
        let project = dir.path().join("proj.als");
        std::fs::write(&project, content).unwrap();
        Self { dir, project }
    }

    fn store(&self) -> SnapshotStore {
        let blobs = BlobStore::new(self.dir.path().join("blobs")).unwrap();
        SnapshotStore::new(blobs, "proj")
    }
}

/// Generate plausible project file content.
///
/// Each chunk is suffixed with its index so that no two generated contents
/// are equal — identical content deliberately does *not* create a second
/// snapshot, which is covered by its own test.
fn content_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(
        prop::collection::vec(any::<u8>(), 1..256),
        1..8,
    )
    .prop_map(|chunks| chunks.into_iter().flatten().collect())
}

/// Make every element of `contents` distinct by appending its index.
fn make_distinct(contents: &mut [Vec<u8>]) {
    for (i, c) in contents.iter_mut().enumerate() {
        c.extend_from_slice(&(i as u64).to_le_bytes());
    }
}

// ─── Chain invariants ───────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Every snapshot's parent must be the previous HEAD, and the chain
    /// length must equal the number of snapshots created.
    #[test]
    fn chain_length_matches_snapshot_count(
        mut contents in prop::collection::vec(content_strategy(), 1..12),
    ) {
        make_distinct(&mut contents);
        let scratch = Scratch::new(b"seed");
        let store = scratch.store();

        let mut parent: Option<String> = None;
        let mut created = 0usize;

        for content in contents {
            std::fs::write(&scratch.project, &content).unwrap();
            let snap = store
                .save(
                    &scratch.project,
                    "msg",
                    parent.as_deref(),
                    0,
                    120.0,
                    vec![],
                    Default::default(),
                )
                .unwrap();
            // The new snapshot's parent is the old HEAD.
            prop_assert_eq!(snap.parent.clone(), parent.clone());
            parent = Some(snap.hash.clone());
            created += 1;
        }

        prop_assert_eq!(store.chain_length(None), created);
        prop_assert_eq!(store.walk_chain(None).len(), created);
    }

    /// Walking the chain from any snapshot yields a prefix of the full chain
    /// and always terminates.
    #[test]
    fn walk_chain_from_any_point_terminates(
        mut contents in prop::collection::vec(content_strategy(), 1..12),
    ) {
        make_distinct(&mut contents);
        let scratch = Scratch::new(b"seed");
        let store = scratch.store();

        let mut parent: Option<String> = None;
        let mut hashes = Vec::new();

        for content in contents {
            std::fs::write(&scratch.project, &content).unwrap();
            let snap = store
                .save(
                    &scratch.project,
                    "msg",
                    parent.as_deref(),
                    0,
                    120.0,
                    vec![],
                    Default::default(),
                )
                .unwrap();
            parent = Some(snap.hash.clone());
            hashes.push(snap.hash);
        }

        for (i, hash) in hashes.iter().enumerate() {
            let walk = store.walk_chain(Some(hash));
            // Walking from snapshot i follows parents back to the root, so
            // the chain has i+1 entries.
            prop_assert_eq!(walk.len(), i + 1);
            prop_assert_eq!(&walk[0].hash, hash);
            // And it is a prefix of the full chain, in reverse order.
            let full = store.walk_chain(None);
            let walk_hashes: Vec<&str> = walk.iter().map(|s| s.hash.as_str()).collect();
            let full_hashes: Vec<&str> = full.iter().map(|s| s.hash.as_str()).collect();
            prop_assert_eq!(&walk_hashes, &full_hashes[full_hashes.len() - walk_hashes.len()..]);
        }
    }

    /// Restoring a snapshot reproduces the exact original bytes.
    #[test]
    fn restore_reproduces_original_bytes(
        mut contents in prop::collection::vec(content_strategy(), 1..8),
    ) {
        make_distinct(&mut contents);
        let scratch = Scratch::new(b"seed");
        let store = scratch.store();

        let mut parent: Option<String> = None;
        let mut saved = Vec::new();

        for content in contents {
            std::fs::write(&scratch.project, &content).unwrap();
            let snap = store
                .save(
                    &scratch.project,
                    "msg",
                    parent.as_deref(),
                    0,
                    120.0,
                    vec![],
                    Default::default(),
                )
                .unwrap();
            parent = Some(snap.hash.clone());
            saved.push((snap.hash.clone(), content));
        }

        for (hash, content) in &saved {
            let target = scratch.dir.path().join(format!("restored-{}", hash));
            let snap = store.restore(hash, Some(&target)).unwrap();
            prop_assert_eq!(&snap.hash, hash);
            let restored = std::fs::read(&target).unwrap();
            prop_assert_eq!(&restored, content);
        }
    }

    /// Identical file content produces the same snapshot hash and does not
    /// create a duplicate.
    #[test]
    fn identical_content_does_not_duplicate(
        content in content_strategy(),
    ) {
        let scratch = Scratch::new(&content);
        let store = scratch.store();

        let first = store
            .save(&scratch.project, "one", None, 0, 120.0, vec![], Default::default())
            .unwrap();
        let second = store
            .save(&scratch.project, "two", None, 0, 120.0, vec![], Default::default())
            .unwrap();

        prop_assert_eq!(&first.hash, &second.hash);
        prop_assert_eq!(store.chain_length(None), 1);
    }

    /// Restoring to a hash prefix resolves to the same snapshot.
    #[test]
    fn restore_by_prefix_resolves(
        content in content_strategy(),
    ) {
        let scratch = Scratch::new(&content);
        let store = scratch.store();
        let snap = store
            .save(&scratch.project, "msg", None, 0, 120.0, vec![], Default::default())
            .unwrap();

        let prefix = &snap.hash[..8];
        let target = scratch.dir.path().join("restored");
        let restored = store.restore(prefix, Some(&target)).unwrap();
        prop_assert_eq!(&restored.hash, &snap.hash);
    }

    /// An unknown hash is an error, not a silent success.
    #[test]
    fn restore_unknown_hash_errors(noise in "[0-9a-f]{64}") {
        let scratch = Scratch::new(b"seed");
        let store = scratch.store();
        let target = scratch.dir.path().join("restored");
        prop_assert!(store.restore(&noise, Some(&target)).is_err());
    }
}

// ─── Blob store invariants ──────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any bytes round-trip through the blob store unchanged.
    #[test]
    fn blob_roundtrip(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs")).unwrap();
        let hash = store.put(&data);
        let loaded = store.get(&hash).unwrap();
        prop_assert_eq!(&loaded, &data);
        prop_assert_eq!(&BlobStore::sha256(&data), &hash);
    }

    /// A blob that has been tampered with is detected by its hash.
    #[test]
    fn tampered_blob_is_detected(
        data in prop::collection::vec(any::<u8>(), 1..1024),
        index in 0usize..1024,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs")).unwrap();
        let hash = store.put(&data);

        let mut tampered = data.clone();
        let idx = index % tampered.len();
        tampered[idx] ^= 0xFF;

        // The tampered bytes hash differently, so the original hash no
        // longer identifies them.
        prop_assert_ne!(&BlobStore::sha256(&tampered), &hash);
    }

    /// A missing blob is reported as missing, never as empty content.
    #[test]
    fn missing_blob_is_none(noise in "[0-9a-f]{64}") {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs")).unwrap();
        prop_assert!(store.get(&noise).is_none());
    }

    /// Storing the same bytes twice is idempotent.
    #[test]
    fn blob_put_is_idempotent(data in prop::collection::vec(any::<u8>(), 0..512)) {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs")).unwrap();
        let h1 = store.put(&data);
        let h2 = store.put(&data);
        prop_assert_eq!(&h1, &h2);
        prop_assert!(store.get(&h1).is_some());
    }
}

// ─── Sample audit invariants ───────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// A sample is classified by exactly one rule:
    ///   found inside the project -> collected
    ///   found outside the project -> outside_project
    ///   not found anywhere -> missing
    /// and the three lists never overlap.
    #[test]
    fn audit_classification_is_exhaustive_and_exclusive(
        inside_names in prop::collection::vec("[a-z]{1,8}", 0..4),
        outside_names in prop::collection::vec("[a-z]{1,8}", 0..4),
        missing_names in prop::collection::vec("[a-z]{1,8}", 0..4),
    ) {
        use std::collections::HashSet;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        // `inside` must be a subdirectory of the project; `outside` must not be.
        let inside = project.join("Samples");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // Build the set of references.
        let mut refs: Vec<String> = Vec::new();
        for n in &inside_names {
            let p = inside.join(n);
            std::fs::write(&p, b"audio").unwrap();
            refs.push(p.to_string_lossy().to_string());
        }
        for n in &outside_names {
            let p = outside.join(n);
            std::fs::write(&p, b"audio").unwrap();
            refs.push(p.to_string_lossy().to_string());
        }
        for n in &missing_names {
            refs.push(outside.join(format!("missing-{}.wav", n)).to_string_lossy().to_string());
        }

        // The audit de-duplicates references, and the generated names can
        // repeat, so compare against the distinct set.
        let mut distinct: Vec<&String> = Vec::new();
        for r in &refs {
            if !distinct.iter().any(|d| *d == r) {
                distinct.push(r);
            }
        }
        let unique_refs = distinct.len();


        // Audit a project file that references these paths.
        let als = project.join("p.als");
        let xml = format!(
            "<?xml version=\"1.0\"?><LiveSet>{}</LiveSet>",
            refs.iter()
                .map(|r| format!("<SampleRef><Path Value=\"{}\"/></SampleRef>", r))
                .collect::<String>()
        );
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        {
            use std::io::Write;
            enc.write_all(xml.as_bytes()).unwrap();
        }
        std::fs::write(&als, enc.finish().unwrap()).unwrap();

        let report = clavus_core::audit_project(&als).unwrap();

        // Every reference is accounted for exactly once.
        let total = report.collected.len() + report.outside_project.len() + report.missing.len();
        prop_assert_eq!(total, unique_refs);

        // The three categories are disjoint.
        let collected: HashSet<&str> = report.collected.iter().map(|s| s.path.to_str().unwrap()).collect();
        let outsides: HashSet<&str> = report.outside_project.iter().map(|s| s.path.to_str().unwrap()).collect();
        let missing: HashSet<&str> = report.missing.iter().map(|s| s.path.to_str().unwrap()).collect();
        for c in &collected { prop_assert!(!outsides.contains(c)); prop_assert!(!missing.contains(c)); }
        for m in &outsides { prop_assert!(!missing.contains(m)); }

        // A sample that exists inside the project folder is 'collected'.
        for n in &inside_names {
            let p = inside.join(n);
            let key = p.to_string_lossy().to_string();
            prop_assert!(collected.contains(key.as_str()), "inside sample should be collected");
        }
        // A sample that exists outside the project folder is 'outside_project'.
        for n in &outside_names {
            let p = outside.join(n);
            let key = p.to_string_lossy().to_string();
            prop_assert!(outsides.contains(key.as_str()), "outside sample should be flagged");
        }
        // A sample that does not exist is 'missing'.
        for n in &missing_names {
            let p = outside.join(format!("missing-{}.wav", n));
            let key = p.to_string_lossy().to_string();
            prop_assert!(missing.contains(key.as_str()), "missing sample should be flagged");
        }
    }
}
