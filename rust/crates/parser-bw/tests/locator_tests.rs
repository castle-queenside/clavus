//! Regression tests for injecting cues into a Bitwig bundle.
//!
//! The locator name is written by a person, so it can be longer than a
//! locator has room for, it can contain an accent, and it can contain
//! a quote. None of those may panic, and none of them may produce XML
//! that does not parse.

use clavus_core::types::{Cue, CueStatus};
use std::io::{Read, Write};

fn cue(text: &str) -> Cue {
    Cue {
        id: "c0ffee00".into(),
        project: "Test".into(),
        position: "1.1.1".into(),
        text: text.into(),
        author: "chris".into(),
        timestamp: 0.0,
        status: CueStatus::Pending,
        snapshot_hash: String::new(),
        track_name: String::new(),
        replies: Vec::new(),
        assignee: String::new(),
        in_progress: false,
    }
}

/// A `.bwproject` bundle with one `project.xml` in it.
fn bundle(project_xml: &str) -> Vec<u8> {
    let mut out = Vec::new();
    {
        use zip::write::SimpleFileOptions;
        use zip::CompressionMethod;
        use zip::ZipWriter;
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut out));
        let options =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("project.xml", options).expect("zip");
        zip.write_all(project_xml.as_bytes()).expect("zip");
        zip.finish().expect("zip");
    }
    out
}

/// The `project.xml` inside a bundle.
fn read_project_xml(bytes: &[u8]) -> String {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip open");
    let mut found = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).expect("entry");
        if entry.name() == "project.xml" || entry.name().ends_with("/project.xml") {
            let mut text = String::new();
            entry.read_to_string(&mut text).expect("read");
            found = Some(text);
            break;
        }
    }
    found.expect("project.xml should be in the bundle")
}

const PROJECT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<BitwigProject><Locators></Locators></BitwigProject>"#;

#[test]
fn a_cue_with_an_accent_does_not_panic() {
    // The old code cut the text at a *byte* boundary, so a cue with a
    // multi-byte character near the cut cut the character in half and
    // panicked.
    let mut text = "a".repeat(59);
    text.push('é');
    let out = clavus_parser_bw::inject_locators(&bundle(PROJECT_XML), &[cue(&text)])
        .expect("injection should succeed");
    let xml = read_project_xml(&out);
    assert!(xml.contains("Name"), "the locator should be in the bundle: {}", xml);
}

#[test]
fn a_cue_with_a_quote_does_not_break_the_xml() {
    let text = r#"say "hi" & go <now>"#;
    let out = clavus_parser_bw::inject_locators(&bundle(PROJECT_XML), &[cue(text)])
        .expect("injection should succeed");
    let xml = read_project_xml(&out);

    assert!(
        !xml.contains(r#"Name="● say "hi""#),
        "the quote was written raw into the attribute: {}",
        xml
    );
    assert!(xml.contains("&quot;"), "the quote should be escaped: {}", xml);
    assert!(xml.contains("&amp;"), "the ampersand should be escaped: {}", xml);
    assert!(xml.contains("&lt;"), "the angle bracket should be escaped: {}", xml);
    assert!(xml_is_well_formed(&xml), "the bundle should still be valid XML");
}

#[test]
fn a_long_cue_is_cut_at_a_character_boundary() {
    let text = "é".repeat(100);
    let out = clavus_parser_bw::inject_locators(&bundle(PROJECT_XML), &[cue(&text)])
        .expect("injection should succeed");
    let xml = read_project_xml(&out);
    assert_eq!(
        xml.matches('é').count(),
        60,
        "the cue should be cut at 60 characters"
    );
}

/// Check that the injected bundle is still well-formed XML.
///
/// A real parser is used rather than a hand-rolled check: the point
/// of this test is that a bundle with a quote in a cue still opens.
fn xml_is_well_formed(xml: &str) -> bool {
    use quick_xml::events::Event;
    use quick_xml::Reader;
    let mut reader = Reader::from_str(xml);
    // Without this, quick-xml does not notice a tag that closes the
    // wrong one — and the whole point of this check is to notice.
    reader.check_end_names(true);
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

#[test]
fn the_xml_checker_really_notices_broken_xml() {
    assert!(
        !xml_is_well_formed(r#"<a><b></a>"#),
        "a mismatched tag should not pass the check"
    );
    assert!(xml_is_well_formed(PROJECT_XML), "the sample should pass");
}
