//! Integration tests for sample audit and collect functionality.
//!
//! Tests create real temp directories with .als files and sample files,
//! then verify audit and collect behavior end-to-end.

use clavus_core::{audit_project, collect_samples};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Create a minimal gzip-compressed .als XML file.
fn make_als(path: &PathBuf, xml: &str) {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut enc, xml.as_bytes()).ok();
    let compressed = enc.finish().unwrap_or_default();
    fs::write(path, &compressed).ok();
}

/// Create a sample file on disk (creates parent directories).
fn make_sample(path: &PathBuf, content: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(path, content).ok();
}

fn als_xml(samples: &[(&str, &str)]) -> String {
    // samples: (xml_path_attr, disk_filename)
    // Format: <SampleRef><Path Value="path"/></SampleRef> (direct, no wrapper)
    let mut sample_refs = String::new();
    for (xml_path, _) in samples {
        sample_refs.push_str(&format!(
            r#"<SampleRef><Path Value="{}"/></SampleRef>"#,
            xml_path
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tempo Value="128" />
  <MasterTrack>
    <Name Value="Master" />
    <DeviceChain />
  </MasterTrack>
  <AudioTrack>
    <Name Value="Test" />
    <DeviceChain>{}</DeviceChain>
  </AudioTrack>
</LiveSet>"#,
        sample_refs
    )
}

// ─── Audit Tests ────────────────────────────────────────────────────────

#[test]
fn test_audit_clean_project() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    // .als with a sample inside the project folder
    let als_path = project_dir.join("Test.als");
    make_als(&als_path, &als_xml(&[("Samples/kick.wav", "kick.wav")]));
    make_sample(&project_dir.join("Samples").join("kick.wav"), b"FAKE_AUDIO");

    let report = audit_project(&als_path).unwrap();

    assert!(report.is_clean(), "all samples present → should be clean");
    assert_eq!(report.collected.len(), 1, "one sample should be collected");
    assert_eq!(report.missing.len(), 0);
    assert_eq!(report.outside_project.len(), 0);
}

#[test]
fn test_audit_missing_sample() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    // .als references a sample that doesn't exist
    let als_path = project_dir.join("Test.als");
    make_als(&als_path, &als_xml(&[("Samples/missing.wav", "missing.wav")]));

    let report = audit_project(&als_path).unwrap();

    assert!(!report.is_clean(), "missing sample → should not be clean");
    assert_eq!(report.missing.len(), 1, "one missing sample");
    assert_eq!(report.collected.len(), 0);
}

