//! Regression tests for `collect` — the command that puts a project's
//! samples where the project can reach them.
//!
//! Each test here reproduces a defect an AI beta agent found in the
//! compiled binary: a bundle that lost its audio, a report that was
//! written in the wrong place, a project that stopped being a symlink.

mod common;

use std::fs;
use std::path::Path;

use clavus_cli::{cmd_audit, cmd_collect};
use common::{run, write_als};
use tempfile::TempDir;

/// A tiny valid mono 16-bit WAV.
fn wav_bytes(freq: u32) -> Vec<u8> {
    let rate = 44100u32;
    let n = (rate / 10) as usize;
    let mut frames = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = ((i % 100) as f32 / 100.0 - 0.5) * 10000.0;
        frames.extend_from_slice(&(v as i16).to_le_bytes());
    }
    let data_len = frames.len() as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&frames);
    let _ = freq;
    out
}

/// A `.bwproject` bundle whose `project.xml` points at a sample that
/// lives *outside* the bundle.
fn bundle_pointing_outside(path: &Path, sample: &Path) {
    let sample = sample.to_string_lossy().to_string();
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" VersionMinor="1" Name="Outsider">
  <Arrangement><Timeline><Locators>
    <Locator Position="1.1.1" Name="Intro" />
  </Locators></Timeline></Arrangement>
  <Tracks>
    <Track Type="Audio" Index="0">
      <Name Value="Drums" />
      <DeviceChain><Device Name="EQ Eight" /></DeviceChain>
      <AudioClip File="{sample}" />
    </Track>
  </Tracks>
  <Tempo Bpm="128.0"/>
  <AudioFiles>
    <File Path="{sample}" />
  </AudioFiles>
</Project>"#
    );
    let file = fs::File::create(path).expect("create bundle");
    let mut zf = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    zf.start_file("project.xml", options).expect("project.xml");
    use std::io::Write;
    zf.write_all(xml.as_bytes()).expect("write project.xml");
    zf.finish().expect("finish bundle");
}

/// A `.bwproject` bundle that already carries its samples.
fn bundle_with_samples_inside(path: &Path) {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" VersionMinor="1" Name="Gamma">
  <Tracks>
    <Track Type="Audio" Index="0">
      <Name Value="Drums" />
      <DeviceChain><Device Name="EQ Eight" /></DeviceChain>
      <AudioClip File="samples/kick.wav" />
    </Track>
  </Tracks>
  <Tempo Bpm="128.0"/>
  <AudioFiles>
    <File Path="samples/kick.wav" />
  </AudioFiles>
</Project>"#;
    let file = fs::File::create(path).expect("create bundle");
    let mut zf = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    zf.start_file("project.xml", options).expect("project.xml");
    use std::io::Write;
    zf.write_all(xml.as_bytes()).expect("write project.xml");
    zf.start_file("samples/kick.wav", options)
        .expect("sample");
    zf.write_all(&wav_bytes(440)).expect("write sample");
    zf.finish().expect("finish bundle");
}

/// The entries of a bundle.
fn bundle_entries(path: &Path) -> Vec<String> {
    let file = fs::File::open(path).expect("open bundle");
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    (0..archive.len())
        .map(|i| archive.by_index(i).expect("entry").name().to_string())
        .collect()
}

/// The `project.xml` of a bundle.
fn bundle_xml(path: &Path) -> String {
    let file = fs::File::open(path).expect("open bundle");
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    let mut out = String::new();
    use std::io::Read;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).expect("entry");
        if entry.name() == "project.xml" {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).expect("read");
            out = String::from_utf8_lossy(&buf).to_string();
        }
    }
    out
}

// ─── B2: collect keeps a bundle self-contained ───────────────────────────
//
// Regression: a sample that lived outside the bundle was copied next to
// the bundle, and the reference inside the bundle was pointed at the
// copy it did not carry. The project then opened without its audio, and
// the audit called it healthy.

#[test]
fn collect_keeps_a_bundle_self_contained() {
    let tmp = TempDir::new().expect("temp dir");
    // The sample lives in another tree altogether: the folder that
    // holds the bundle is the project folder, and a sample under it
    // would already be "inside the project".
    let outside = tmp.path().join("elsewhere").join("Library").join("Samples");
    fs::create_dir_all(&outside).expect("samples dir");
    let sample = outside.join("kick.wav");
    fs::write(&sample, wav_bytes(440)).expect("write sample");

    let project_dir = tmp.path().join("notes");
    fs::create_dir_all(&project_dir).expect("project dir");
    let bundle = project_dir.join("Outsider.bwproject");
    bundle_pointing_outside(&bundle, &sample);

    let (out, result) = run(|o, e| cmd_collect(&bundle, &tmp.path().join(".clavus"), o, e));
    assert!(result.is_ok(), "collect: {}", out.all());

    // The sample must be inside the bundle.
    let entries = bundle_entries(&bundle);
    assert!(
        entries.iter().any(|e| e == "samples/kick.wav"),
        "the sample must be inside the bundle: {:?}",
        entries
    );
    // And no copy may be left next to the bundle.
    assert!(
        !project_dir.join("Samples").exists(),
        "no copy may be written next to the bundle"
    );

    // The reference inside the bundle must point at the copy it carries.
    let xml = bundle_xml(&bundle);
    assert!(
        xml.contains("samples/kick.wav"),
        "the reference must point inside the bundle: {}",
        xml
    );
    assert!(
        !xml.contains(&sample.to_string_lossy().to_string()),
        "the reference must not point outside the bundle: {}",
        xml
    );
}

