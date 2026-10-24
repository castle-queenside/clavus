//! The relocation map — where a path a set points at has moved to.
//!
//! A set writes down the absolute path of every sample it uses. When a
//! drive is renamed, a machine is rebuilt under a different user name, or
//! a library moves to another disk, those paths stop resolving and the
//! set looks like it has lost its audio.
//!
//! These tests drive the real map, the real audit and the real collect —
//! no reimplementation of the resolution order.

use clavus_core::relocate::{is_safe_source, is_safe_target, Relocation, Relocations};
use clavus_core::{audit_project_with, collect_samples};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// ─── Helpers ──────────────────────────────────────────────────────────────

/// A minimal gzip-compressed Live set that points at one sample.
fn write_als(path: &Path, sample_path: &str) {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <AudioTrack><Name Value="Drums" /><DeviceChain /></AudioTrack>
  <SampleRef><Path Value="{}" /></SampleRef>
</LiveSet>"#,
        sample_path
    );
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut enc, xml.as_bytes()).expect("gzip");
    let compressed = enc.finish().expect("gzip finish");
    fs::create_dir_all(path.parent().unwrap()).expect("project dir");
    fs::write(path, compressed).expect("write als");
}

/// A workspace with a project, a place the audio used to be, and a place
/// it is now.
struct Scene {
    _tmp: TempDir,
    /// Where the set says its sample is.
    old_root: PathBuf,
    /// Where the sample actually is.
    new_root: PathBuf,
    project: PathBuf,
    sample: PathBuf,
}

impl Scene {
    fn new() -> Self {
        let tmp = TempDir::new().expect("temp dir");
        let old_root = tmp.path().join("Old Disk").join("Music");
        let new_root = tmp.path().join("New Disk").join("Music");
        let project = tmp.path().join("Project");
        fs::create_dir_all(&project).expect("project dir");
        fs::create_dir_all(&old_root).expect("old root");
        fs::create_dir_all(&new_root).expect("new root");
        let sample = new_root.join("Samples").join("kick.wav");
        fs::create_dir_all(sample.parent().unwrap()).expect("sample dir");
        fs::write(&sample, b"RIFF----WAVEfake").expect("sample");
        Scene {
            _tmp: tmp,
            old_root,
            new_root,
            project,
            sample,
        }
    }

    /// The set, pointing at the old place.
    fn write_set(&self) -> PathBuf {
        let path = self.project.join("Set.als");
        write_als(
            &path,
            &self.old_root.join("Samples/kick.wav").to_string_lossy(),
        );
        path
    }
}

// ─── The map itself ───────────────────────────────────────────────────────

#[test]
fn a_rule_only_matches_on_a_path_boundary() {
    let map = Relocations {
        entries: vec![Relocation {
            from: "/Volumes/Set".into(),
            to: "/Volumes/LaCie".into(),
        }],
    };
    assert_eq!(
        map.candidates(Path::new("/Volumes/Set/a.wav")),
        vec![PathBuf::from("/Volumes/LaCie/a.wav")]
    );
    // "Set Two" is a different folder, and must not be rewritten.
    assert!(map
        .candidates(Path::new("/Volumes/Set Two/a.wav"))
        .is_empty());
}

#[test]
fn the_map_survives_a_round_trip_through_the_store() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");

    let mut map = Relocations::default();
    map.map("/Users/old", "/Users/new").expect("map");
    map.save(&store).expect("save");

    let loaded = Relocations::load(&store).expect("load");
    assert_eq!(loaded.entries.len(), 1);
    assert_eq!(loaded.entries[0].from, "/Users/old");
    assert_eq!(loaded.entries[0].to, "/Users/new");
}

#[test]
fn a_store_with_no_map_is_not_an_error() {
    let tmp = TempDir::new().unwrap();
    let map = Relocations::load(&tmp.path().join(".clavus")).expect("load");
    assert!(map.entries.is_empty());
}

#[test]
fn a_map_that_cannot_be_read_is_reported_rather_than_ignored() {
    // A map that exists but is broken is not an empty map. Treating it
    // as empty would report the user's audio as lost.
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join("relocations.json"), b"{ this is not json").unwrap();
    assert!(Relocations::load(&store).is_err());
}

#[test]
fn mapping_the_same_path_twice_replaces_the_rule() {
    let mut map = Relocations::default();
    map.map("/Volumes/Set", "/Volumes/LaCie").unwrap();
    map.map("/Volumes/Set", "/Volumes/Other").unwrap();
    assert_eq!(map.entries.len(), 1);
    assert_eq!(map.entries[0].to, "/Volumes/Other");
}