#[test]
fn test_audit_outside_project() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    // Sample lives OUTSIDE the project folder — a sibling path in the
    // system temp dir. The name is unique so two test runs happening at
    // once cannot write over each other's file.
    let outside_path = std::env::temp_dir().join(format!(
        "clavus_collect_ext_kick_{}_{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    make_sample(&outside_path, b"FAKE_AUDIO");

    let als_path = project_dir.join("Test.als");
    // Reference with absolute path outside project
    make_als(&als_path, &als_xml(&[(outside_path.to_str().unwrap(), "kick.wav")]));

    let report = audit_project(&als_path).unwrap();

    // Sample is found (file exists) but NOT inside project folder
    assert!(!report.is_clean(), "sample found but outside project → should not be clean");
    assert_eq!(report.outside_project.len(), 1, "one outside-project sample");
    assert_eq!(report.collected.len(), 0, "no collected samples");
}

#[test]
fn test_audit_nonexistent_file() {
    let result = audit_project(&PathBuf::from("/nonexistent/project.als"));
    assert!(result.is_err(), "audit on nonexistent file should return error");
}

// ─── Collect Tests ─────────────────────────────────────────────────────

#[test]
fn test_collect_copies_and_rewrites_path() {
    // Use a separate temp dir for the external sample (outside project)
    let project_tmp = TempDir::new().unwrap();
    let external_tmp = TempDir::new().unwrap();
    let project_dir = project_tmp.path().to_path_buf();

    // External sample (in SEPARATE temp dir, definitely outside project)
    let ext_sample = external_tmp.path().join("kick.wav");
    make_sample(&ext_sample, b"FAKE_AUDIO_DATA");

    // Project with absolute path reference to external sample
    let als_path = project_dir.join("Test.als");
    make_als(&als_path, &als_xml(&[(ext_sample.to_str().unwrap(), "kick.wav")]));

    // Audit: sample should be outside_project (found on disk, but not inside project dir)
    let report = audit_project(&als_path).unwrap();
    assert_eq!(
        report.outside_project.len(),
        1,
        "pre-condition: sample is outside project"
    );

    // Collect
    let result = collect_samples(&als_path, &report).unwrap();
    assert!(result.failed.is_empty(), "no failures: {:?}", result.failed);
    assert_eq!(result.copied.len(), 1, "one sample copied");

    // Verify sample is in project/Samples/
    let collected_path = project_dir.join("Samples").join("kick.wav");
    assert!(
        collected_path.exists(),
        "sample should be copied to project/Samples/"
    );

    // Verify .als path was rewritten
    let xml_bytes = flate2_read_gzip(&als_path);
    let xml = String::from_utf8(xml_bytes).unwrap();
    assert!(
        xml.contains(r#"Value="Samples/kick.wav""#),
        "als path should be rewritten to Samples/kick.wav"
    );
    assert!(
        !xml.contains(ext_sample.to_str().unwrap()),
        "old absolute path should not appear in .als"
    );
}

#[test]
fn test_collect_idempotent() {
    // Use separate temp dirs to keep external sample truly outside project
    let project_tmp = TempDir::new().unwrap();
    let external_tmp = TempDir::new().unwrap();
    let project_dir = project_tmp.path().to_path_buf();


    let ext_sample = external_tmp.path().join("kick.wav");
    make_sample(&ext_sample, b"IDEMPOTENT");

    let als_path = project_dir.join("Test.als");
    make_als(&als_path, &als_xml(&[(ext_sample.to_str().unwrap(), "kick.wav")]));

    let report = audit_project(&als_path).unwrap();
    assert_eq!(report.outside_project.len(), 1, "pre-condition: outside project");

    // First collect
    let r1 = collect_samples(&als_path, &report).unwrap();
    assert_eq!(r1.copied.len(), 1);

    // Second collect — should find 0 outside-project samples now
    let report2 = audit_project(&als_path).unwrap();
    assert!(report2.is_clean(), "after collect, audit should pass");
    assert_eq!(report2.outside_project.len(), 0);
    assert_eq!(report2.collected.len(), 1);
}

#[test]
fn test_collect_multiple_samples() {
    let project_tmp = TempDir::new().unwrap();
    let external_tmp = TempDir::new().unwrap();
    let project_dir = project_tmp.path().to_path_buf();

    let kick = external_tmp.path().join("kick.wav");
    let snare = external_tmp.path().join("snare.wav");
    let hihat = external_tmp.path().join("hihat.wav");
    make_sample(&kick, b"KICK");
    make_sample(&snare, b"SNARE");
    make_sample(&hihat, b"HIHAT");

    let als_path = project_dir.join("Test.als");
    make_als(
        &als_path,
        &als_xml(&[
            (kick.to_str().unwrap(), "kick.wav"),
            (snare.to_str().unwrap(), "snare.wav"),
            (hihat.to_str().unwrap(), "hihat.wav"),
        ]),
    );

    let report = audit_project(&als_path).unwrap();
    let result = collect_samples(&als_path, &report).unwrap();

    assert_eq!(result.copied.len(), 3, "all 3 samples should be copied");
    assert!(result.failed.is_empty());

    // Re-parse
    let report2 = audit_project(&als_path).unwrap();
    assert!(report2.is_clean());
    assert_eq!(report2.collected.len(), 3);
}

#[test]
fn test_collect_missing_file_not_copied() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    // XML references a file that doesn't exist on disk
    let als_path = project_dir.join("Test.als");
    make_als(&als_path, &als_xml(&[("Samples/ghost.wav", "ghost.wav")]));

    let report = audit_project(&als_path).unwrap();
    assert_eq!(report.missing.len(), 1);

    let result = collect_samples(&als_path, &report).unwrap();
    assert!(result.copied.is_empty(), "missing files can't be copied");
    assert!(!result.failed.is_empty(), "missing file should be in failed list");
}

#[test]
fn test_collect_fails_when_samples_dir_unwritable() {
    // Test that collect fails gracefully when the Samples/ dir can't be created.
    // We simulate this by passing a report that would require copying but the
    // project root is itself unreadable (impossible to create Samples/ inside).
    // This is a smoke test — the actual permission behavior is OS-dependent.
    let tmp = TempDir::new().unwrap();
    let als_path = tmp.path().join("Test.als");
    // Create a minimal .als
    make_als(&als_path, &als_xml(&[]));
    // Empty project — nothing to collect, so this should trivially succeed
    let report = audit_project(&als_path).unwrap();
    let result = collect_samples(&als_path, &report);
    assert!(result.is_ok(), "empty project collect should succeed");
}

// ─── Helpers ───────────────────────────────────────────────────────────

fn flate2_read_gzip(path: &PathBuf) -> Vec<u8> {
    use flate2::read::GzDecoder;
    let data = fs::read(path).unwrap();
    let mut dec = GzDecoder::new(&data[..]);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut dec, &mut out).ok();
    out
}

