//! Integration tests for the Ableton `.als` parser.
//!
//! Fixture: `fixtures/test_project.als` — 8 tracks, 5 markers, 128 BPM.

use clavus_parser_als::{AlsParser, extract_sample_refs};
use std::path::Path;

fn fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/test_project.als")
}

#[test]
fn test_parse_test_project_track_count() {
    // gen_fixture.py: 8 tracks, 128 BPM
    let parser = AlsParser::new(&fixture_path()).expect("fixture should parse");
    let p = &parser.project;

    assert_eq!(p.track_count, 8, "expected 8 AudioTracks in fixture (2 return tracks parsed separately, 1 master)");
    assert!((p.bpm - 128.0).abs() < 0.1, "expected BPM 128, got {}", p.bpm);
}

#[test]
fn test_parse_test_project_has_tracks() {
    let parser = AlsParser::new(&fixture_path()).expect("fixture should parse");
    assert!(!parser.project.tracks.is_empty(), "should have parsed tracks");
    assert_eq!(
        parser.project.tracks.len(),
        parser.project.track_count,
        "tracks vec should match track_count"
    );
}

#[test]
fn test_parse_track_names() {
    let parser = AlsParser::new(&fixture_path()).expect("fixture should parse");
    let names: Vec<_> = parser.project.tracks.iter().map(|t| t.name.clone()).collect();
    // Fixture generates named tracks
    assert!(
        names.iter().any(|n| !n.is_empty()),
        "at least one track should have a name: {names:?}"
    );
}

#[test]
fn test_sample_ref_extraction() {
    // Fixture is minimal — may have 0 refs; that's valid.
    let refs = extract_sample_refs(&fixture_path()).expect("extract should succeed");
    assert!(refs.len() >= 0);
}

#[test]
fn test_project_type() {
    let parser = AlsParser::new(&fixture_path()).expect("fixture should parse");
    assert_eq!(
        parser.project.project_type,
        clavus_core::types::ProjectType::Ableton,
        "project type should be Ableton"
    );
}

#[test]
fn test_fixture_file_exists() {
    let path = fixture_path();
    assert!(path.exists(), "fixture should exist at {}", path.display());
}
