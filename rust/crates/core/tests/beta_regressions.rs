//! Regression tests for the failures the beta lab found.
//!
//! Every test here reproduces a bug that a real beta tester hit against
//! the compiled binary, and asserts the corrected behaviour.

use clavus_core::sample::{audit_project, collect_samples};
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

// ─── Bitwig bundle integrity ─────────────────────────────────────────────

/// Build a `.bwproject` archive from raw parts.
fn write_bwproject(path: &Path, project_xml: Option<&str>, entries: &[(&str, &[u8])]) {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    if let Some(xml) = project_xml {
        w.start_file("project.xml", opts).unwrap();
        w.write_all(xml.as_bytes()).unwrap();
    }
    for (name, data) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(data).unwrap();
    }
    let cursor = w.finish().unwrap();
    std::fs::write(path, cursor.into_inner()).unwrap();
}

const GOOD_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" VersionMinor="1" Name="Test">
  <Tracks>
    <Track Type="Audio" Index="0">
      <Name Value="Drums" />
      <AudioClip File="/Users/music/Library/Samples/kick.wav" />
    </Track>
  </Tracks>
</Project>"#;

#[test]
fn a_bundle_without_project_xml_is_not_a_healthy_project() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Broken.bwproject");
    write_bwproject(&proj, None, &[("readme.txt", b"not a project")]);

    let err = audit_project(&proj).expect_err("a bundle without project.xml must be rejected");
    assert!(
        err.to_string().contains("project.xml"),
        "the error should name the missing file: {}",
        err
    );
}

#[test]
fn a_bundle_with_malformed_project_xml_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Broken.bwproject");
    write_bwproject(&proj, Some("<Project><Tracks><Track>"), &[]);

    let err = audit_project(&proj).expect_err("a malformed project.xml must be rejected");
    assert!(
        err.to_string().contains("malformed"),
        "the error should say the document is malformed: {}",
        err
    );
}

#[test]
fn a_bundle_with_project_xml_that_is_not_a_project_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Wrong.bwproject");
    write_bwproject(&proj, Some("<NotAProject><Thing/></NotAProject>"), &[]);

    let err = audit_project(&proj).expect_err("a project.xml without <Project> must be rejected");
    assert!(
        err.to_string().contains("not a Bitwig project"),
        "the error should say what is wrong: {}",
        err
    );
}

#[test]
fn a_set_truncated_by_one_byte_is_rejected() {
    // Regression: the XML reader accepts a closing tag whose `>` is
    // missing, so a Live set truncated by a single byte parsed as a
    // complete set with every track still in it. It audited clean and
    // could be snapshotted as if it were healthy.
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("Truncated.als");
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tracks>
    <Track Type="Audio" Index="0">
      <Name Value="Drums" />
    </Track>
  </Tracks>
</LiveSet>"#;
    // Cut the last byte — the `>` of the closing tag.
    let truncated = &xml[..xml.len() - 1];
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        use std::io::Write;
        enc.write_all(truncated.as_bytes()).unwrap();
    }
    std::fs::write(&project, enc.finish().unwrap()).unwrap();

    let err = audit_project(&project).expect_err("a truncated set must not audit clean");
    assert!(
        err.to_string().contains("truncated") || err.to_string().contains("malformed"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn a_complete_set_is_accepted() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("Whole.als");
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tracks>
    <Track Type="Audio" Index="0">
      <Name Value="Drums" />
    </Track>
  </Tracks>
</LiveSet>"#;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        use std::io::Write;
        enc.write_all(xml.as_bytes()).unwrap();
    }
    std::fs::write(&project, enc.finish().unwrap()).unwrap();

    let report = audit_project(&project).expect("a whole set must audit");
    assert!(report.is_clean());
}

#[test]
fn a_preset_path_is_not_mistaken_for_a_sample() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Presets.bwproject");
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" Name="Test">
  <Tracks>
    <Track Type="Audio" Index="0">
      <Device Preset="/Users/music/Library/Presets/EQ.preset" />
      <Device Preset="/Users/music/Library/Presets/MyPreset.txt" />
    </Track>
  </Tracks>
</Project>"#;
    write_bwproject(&proj, Some(xml), &[]);

    let report = audit_project(&proj).unwrap();
    assert!(
        report.missing.is_empty(),
        "a preset is not a missing sample: {:?}",
        report.missing
    );
    assert!(report.total() == 0, "no sample references at all");
}

#[test]
fn collect_keeps_the_bundle_self_contained() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Bundle.bwproject");
    // The reference is relative and already points inside the bundle.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" Name="Test">
  <Tracks>
    <Track Type="Audio" Index="0">
      <AudioClip File="samples/kick.wav" />
    </Track>
  </Tracks>
</Project>"#;
    write_bwproject(
        &proj,
        Some(xml),
        &[("samples/kick.wav", b"RIFF----WAVEfake")],
    );

    let report = audit_project(&proj).unwrap();
    assert_eq!(report.embedded.len(), 1, "the sample is inside the bundle");

    collect_samples(&proj, &report).unwrap();

    // The reference already points inside the bundle, so the bundle is
    // complete and nothing has to be written next to the project.
    assert!(
        !tmp.path().join("samples").join("kick.wav").exists(),
        "nothing should be written next to the project"
    );

    // The reference still points at a file the bundle carries.
    let entries = clavus_core::bitwig_archive::list_entries(&proj).unwrap();
    let xml_name = clavus_core::bitwig_archive::find_project_xml(&entries).unwrap();
    let rewritten = String::from_utf8(
        clavus_core::bitwig_archive::read_entry(&proj, &xml_name).unwrap(),
    )
    .unwrap();
    assert!(
        rewritten.contains(r#"File="samples/kick.wav""#),
        "the reference must still resolve inside the bundle: {}",
        rewritten
    );
    assert!(
        entries.contains(&"samples/kick.wav".to_string()),
        "the bundle must still carry the sample"
    );
}