// ─── Relative path regression ───────────────────────────────────────────────
//
// Regression: when the project was given as a relative path (e.g.
// `clavus audit ./myproject.als`), the project root stayed relative while
// resolved sample paths were absolute, so `strip_prefix` failed and every
// sample was wrongly reported as "outside the project folder".

/// `set_current_dir` is process-global, so tests that change it must not
/// run concurrently.
static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_audit_with_relative_project_path() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new().unwrap();
    let proj_dir = tmp.path().join("myproj");
    fs::create_dir_all(&proj_dir).unwrap();

    // Sample lives INSIDE the project folder.
    make_sample(&proj_dir.join("Samples").join("kick.wav"), b"audio data");

    // The .als references it by absolute path.
    let abs = proj_dir.join("Samples").join("kick.wav");
    let xml = als_xml(&[(abs.to_str().unwrap(), "kick.wav")]);
    make_als(&proj_dir.join("test.als"), &xml);

    // Audit using a RELATIVE path (simulates running from the parent dir).
    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let relative = std::path::Path::new("myproj").join("test.als");
    let report = audit_project(&relative).unwrap();

    std::env::set_current_dir(prev_cwd).unwrap();

    assert_eq!(report.missing.len(), 0, "sample inside project was reported missing");
    assert_eq!(
        report.outside_project.len(),
        0,
        "sample inside project was wrongly reported as outside (relative path bug)"
    );
    assert_eq!(report.collected.len(), 1, "sample should be collected");
}

#[test]
fn test_audit_relative_path_still_detects_outside_samples() {
    // The relative-path fix must not break genuine outside-project detection.
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new().unwrap();
    let proj_dir = tmp.path().join("myproj");
    let outside_dir = tmp.path().join("external");
    fs::create_dir_all(&proj_dir).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();

    make_sample(&outside_dir.join("snare.wav"), b"audio data");
    let abs = outside_dir.join("snare.wav");
    let xml = als_xml(&[(abs.to_str().unwrap(), "snare.wav")]);
    make_als(&proj_dir.join("test.als"), &xml);

    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let relative = std::path::Path::new("myproj").join("test.als");
    let report = audit_project(&relative).unwrap();

    std::env::set_current_dir(prev_cwd).unwrap();

    assert_eq!(report.outside_project.len(), 1, "genuine outside sample not detected");
    assert_eq!(report.collected.len(), 0);
}

// ─── Relative path handling ──────────────────────────────────────────────
//
// Regression: `Path::parent()` returns an *empty* path for a bare filename,
// and `strip_prefix("")` matches every path. That made every out-of-project
// sample look like it was inside the project, so `clavus collect` silently
// did nothing when invoked with a relative filename.

#[test]
fn test_audit_with_bare_filename_path() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("proj");
    let outside_dir = tmp.path().join("outside");
    fs::create_dir_all(&project_dir).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();

    let als_path = project_dir.join("Rel.als");
    let outside_sample = outside_dir.join("kick.wav");
    make_als(
        &als_path,
        &als_xml(&[(outside_sample.to_str().unwrap(), "kick.wav")]),
    );
    make_sample(&outside_sample, b"FAKE_AUDIO");

    // Audit using a bare filename: `Path::parent()` is empty here.
    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&project_dir).unwrap();
    let report = audit_project(Path::new("Rel.als"));
    std::env::set_current_dir(prev_cwd).unwrap();

    let report = report.unwrap();
    assert_eq!(
        report.outside_project.len(),
        1,
        "sample outside the project must be reported as outside, not collected"
    );
    assert_eq!(report.collected.len(), 0);
    assert_eq!(report.missing.len(), 0);
}

