//! Integration tests for the Bitwig `.bwproject` parser.
//!
//! No .bwproject fixture yet — these tests document the expected API
//! and provide a smoke test that the crate compiles + is callable.

use clavus_parser_bw::BitwigParser;
use std::path::Path;

fn fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/test_project.bwproject")
}

#[test]
#[ignore = "no .bwproject fixture yet — implement once user provides a fixture"]
fn test_parse_bitwig_fixture() {
    let parser = BitwigParser::new(&fixture_path()).expect("fixture should parse");
    assert!(
        parser.project.track_count >= 0,
        "track_count should be non-negative"
    );
}

#[test]
fn test_parser_module_loads() {
    // Smoke test — verifies the crate compiles and the public API is accessible.
    // BitwigParser::new returns Err for a non-existent file, not a compile error.
    let result = BitwigParser::new(Path::new("/nonexistent/path.bwproject"));
    assert!(
        result.is_err(),
        "BitwigParser::new should fail for nonexistent file, not compile-fail"
    );
}

#[test]
fn test_nonexistent_fixture_returns_error() {
    let result = BitwigParser::new(Path::new("../../../fixtures/nonexistent.bwproject"));
    assert!(result.is_err(), "nonexistent fixture should return an error");
}
