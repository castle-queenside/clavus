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
fn a_relative_path_is_refused() {
    let tmp = TempDir::new().unwrap();
    let store = tmp.path().join(".clavus");
    let (_, result) = run(|o, e| {
        cmd_relocate(
            &RelocateAction::Map {
                from: "Volumes/Old".into(),
                to: "/Volumes/New".into(),
            },
            &store,
            o,
            e,
        )
    });
    let err = result.expect_err("a relative path should be refused");
    assert!(err.to_string().contains("absolute"), "{}", err);
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