#[test]
fn a_collected_bundle_audits_clean() {
    let tmp = TempDir::new().expect("temp dir");
    let outside = tmp.path().join("elsewhere").join("Library").join("Samples");
    fs::create_dir_all(&outside).expect("samples dir");
    let sample = outside.join("kick.wav");
    fs::write(&sample, wav_bytes(440)).expect("write sample");

    let project_dir = tmp.path().join("notes");
    fs::create_dir_all(&project_dir).expect("project dir");
    let bundle = project_dir.join("Outsider.bwproject");
    bundle_pointing_outside(&bundle, &sample);

    let (out, result) = run(|o, e| cmd_collect(&bundle, &tmp.path().join(".clavus"), o, e));
    assert!(result.is_ok(), "collect: {}", out.all());

    let (out, result) = run(|o, e| cmd_audit(&bundle, false, &tmp.path().join(".clavus"), o, e));
    assert!(result.is_ok(), "audit: {}", out.all());
    assert!(
        !out.stdout.contains("missing"),
        "the collected bundle must be healthy: {}",
        out.all()
    );
}

// ─── B4: collect must not claim a copy it did not make ───────────────────

#[test]
fn collect_says_what_it_actually_did_with_a_bundle() {
    let tmp = TempDir::new().expect("temp dir");
    let bundle = tmp.path().join("Gamma.bwproject");
    bundle_with_samples_inside(&bundle);

    let (out, result) = run(|o, e| cmd_collect(&bundle, &tmp.path().join(".clavus"), o, e));
    assert!(result.is_ok(), "collect: {}", out.all());
    assert!(
        !out.stdout.contains("into project/Samples/"),
        "a bundle is not a folder: {}",
        out.all()
    );
    assert!(
        out.stdout.contains("already inside the project bundle"),
        "the message must say the samples were already inside: {}",
        out.all()
    );
    assert!(
        !tmp.path().join("Samples").exists(),
        "no folder may be created for a bundle"
    );
}

// ─── A6: collect must not destroy a symlinked project ────────────────────
//
// Regression: the rewrite wrote a temporary file and renamed it over
// the project, which replaced a symlink with a regular file. The
// user's project became a copy, and the target kept the old sample
// paths.

#[cfg(unix)]
#[test]
fn collect_writes_through_a_symlinked_project() {
    let tmp = TempDir::new().expect("temp dir");
    let outside = tmp.path().join("Library").join("Samples");
    fs::create_dir_all(&outside).expect("samples dir");
    let sample = outside.join("kick.wav");
    fs::write(&sample, wav_bytes(440)).expect("write sample");

    // The real project, somewhere else.
    let real_dir = tmp.path().join("real");
    fs::create_dir_all(&real_dir).expect("real dir");
    let real = real_dir.join("Eps.als");
    write_als(
        &real,
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="Vox" />
    <DeviceChain />
    <SampleRef><Path Value="{}" /></SampleRef>
  </AudioTrack>
</LiveSet>"#,
            sample.to_string_lossy()
        ),
    );

    // A link to it, where the user works.
    let link_dir = tmp.path().join("notes");
    fs::create_dir_all(&link_dir).expect("notes dir");
    let link = link_dir.join("Link.als");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    let (out, result) = run(|o, e| cmd_collect(&link, &tmp.path().join(".clavus"), o, e));
    assert!(result.is_ok(), "collect: {}", out.all());

    // The link must still be a link.
    let meta = fs::symlink_metadata(&link).expect("the link is there");
    assert!(
        meta.file_type().is_symlink(),
        "the symlink must survive the collect"
    );
    // And the file it points at must carry the new path.
    let target = fs::read_link(&link).expect("the link still points somewhere");
    let target = if target.is_absolute() {
        target
    } else {
        link_dir.join(target)
    };
    let raw = fs::read(&target).expect("the target is readable");
    let (xml, _) = decompress(&raw);
    assert!(
        xml.contains("Samples/kick.wav"),
        "the target must carry the new path: {}",
        xml
    );
}

/// The XML inside a gzipped (or plain) `.als`.
fn decompress(raw: &[u8]) -> (String, bool) {
    use flate2::bufread::GzDecoder;
    use std::io::BufReader;
    use std::io::Read;
    let mut out = Vec::new();
    let mut decoder = GzDecoder::new(BufReader::new(raw));
    if decoder.read_to_end(&mut out).is_ok() && !out.is_empty() {
        return (String::from_utf8_lossy(&out).to_string(), true);
    }
    (String::from_utf8_lossy(raw).to_string(), false)
}