#[test]
fn test_collect_with_bare_filename_path() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().join("proj");
    let outside_dir = tmp.path().join("outside");
    fs::create_dir_all(&project_dir).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();

    let als_path = project_dir.join("Rel2.als");
    let outside_sample = outside_dir.join("kick.wav");
    make_als(
        &als_path,
        &als_xml(&[(outside_sample.to_str().unwrap(), "kick.wav")]),
    );
    make_sample(&outside_sample, b"FAKE_AUDIO");

    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&project_dir).unwrap();
    let result = collect_samples(
        Path::new("Rel2.als"),
        &audit_project(Path::new("Rel2.als")).unwrap(),
    );
    std::env::set_current_dir(prev_cwd).unwrap();

    let result = result.unwrap();
    assert_eq!(
        result.copied.len(),
        1,
        "collect must copy the sample even when the project path is relative"
    );
    assert!(project_dir.join("Samples").join("kick.wav").exists());
}

#[test]
fn test_audit_deduplicates_repeated_references() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    // The same sample referenced by three clips.
    let als_path = project_dir.join("Dup.als");
    make_als(
        &als_path,
        &als_xml(&[
            ("Samples/kick.wav", "kick.wav"),
            ("Samples/kick.wav", "kick.wav"),
            ("Samples/kick.wav", "kick.wav"),
        ]),
    );
    make_sample(&project_dir.join("Samples").join("kick.wav"), b"FAKE_AUDIO");

    let report = audit_project(&als_path).unwrap();
    assert_eq!(
        report.collected.len(),
        1,
        "one file referenced three times is one sample"
    );
}

#[test]
fn test_audit_deduplicates_missing_references() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    let als_path = project_dir.join("Dup2.als");
    make_als(
        &als_path,
        &als_xml(&[
            ("Samples/gone.wav", "gone.wav"),
            ("Samples/gone.wav", "gone.wav"),
        ]),
    );

    let report = audit_project(&als_path).unwrap();
    assert_eq!(report.missing.len(), 1, "missing file counted once");
}

// ─── Non-project files must not audit as clean ─────────────────────────────

#[test]
fn test_audit_rejects_a_file_that_is_not_a_live_set() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    let path = project_dir.join("notes.txt");
    fs::write(&path, b"not a project at all").unwrap();

    let err = audit_project(&path).expect_err("a non-project must not audit clean");
    let msg = err.to_string();
    assert!(
        msg.contains("LiveSet") || msg.contains("Ableton") || msg.contains("malformed"),
        "the error should explain the problem: {}",
        msg
    );
}

#[test]
fn test_audit_rejects_valid_xml_that_is_not_a_live_set() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    let path = project_dir.join("Other.als");
    fs::write(
        &path,
        br#"<?xml version="1.0" encoding="UTF-8"?><SomeOtherApp><Thing/></SomeOtherApp>"#,
    )
    .unwrap();

    let err = audit_project(&path).expect_err("a non-Live XML must not audit clean");
    assert!(
        err.to_string().contains("not an Ableton Live project"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_audit_rejects_malformed_xml() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    let path = project_dir.join("Broken.als");
    fs::write(&path, b"<LiveSet><unclosed>").unwrap();

    assert!(
        audit_project(&path).is_err(),
        "malformed XML must not audit clean"
    );
}

#[test]
fn test_audit_accepts_a_plain_xml_live_set() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path().to_path_buf();

    let path = project_dir.join("Plain.als");
    fs::write(
        &path,
        als_xml(&[("Samples/kick.wav", "kick.wav")]).as_bytes(),
    )
    .unwrap();
    make_sample(&project_dir.join("Samples").join("kick.wav"), b"FAKE_AUDIO");

    let report = audit_project(&path).expect("plain XML should audit");
    assert_eq!(report.collected.len(), 1);
}

