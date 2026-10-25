//! Regression tests for injecting cues into an Ableton set.
//!
//! The marker name is written by a person, so it can be longer than a
//! marker has room for, it can contain an accent, and it can contain a
//! quote. None of those may panic, and none of them may produce XML
//! that does not parse.

use clavus_core::types::{Cue, CueStatus};
use clavus_parser_als::inject_markers;
use std::io::Write;
use std::path::PathBuf;

/// A counter that is unique per process, so two tests writing the
/// same file name cannot collide.
fn unique_path(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "clavus_marker_{}_{}_{}.als",
        tag,
        std::process::id(),
        n
    ))
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).expect("gzip");
    enc.finish().expect("gzip finish")
}

fn gunzip(data: &[u8]) -> String {
    let mut dec = flate2::read::GzDecoder::new(data);
    let mut out = String::new();
    std::io::Read::read_to_string(&mut dec, &mut out).expect("gunzip");
    out
}

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

const LIVE11_SET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton><LiveSet><CuePoints></CuePoints></LiveSet></Ableton>"#;

const LIVE12_SET: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton><LiveSet><Locators><Locators></Locators></Locators></LiveSet></Ableton>"#;

/// Write a set to disk, because the injector makes a backup of it.
fn write_set(path: &PathBuf, xml: &str) -> Vec<u8> {
    let bytes = gzip(xml.as_bytes());
    std::fs::write(path, &bytes).expect("write set");
    bytes
}

#[test]
fn a_cue_with_an_accent_does_not_panic() {
    // The old code cut the text at a *byte* boundary. A cue whose
    // text had a multi-byte character near the cut would cut the
    // character in half and panic.
    let text = "café needs a new sample";
    let path = unique_path("accent");
    write_set(&path, LIVE11_SET);

    let out = inject_markers(&gzip(LIVE11_SET.as_bytes()), &[cue(text)], Some(&path))
        .expect("injection should succeed");
    let xml = gunzip(&out);
    assert!(xml.contains("caf"), "the cue text should be in the set: {}", xml);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("als.bak"));
}

#[test]
fn a_cue_with_a_multi_byte_character_at_the_cut_does_not_panic() {
    // Exactly at the boundary: 59 ASCII characters and then an
    // accent. The old byte cut landed in the middle of the accent.
    let mut text = "a".repeat(59);
    text.push('é');
    let path = unique_path("boundary");
    write_set(&path, LIVE11_SET);

    let out = inject_markers(&gzip(LIVE11_SET.as_bytes()), &[cue(&text)], Some(&path))
        .expect("injection should succeed");
    assert!(gunzip(&out).contains("Name"));

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("als.bak"));
}

#[test]
fn the_xml_checker_really_notices_broken_xml() {
    // A test that says "this is valid XML" is worth nothing unless the
    // checker says no when it should. This is that proof.
    assert!(
        !quick_xml_check(r#"<a><b></a>"#),
        "a mismatched tag should not pass the check"
    );
    assert!(
        !quick_xml_check(r#"<a attr="unclosed></a>"#),
        "an unclosed quote should not pass the check"
    );
    assert!(quick_xml_check(LIVE11_SET), "the sample set should pass");
}

#[test]
fn a_cue_with_a_quote_does_not_break_the_xml() {
    // A quote in the cue text used to be written straight into an
    // XML attribute, which made the set unreadable.
    let text = r#"say "hi" & go <now>"#;
    let path = unique_path("quote");
    write_set(&path, LIVE11_SET);

    let out = inject_markers(&gzip(LIVE11_SET.as_bytes()), &[cue(text)], Some(&path))
        .expect("injection should succeed");
    let xml = gunzip(&out);

    // The quote must not appear raw inside the attribute.
    assert!(
        !xml.contains(r#"Name Value="● say "hi""#),
        "the quote was written raw into the attribute: {}",
        xml
    );
    assert!(xml.contains("&quot;"), "the quote should be escaped: {}", xml);
    assert!(xml.contains("&amp;"), "the ampersand should be escaped: {}", xml);
    assert!(xml.contains("&lt;"), "the angle bracket should be escaped: {}", xml);

    // And the result must still be well-formed XML.
    let parsed = quick_xml_check(&xml);
    assert!(parsed, "the injected set should still parse as XML");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("als.bak"));
}

#[test]
fn a_long_cue_is_cut_at_a_character_boundary() {
    let text = "é".repeat(100);
    let path = unique_path("long");
    write_set(&path, LIVE11_SET);

    let out = inject_markers(&gzip(LIVE11_SET.as_bytes()), &[cue(&text)], Some(&path))
        .expect("injection should succeed");
    let xml = gunzip(&out);

    // 60 characters of the marker text, plus the bullet and the space.
    let bullet_count = xml.matches('é').count();
    assert_eq!(bullet_count, 60, "the cue should be cut at 60 characters");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("als.bak"));
}

#[test]
fn a_live_12_locator_is_escaped_too() {
    let text = r#"quote " and & ampersand"#;
    let path = unique_path("live12");
    write_set(&path, LIVE12_SET);

    let out = inject_markers(&gzip(LIVE12_SET.as_bytes()), &[cue(text)], Some(&path))
        .expect("injection should succeed");
    let xml = gunzip(&out);
    assert!(xml.contains("&quot;"), "the quote should be escaped: {}", xml);
    assert!(xml.contains("&amp;"), "the ampersand should be escaped: {}", xml);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("als.bak"));
}

/// Check that the injected set is still well-formed XML.
///
/// A real parser is used rather than a hand-rolled check: the point
/// of this test is that a set with a quote in a cue still opens.
fn quick_xml_check(xml: &str) -> bool {
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
