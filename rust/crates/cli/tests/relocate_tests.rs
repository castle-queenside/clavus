//! The relocation map, from the command line.
//!
//! A set writes down the absolute path of every sample it uses. When a
//! drive is renamed or a library moves, those paths stop resolving and
//! the set looks like it has lost its audio. `clavus relocate` is where
//! the user says what moved where.

use clavus_cli::{cmd_relocate, RelocateAction};
use std::fs;
use std::path::PathBuf;
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

/// A minimal gzip-compressed Live set that points at one sample.
fn write_als(path: &PathBuf, sample_path: &str) {
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

// ─── The command ──────────────────────────────────────────────────────────

#[test]
fn a_store_with_no_map_says_so() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    let (out, result) = run(|o, e| cmd_relocate(&RelocateAction::List, &store, o, e));
    assert!(result.is_ok());
    assert!(
        out.all().contains("No rules"),
        "an empty map should say so: {}",
        out.all()
    );
}

#[test]
fn a_rule_is_written_and_shown_again() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");

    let (out, result) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Map {
                from: "/Volumes/Old".into(),
                to: "/Volumes/New".into(),
            },
            &store,
            o,
            e,
        )
    });
    assert!(result.is_ok(), "{}", out.all());
    assert!(out.all().contains("/Volumes/New"), "{}", out.all());

    // The rule is on disk, not just in the message.
    let saved = fs::read_to_string(store.join("relocations.json")).expect("the map was written");
    assert!(saved.contains("/Volumes/Old"), "{}", saved);

    // And it is listed.
    let (out, _) = run(|o, e| cmd_relocate(&RelocateAction::List, &store, o, e));
    assert!(out.all().contains("/Volumes/Old"), "{}", out.all());
    assert!(out.all().contains("/Volumes/New"), "{}", out.all());
}

#[test]
fn a_rule_can_be_removed() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    let _ = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Map {
                from: "/Volumes/Old".into(),
                to: "/Volumes/New".into(),
            },
            &store,
            o,
            e,
        )
    });

    let (out, _) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Unmap {
                from: "/Volumes/Old".into(),
            },
            &store,
            o,
            e,
        )
    });
    assert!(out.all().contains("Removed"), "{}", out.all());

    let (out, _) = run(|o, e| cmd_relocate(&RelocateAction::List, &store, o, e));
    assert!(out.all().contains("No rules"), "{}", out.all());
}

#[test]
fn removing_a_rule_that_is_not_there_says_so() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    let (_, result) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Unmap {
                from: "/Volumes/Nothing".into(),
            },
            &store,
            o,
            e,
        )
    });
    let err = result.expect_err("removing a rule that is not there should fail");
    assert!(err.to_string().contains("no rule"), "{}", err);
}

#[test]
fn a_relative_destination_is_refused() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    let (_, result) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Map {
                from: "/Volumes/Old".into(),
                to: "Volumes/New".into(),
            },
            &store,
            o,
            e,
        )
    });
    let err = result.expect_err("a relative destination should be refused");
    assert!(err.to_string().contains("absolute"), "{}", err);
}

#[test]
fn a_relative_start_is_allowed_because_a_set_writes_it_that_way() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    // A set that refers to its library by folder name, or that was made
    // on a PC, writes a path that is not absolute. The rule has to be
    // able to name that path as it was written.
    let (out, result) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Map {
                from: "Beat Lab/Samples".into(),
                to: "/Volumes/LaCie/New Packs".into(),
            },
            &store,
            o,
            e,
        )
    });
    assert!(result.is_ok(), "{}", out.all());
    let (out, _) = run(|o, e| cmd_relocate(&RelocateAction::List, &store, o, e));
    assert!(out.stdout.contains("Beat Lab/Samples"), "{}", out.all());
}

#[test]
fn a_path_that_climbs_out_is_refused() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    let (_, result) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Map {
                from: "/Volumes/Old".into(),
                to: "/Volumes/../../etc".into(),
            },
            &store,
            o,
            e,
        )
    });
    let err = result.expect_err("a path that climbs out should be refused");
    assert!(err.to_string().contains("absolute"), "{}", err);
}