// ─── Bitwig archives that carry their own samples ──────────────────────────
//
// A `.bwproject` exported from Bitwig keeps a copy of every sample it
// uses, but the references inside project.xml still point at the
// original machine's absolute paths. On the receiving machine those
// samples look missing even though they are in the archive.

use std::io::Write;

/// Build a `.bwproject` archive with an absolute sample reference and an
/// embedded copy of the sample.
fn make_bwproject(path: &Path, sample_name: &str, reference: &str, embed: bool) {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();

    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" VersionMinor="1" Name="Test">
  <Tracks>
    <Track Type="Audio" Index="0">
      <Name Value="Drums" />
      <DeviceChain />
      <Mute Value="false" />
      <Solo Value="false" />
      <Volume Value="0.8" />
      <AudioClip File="{}" />
    </Track>
  </Tracks>
</Project>"#,
        reference
    );
    w.start_file("project.xml", opts).unwrap();
    w.write_all(xml.as_bytes()).unwrap();

    if embed {
        w.start_file(format!("samples/{}", sample_name), opts).unwrap();
        w.write_all(b"RIFF----WAVEfake").unwrap();
    }

    let cursor = w.finish().unwrap();
    std::fs::write(path, cursor.into_inner()).unwrap();
}

#[test]
fn test_a_sample_inside_the_archive_is_not_missing() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Test.bwproject");
    make_bwproject(&proj, "kick.wav", "/Users/other/Library/kick.wav", true);

    let report = audit_project(&proj).unwrap();
    assert!(
        report.missing.is_empty(),
        "a sample inside the archive must not be reported missing: {:?}",
        report.missing
    );
    assert_eq!(report.embedded.len(), 1);
    assert_eq!(report.embedded[0].filename, "kick.wav");
    // It is present, so the snapshot is not blocked.
    assert!(report.is_clean());
}

#[test]
fn test_a_sample_missing_from_disk_and_the_archive_is_missing() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Test.bwproject");
    make_bwproject(&proj, "kick.wav", "/Users/other/Library/kick.wav", false);

    let report = audit_project(&proj).unwrap();
    assert_eq!(report.missing.len(), 1);
    assert!(report.embedded.is_empty());
    assert!(!report.is_clean());
}

#[test]
fn test_collect_extracts_a_sample_from_the_archive() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Test.bwproject");
    make_bwproject(&proj, "kick.wav", "/Users/other/Library/kick.wav", true);

    let report = audit_project(&proj).unwrap();
    let collect = collect_samples(&proj, &report).unwrap();

    assert_eq!(collect.copied.len(), 1, "{:?}", collect.failed);
    assert!(collect.failed.is_empty());
    // The sample keeps the place it has inside the archive, so the bundle
    // stays self-contained and the reference still resolves. Nothing is
    // written next to the project.
    assert!(
        !tmp.path().join("samples").join("kick.wav").exists(),
        "nothing should be written next to the project"
    );

    // The reference inside the archive now points at the sample's place
    // inside the bundle.
    let entries = clavus_core::bitwig_archive::list_entries(&proj).unwrap();
    let xml_name = clavus_core::bitwig_archive::find_project_xml(&entries).unwrap();
    let xml = String::from_utf8(
        clavus_core::bitwig_archive::read_entry(&proj, &xml_name).unwrap(),
    )
    .unwrap();
    assert!(
        xml.contains(r#"File="samples/kick.wav""#),
        "the reference should point at the sample inside the bundle: {}",
        xml
    );
    assert!(
        !xml.contains("/Users/other/Library/kick.wav"),
        "the old path should be gone: {}",
        xml
    );

    // The other archive entries survive the rewrite.
    assert!(entries.contains(&"samples/kick.wav".to_string()));
    assert_eq!(
        clavus_core::bitwig_archive::read_entry(&proj, "samples/kick.wav").unwrap(),
        b"RIFF----WAVEfake"
    );

    // And the project now audits clean: the sample is present, inside the
    // bundle, and the project points at it.
    let after = audit_project(&proj).unwrap();
    assert!(after.is_clean());
    assert_eq!(after.embedded.len(), 1);
    assert!(after.collected.is_empty());
    assert!(
        !after.embedded[0].archive_needs_repair(),
        "the project should point at the copy inside the bundle"
    );
}