#[test]
fn a_destination_has_to_be_an_absolute_path_here() {
    assert!(is_safe_target("/Volumes/LaCie"));
    assert!(!is_safe_target("Volumes/LaCie"));
    assert!(!is_safe_target("/Volumes/LaCie/../../etc"));
}

#[test]
fn the_start_may_be_relative_or_from_a_windows_machine() {
    // A set made on a PC writes `C:/Users/Chris/...`, and a set that
    // refers to a library by its folder name writes a relative path.
    // Both are the reference as written, and the rule has to be able to
    // name them.
    assert!(is_safe_source("C:/Users/Chris/Music"));
    assert!(is_safe_source("Beat Lab/Samples"));
    assert!(!is_safe_source("../etc"));
    assert!(!is_safe_source("Samples/../../etc"));
}

#[test]
fn a_rule_that_points_at_itself_is_refused() {
    let mut map = Relocations::default();
    assert!(map.map("/Volumes/Set", "/Volumes/Set").is_err());
}

// ─── The audit ────────────────────────────────────────────────────────────

#[test]
fn a_sample_whose_path_moved_is_found_again() {
    let scene = Scene::new();
    let set = scene.write_set();

    // Without the map the audio is lost.
    let report = audit_project_with(&set, &Relocations::default()).expect("audit");
    assert_eq!(report.missing.len(), 1, "the old path does not resolve");
    assert!(!report.is_clean());

    // With the map it is found — but outside the project, which is a
    // different thing from missing.
    let mut map = Relocations::default();
    map.map(
        &scene.old_root.to_string_lossy(),
        &scene.new_root.to_string_lossy(),
    )
    .unwrap();
    let report = audit_project_with(&set, &map).expect("audit");
    assert!(report.missing.is_empty(), "the map finds it again");
    assert_eq!(report.outside_project.len(), 1);
    let found = report.outside_project[0]
        .resolved_path
        .as_ref()
        .expect("the report says where the audio is");
    assert_eq!(
        fs::canonicalize(found).unwrap(),
        fs::canonicalize(&scene.sample).unwrap(),
        "the report says where the audio is"
    );
}

#[test]
fn a_map_that_is_out_of_date_cannot_move_a_sample_that_is_already_there() {
    // The map is asked last, on purpose: a path that still resolves
    // where the set says it does is used as it is.
    let scene = Scene::new();
    let set = scene.project.join("Set.als");
    write_als(&set, &scene.sample.to_string_lossy());

    let mut map = Relocations::default();
    map.map("/Users/someone-else", "/Volumes/Else").unwrap();
    let report = audit_project_with(&set, &map).expect("audit");
    assert_eq!(
        report.outside_project.len(),
        1,
        "the sample is found where the set says it is"
    );
    let found = report.outside_project[0]
        .resolved_path
        .as_ref()
        .expect("the sample was found");
    assert_eq!(
        fs::canonicalize(found).unwrap(),
        fs::canonicalize(&scene.sample).unwrap(),
        "a stale rule must not move a sample that is already where the set says"
    );
    assert!(report.missing.is_empty());
}

#[test]
fn collecting_after_a_relocation_brings_the_audio_home() {
    let scene = Scene::new();
    let set = scene.write_set();

    let mut map = Relocations::default();
    map.map(
        &scene.old_root.to_string_lossy(),
        &scene.new_root.to_string_lossy(),
    )
    .unwrap();
    let report = audit_project_with(&set, &map).expect("audit");
    assert_eq!(report.outside_project.len(), 1);

    let collect = collect_samples(&set, &report).expect("collect");
    assert_eq!(
        collect.failed.len(),
        0,
        "nothing failed: {:?}",
        collect.failed
    );

    // The audio is now part of the project...
    let copied = scene.project.join("Samples").join("kick.wav");
    assert!(copied.exists(), "the sample was copied in");
    assert_eq!(
        fs::read(&copied).unwrap(),
        fs::read(&scene.sample).unwrap(),
        "the bytes are the user's own"
    );

    // ...and the set points at it.
    let after = audit_project_with(&set, &map).expect("audit");
    assert!(after.is_clean(), "the project is clean: {:?}", after);
}

// ── the suggestion engine ──────────────────────────────────────────────

fn touch(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, bytes).unwrap();
}