#[test]
fn collect_rewrites_an_absolute_reference_to_the_place_inside_the_bundle() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Bundle.bwproject");
    write_bwproject(
        &proj,
        Some(GOOD_XML),
        &[("samples/kick.wav", b"RIFF----WAVEfake")],
    );

    let report = audit_project(&proj).unwrap();
    assert_eq!(report.embedded.len(), 1);

    collect_samples(&proj, &report).unwrap();

    let entries = clavus_core::bitwig_archive::list_entries(&proj).unwrap();
    let xml_name = clavus_core::bitwig_archive::find_project_xml(&entries).unwrap();
    let rewritten = String::from_utf8(
        clavus_core::bitwig_archive::read_entry(&proj, &xml_name).unwrap(),
    )
    .unwrap();
    assert!(
        rewritten.contains(r#"File="samples/kick.wav""#),
        "the reference must point inside the bundle: {}",
        rewritten
    );
    assert!(
        !rewritten.contains("/Users/music/Library/Samples/kick.wav"),
        "the stale absolute path must be gone: {}",
        rewritten
    );
}

// ─── Sample resolution ───────────────────────────────────────────────────

/// Write a Live set that references `Samples/loop.wav`.
fn write_als(path: &Path, reference: &str) {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton><LiveSet><Tracks>
  <Track Type="Audio" Index="0">
    <Name Value="A" />
    <SampleRef><Path Value="{}" /></SampleRef>
  </Track>
</Tracks></LiveSet></Ableton>"#,
        reference
    );
    std::fs::write(path, xml).unwrap();
}

#[test]
fn a_sample_that_is_not_where_the_set_says_is_missing() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Set.als");
    write_als(&proj, "Samples/loop.wav");

    // A file with the same name exists elsewhere in the project folder —
    // but the set does not point at it.
    let other = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("loop.wav"), b"RIFF----WAVEfake").unwrap();

    let report = audit_project(&proj).unwrap();
    assert_eq!(
        report.missing.len(),
        1,
        "a file the set does not reference must not count as present: {:?}",
        report.collected
    );
}

#[test]
fn a_sample_reachable_with_different_case_is_found() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Set.als");
    write_als(&proj, "Samples/Loop.wav");

    let samples = tmp.path().join("Samples");
    std::fs::create_dir_all(&samples).unwrap();
    std::fs::write(samples.join("loop.wav"), b"RIFF----WAVEfake").unwrap();

    let report = audit_project(&proj).unwrap();
    assert!(
        report.missing.is_empty(),
        "a case difference must not hide a sample: {:?}",
        report.missing
    );
    assert_eq!(report.collected.len(), 1);
}

// ─── Collect collisions ──────────────────────────────────────────────────

#[test]
fn two_samples_with_the_same_name_are_not_merged() {
    let tmp = TempDir::new().unwrap();
    // The project lives in its own folder, so the two sources are outside it.
    let project_dir = tmp.path().join("Project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let proj = project_dir.join("Set.als");

    // Two different files, both called loop.wav, in different folders.
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("loop.wav"), b"RIFF----WAVEaaa").unwrap();
    std::fs::write(b.join("loop.wav"), b"RIFF----WAVEbbb").unwrap();

    // The set references both, by their absolute path.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton><LiveSet><Tracks>
  <Track Type="Audio" Index="0">
    <SampleRef><Path Value="{}/loop.wav" /></SampleRef>
  </Track>
  <Track Type="Audio" Index="1">
    <SampleRef><Path Value="{}/loop.wav" /></SampleRef>
  </Track>
</Tracks></LiveSet></Ableton>"#,
        a.display(),
        b.display()
    );
    std::fs::write(&proj, xml).unwrap();

    let report = audit_project(&proj).unwrap();
    assert_eq!(report.outside_project.len(), 2);

    let collect = collect_samples(&proj, &report).unwrap();
    assert!(collect.failed.is_empty(), "{:?}", collect.failed);
    assert_eq!(
        collect.copied.len(),
        2,
        "both samples must be collected: {:?}",
        collect.copied
    );

    // Two distinct files, with two distinct contents.
    let mut contents: Vec<Vec<u8>> = collect
        .copied
        .iter()
        .map(|p| std::fs::read(p).unwrap())
        .collect();
    contents.sort();
    assert_eq!(contents.len(), 2, "the two samples must not be merged");
    assert_ne!(contents[0], contents[1], "one track must not play the other");
}

#[test]
fn a_failed_collect_leaves_no_empty_samples_folder() {
    let tmp = TempDir::new().unwrap();
    let proj = tmp.path().join("Set.als");
    write_als(&proj, "Samples/gone.wav");

    let report = audit_project(&proj).unwrap();
    assert_eq!(report.missing.len(), 1);

    let collect = collect_samples(&proj, &report).unwrap();
    assert_eq!(collect.failed.len(), 1);
    assert!(
        !tmp.path().join("Samples").exists(),
        "a failed collect must not leave an empty folder behind"
    );
}