// ─── The map in the audit ─────────────────────────────────────────────────

#[test]
fn the_audit_finds_a_sample_whose_path_moved() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    let old_root = tmp.path().join("Old Disk");
    let new_root = tmp.path().join("New Disk");
    let project = tmp.path().join("Project");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(new_root.join("Samples")).unwrap();
    fs::write(new_root.join("Samples").join("kick.wav"), b"RIFFfake").unwrap();

    let als = project.join("Set.als");
    write_als(&als, &old_root.join("Samples/kick.wav").to_string_lossy());

    // Without the map the audio is missing.
    let (out, _) = run(|o, e| clavus_cli::cmd_audit(&als, false, &store, o, e));
    assert!(out.stdout.contains("missing"), "{}", out.all());

    // With the map it is found, and the report says where.
    let _ = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Map {
                from: old_root.to_string_lossy().into_owned(),
                to: new_root.to_string_lossy().into_owned(),
            },
            &store,
            o,
            e,
        )
    });
    let (out, _) = run(|o, e| clavus_cli::cmd_audit(&als, false, &store, o, e));
    assert!(!out.stdout.contains("missing"), "{}", out.all());
    assert!(
        out.stdout.contains("found at"),
        "the report must say where the audio is: {}",
        out.all()
    );
    assert!(
        out.stdout.contains("New Disk"),
        "the report must name the place: {}",
        out.all()
    );
}

// ─── The suggestions ─────────────────────────────────────────────────────

/// A set that points at a sample which is not where it says.
fn a_set_pointing_at_a_moved_sample(root: &std::path::Path) -> (PathBuf, PathBuf) {
    let project = root.join("Project");
    let library = root.join("New Library");
    let sample = library.join("Kick.wav");
    fs::create_dir_all(&project).expect("project dir");
    fs::create_dir_all(sample.parent().unwrap()).expect("sample dir");
    fs::write(&sample, b"kick").expect("write sample");

    let als = project.join("Set.als");
    write_als(&als, "/Old/Machine/Library/Kick.wav");
    (als, library)
}

#[test]
fn clavus_suggests_the_rule_that_would_put_the_sample_back() {
    let tmp = TempDir::new().expect("temp dir");
    let store = tmp.path().join("store");
    let (als, library) = a_set_pointing_at_a_moved_sample(tmp.path());

    let (out, _) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Suggest {
                root: library.clone(),
                projects: vec![als.clone()],
                dir: None,
            },
            &store,
            o,
            e,
        )
    });

    assert!(
        out.stdout.contains("relocate map"),
        "the command must print the rule to run: {}",
        out.all()
    );
    assert!(
        out.stdout.contains("/Old/Machine/Library"),
        "the rule must name the path the set points at: {}",
        out.all()
    );
    assert!(
        out.stdout.contains("New Library"),
        "the rule must name where the audio is now: {}",
        out.all()
    );
}

#[test]
fn clavus_says_when_there_is_nothing_to_suggest() {
    let tmp = TempDir::new().expect("temp dir");
    let store = tmp.path().join("store");
    let library = tmp.path().join("Library");
    fs::create_dir_all(&library).expect("library");

    let (out, _) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Suggest {
                root: library,
                projects: vec![],
                dir: Some(tmp.path().join("no sets here").clone()),
            },
            &store,
            o,
            e,
        )
    });
    // With no sets named and no standard locations, the command has to
    // say so rather than quietly doing nothing.
    assert!(
        out.stderr.contains("standard locations") || out.stdout.contains("nothing to suggest"),
        "{}",
        out.all()
    );
}

#[test]
fn a_suggestion_needs_a_folder_that_exists() {
    let tmp = TempDir::new().expect("temp dir");
    let store = tmp.path().join("store");
    let (_, result) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Suggest {
                root: tmp.path().join("not there"),
                projects: vec![],
                dir: None,
            },
            &store,
            o,
            e,
        )
    });
    let err = result.expect_err("a missing folder has to be an error");
    assert!(err.to_string().contains("does not exist"), "{}", err);
}