#[test]
fn a_reference_that_moved_gets_a_rule() {
    let tmp = TempDir::new().unwrap();
    let lib = tmp.path().join("library");
    touch(&lib.join("New Pack/Kick.wav"), b"kick bytes");

    let suggestions = clavus_core::relocate::suggest_rules(
        &[PathBuf::from("/Old/Machine/Library/Kick.wav")],
        &lib,
    );

    assert_eq!(suggestions.rules.len(), 1);
    assert_eq!(suggestions.rules[0].from, "/Old/Machine/Library");
    assert_eq!(
        suggestions.rules[0].to,
        lib.join("New Pack").to_string_lossy()
    );
    assert_eq!(suggestions.rules[0].references, 1);
    assert_eq!(suggestions.ambiguous, 0);
    assert_eq!(suggestions.unmatched, 0);
}

#[test]
fn a_rule_that_was_worked_out_actually_resolves_the_reference() {
    let tmp = TempDir::new().unwrap();
    let lib = tmp.path().join("library");
    touch(&lib.join("Packs/Beat Lab/Kick.wav"), b"kick");

    let reference = PathBuf::from("Beat Lab/Kick.wav");
    let suggestions = clavus_core::relocate::suggest_rules(std::slice::from_ref(&reference), &lib);
    assert_eq!(suggestions.rules.len(), 1);

    // The rule has to work when it is actually used, not just look right.
    let mut map = Relocations::default();
    map.map(&suggestions.rules[0].from, &suggestions.rules[0].to)
        .expect("the rule it suggested has to be a usable rule");
    let candidates = map.candidates(&reference);
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].exists());
    assert_eq!(fs::read(&candidates[0]).unwrap(), b"kick");
}

#[test]
fn a_sample_that_is_not_on_the_drive_at_all_is_reported_as_gone() {
    let tmp = TempDir::new().unwrap();
    let lib = tmp.path().join("library");
    touch(&lib.join("something-else.wav"), b"x");

    let suggestions = clavus_core::relocate::suggest_rules(
        &[PathBuf::from("/Old/Machine/Library/Not Here.wav")],
        &lib,
    );

    assert!(suggestions.rules.is_empty());
    assert_eq!(suggestions.unmatched, 1);
    assert_eq!(suggestions.ambiguous, 0);
}

#[test]
fn two_different_files_of_the_same_name_are_ambiguous() {
    let tmp = TempDir::new().unwrap();
    let lib = tmp.path().join("library");
    touch(&lib.join("A/Kick.wav"), b"one");
    touch(&lib.join("B/Kick.wav"), b"two different");

    let suggestions =
        clavus_core::relocate::suggest_rules(&[PathBuf::from("/Old/Library/Kick.wav")], &lib);

    assert!(suggestions.rules.is_empty());
    assert_eq!(suggestions.ambiguous, 1);
}

#[test]
fn copies_of_the_same_audio_are_not_a_real_ambiguity() {
    let tmp = TempDir::new().unwrap();
    let lib = tmp.path().join("library");
    touch(&lib.join("A/Kick.wav"), b"the very same bytes");
    touch(&lib.join("B/Kick.wav"), b"the very same bytes");

    let suggestions =
        clavus_core::relocate::suggest_rules(&[PathBuf::from("/Old/Library/Kick.wav")], &lib);

    assert_eq!(suggestions.rules.len(), 1);
    assert_eq!(suggestions.ambiguous, 0);
}

#[test]
fn a_trailing_slash_does_not_confuse_the_rule() {
    let tmp = TempDir::new().unwrap();
    let lib = tmp.path().join("library");
    touch(&lib.join("New Pack/Kick.wav"), b"kick");

    // Some sets write a path with a trailing slash on it.
    let suggestions = clavus_core::relocate::suggest_rules(
        &[PathBuf::from("/Old/Machine/Library/Kick.wav/")],
        &lib,
    );

    assert_eq!(suggestions.rules.len(), 1);
    assert_eq!(suggestions.rules[0].from, "/Old/Machine/Library");
}

#[test]
fn the_rules_are_offered_most_valuable_first() {
    let tmp = TempDir::new().unwrap();
    let lib = tmp.path().join("library");
    touch(&lib.join("New/Kick.wav"), b"kick");
    touch(&lib.join("New/Snare.wav"), b"snare");
    touch(&lib.join("New/Clap.wav"), b"clap");

    let references = vec![
        PathBuf::from("/Old/A/Kick.wav"),
        PathBuf::from("/Old/A/Snare.wav"),
        PathBuf::from("/Old/A/Clap.wav"),
    ];
    let suggestions = clavus_core::relocate::suggest_rules(&references, &lib);

    assert_eq!(suggestions.rules.len(), 1);
    assert_eq!(suggestions.rules[0].references, 3);
}
