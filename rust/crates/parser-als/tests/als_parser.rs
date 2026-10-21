//! Integration tests for the Ableton `.als` parser.
//!
//! Fixture: `fixtures/test_project.als` — 8 tracks, 5 markers, 128 BPM.

use clavus_parser_als::{AlsParser, extract_sample_refs};
use std::path::Path;

fn fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/test_project.als")
}

// ─── Fixture tests ──────────────────────────────────────────────────────────────

#[test]
fn test_parse_test_project_track_count() {
    let parser = AlsParser::new(&fixture_path()).expect("fixture should parse");
    let p = &parser.project;
    assert_eq!(p.track_count, 8, "expected 8 AudioTracks in fixture");
    assert!((p.bpm - 128.0).abs() < 0.1, "expected BPM 128");
}

#[test]
fn test_parse_test_project_has_tracks() {
    let parser = AlsParser::new(&fixture_path()).expect("fixture should parse");
    assert!(!parser.project.tracks.is_empty(), "should have parsed tracks");
    assert_eq!(parser.project.tracks.len(), parser.project.track_count);
}

#[test]
fn test_parse_track_names() {
    let parser = AlsParser::new(&fixture_path()).expect("fixture should parse");
    let names: Vec<_> = parser.project.tracks.iter().map(|t| t.name.clone()).collect();
    assert!(names.iter().any(|n| !n.is_empty()), "at least one track should have a name: {names:?}");
}

#[test]
fn test_sample_ref_extraction() {
    let _refs = extract_sample_refs(&fixture_path()).expect("extract should succeed");
}

#[test]
fn test_project_type() {
    let parser = AlsParser::new(&fixture_path()).expect("fixture should parse");
    assert_eq!(parser.project.project_type, clavus_core::types::ProjectType::Ableton);
}

#[test]
fn test_fixture_file_exists() {
    assert!(fixture_path().exists(), "fixture should exist");
}

// ─── Fuzz: malformed inputs should not panic ────────────────────────────────

fn gzip_encode(xml: &[u8]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut enc, xml).ok();
    enc.finish().unwrap_or_default()
}

fn parse_robust(xml: &str) -> bool {
    let compressed = gzip_encode(xml.as_bytes());
    // Unique temp file per call to avoid race conditions in parallel tests
    let tmp = std::env::temp_dir().join(format!("clavus_fuzz_{}.als", uuid_simple()));
    if std::fs::write(&tmp, &compressed).is_err() {
        return false;
    }
    let result = AlsParser::new(&tmp);
    let _ = std::fs::remove_file(&tmp);
    result.is_ok()
}

/// Parse an XML string and return the resulting Project.
fn parse_project(xml: &str) -> clavus_core::types::Project {
    let compressed = gzip_encode(xml.as_bytes());
    let tmp = std::env::temp_dir().join(format!("clavus_proj_{}.als", uuid_simple()));
    std::fs::write(&tmp, &compressed).expect("write temp als");
    let parser = AlsParser::new(&tmp).expect("should parse");
    let _ = std::fs::remove_file(&tmp);
    parser.project
}

/// Simple counter for unique temp file names (safe: tests run single-threaded)
fn uuid_simple() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    format!("f{}", CTR.fetch_add(1, Ordering::Relaxed))
}

#[test]
fn test_fuzz_empty_xml() {
    // Empty input may or may not parse — just ensure no panic
    let result = std::panic::catch_unwind(|| parse_robust(""));
    assert!(result.is_ok(), "empty input should not panic");
}

#[test]
fn test_fuzz_only_whitespace() {
    // Whitespace-only may parse as empty project — just ensure no panic
    let result = std::panic::catch_unwind(|| parse_robust("   \n\t  "));
    assert!(result.is_ok(), "whitespace-only should not panic");
}

#[test]
fn test_fuzz_random_binary() {
    let xml = "<?xml version=\"1.0\"?><LiveSet></LiveSet>";
    let compressed = gzip_encode(xml.as_bytes());
    // Flip random bits
    for (i, byte) in compressed.iter().enumerate().take(100) {
        let mut mutated = compressed.clone();
        mutated[i] = byte.wrapping_add(1);
        let tmp = std::env::temp_dir().join(format!("clavus_fuzz_{}.als", i));
        if std::fs::write(&tmp, &mutated).is_err() { continue; }
        let result = AlsParser::new(&tmp);
        let _ = std::fs::remove_file(&tmp);
        // Should not panic — just return Err
        assert!(
            result.is_ok() || result.is_err(),
            "parser should never panic on corrupted data"
        );
    }
}

#[test]
fn test_fuzz_xml_injection_attacks() {
    // XML injection attempts — should not panic (may return Err)
    let attacks = [
        "<?xml version=\"1.0\"?><LiveSet><!-- <script>evil</script> --></LiveSet>",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><LiveSet>&lt;script&gt;evil&lt;/script&gt;</LiveSet>",
        "<?xml version=\"1.0\"?><LiveSet xmlns:xlink=\"http://evil.com\"></LiveSet>",
    ];
    for attack in attacks {
        let result = std::panic::catch_unwind(|| parse_robust(attack));
        assert!(result.is_ok(), "injection case should not panic: {}", &attack[..attack.len().min(40)]);
    }
}

#[test]
fn test_fuzz_deeply_nested_xml() {
    // Moderate nesting should not panic
    let depth = 200;
    let xml = format!(
        "<?xml version=\"1.0\"?><LiveSet><Tempo Value=\"120\" /><MasterTrack><Name Value=\"Master\" /><DeviceChain /></MasterTrack>{}</LiveSet>",
        "<AudioTrack><Name Value=\"T\" /><DeviceChain /></AudioTrack>".repeat(depth)
    );
    let result = std::panic::catch_unwind(|| parse_robust(&xml));
    assert!(result.is_ok(), "{} levels of nesting should not panic", depth);
}

#[test]
fn test_fuzz_very_long_attribute_value() {
    // 10MB attribute value — should not cause OOM
    let long_val = "x".repeat(10 * 1024 * 1024);
    let xml = format!(
        "<?xml version=\"1.0\"?><LiveSet><AudioTrack><Name Value=\"{}\" /></AudioTrack></LiveSet>",
        long_val
    );
    assert!(parse_robust(&xml) || !parse_robust(&xml), "long attribute should not panic");
}

#[test]
fn test_fuzz_very_long_element_content() {
    let long_content = "x".repeat(1 * 1024 * 1024);
    let xml = format!(
        "<?xml version=\"1.0\"?><LiveSet><AudioTrack><Name Value=\"Test\" />{}</AudioTrack></LiveSet>",
        long_content
    );
    assert!(parse_robust(&xml) || !parse_robust(&xml), "long element should not panic");
}

// ─── Property tests ─────────────────────────────────────────────────────────────

#[test]
fn test_parse_valid_minimal_als() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<LiveSet>
  <Tempo Value="120" />
  <MasterTrack>
    <Name Value="Master" />
    <DeviceChain />
  </MasterTrack>
  <AudioTrack>
    <Name Value="Test" />
    <DeviceChain />
  </AudioTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "minimal valid XML should parse");
}

#[test]
fn test_parse_returns_err_for_nonexistent_file() {
    let result = AlsParser::new(Path::new("/nonexistent/path/to/project.als"));
    assert!(result.is_err(), "nonexistent file should return Err, not panic");
}

#[test]
fn test_parse_returns_err_for_plain_file_without_gzip() {
    let tmp = std::env::temp_dir().join("clavus_not_gzip.als");
    std::fs::write(&tmp, b"this is not a gzip file").ok();
    let result = AlsParser::new(&tmp);
    let _ = std::fs::remove_file(&tmp);
    assert!(result.is_err(), "non-gzip file should return Err");
}

#[test]
fn test_parse_ignores_unknown_tags() {
    // Unknown tags should be silently skipped
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="140" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="Track" />
    <CompletelyUnknownTag Value="foo" />
    <DeviceChain />
  </AudioTrack>
  <RandomGarbageTag><Nested>content</Nested></RandomGarbageTag>
</LiveSet>"#;
    let result = std::panic::catch_unwind(|| parse_robust(xml));
    assert!(result.is_ok(), "unknown tags should not panic");
}

#[test]
fn test_tracks_have_defaults_when_not_specified() {
    // When track has no explicit Color, Mute, Solo — should use defaults
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="100" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="Minimal" />
    <DeviceChain />
  </AudioTrack>
</LiveSet>"#;
    if parse_robust(xml) {
        // Parser should handle missing optional fields gracefully
    }
}

#[test]
fn test_empty_device_chain_parses() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="Empty" />
    <DeviceChain></DeviceChain>
  </AudioTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "empty DeviceChain should parse");
}

#[test]
fn test_device_chain_with_multiple_devices() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="128" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="WithDevices" />
    <DeviceChain>
      <Compressor><Name Value="Comp" /></Compressor>
      <Eq8><Name Value="EQ" /></Eq8>
      <Reverb><Name Value="Hall" /></Reverb>
    </DeviceChain>
  </AudioTrack>
</LiveSet>"#;
    if parse_robust(xml) {
        // Should parse with multiple devices
    }
}

#[test]
fn test_multiple_audio_tracks() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack><Name Value="A1" /><DeviceChain /></AudioTrack>
  <AudioTrack><Name Value="A2" /><DeviceChain /></AudioTrack>
  <AudioTrack><Name Value="A3" /><DeviceChain /></AudioTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "multiple tracks should parse");
}

#[test]
fn test_return_tracks_parsed() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <ReturnTrack><Name Value="Reverb" /></ReturnTrack>
  <ReturnTrack><Name Value="Delay" /></ReturnTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "return tracks should parse");
}

#[test]
fn test_sample_ref_format_1_path() {
    // Live 12: <SampleRef><Path Value="..."/></SampleRef>
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="Sample" />
    <DeviceChain>
      <SampleRefs>
        <SampleRef><Path Value="Samples/kick.wav"/></SampleRef>
      </SampleRefs>
    </DeviceChain>
  </AudioTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "Live 12 sample ref format should parse");
}

#[test]
fn test_sample_ref_format_2_relative_path_elements() {
    // Live 10/11: <SampleRef><RelativePathElement Dir="..."/><Name Value="..."/></SampleRef>
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="Sample" />
    <DeviceChain>
      <SampleRefs>
        <SampleRef>
          <RelativePathElement Dir="Samples" />
          <Name Value="kick.wav" />
        </SampleRef>
      </SampleRefs>
    </DeviceChain>
  </AudioTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "Live 10/11 sample ref format should parse");
}

#[test]
fn test_bpm_from_tempo_envelope() {
    // BPM should come from <Tempo Value="..."/> at top level
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="96" />
  <MasterTrack>
    <Name Value="Master" />
    <DeviceChain>
      <TempoEnvelope><Events><AudioPoint Time="0.0" Value="96" /></Events></TempoEnvelope>
    </DeviceChain>
  </MasterTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "BPM should be parsed correctly");
}

// ─── Clip tests ─────────────────────────────────────────────────────────────────

#[test]
fn test_audio_clip_parsed() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <AudioTrack>
    <Name Value="ClipTrack" />
    <DeviceChain />
    <AudioClip>
      <Name Value="Loop 1" />
      <StartTime Value="0.0" />
      <EndTime Value="4.0" />
      <Color Value="16711680" />
    </AudioClip>
  </AudioTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "AudioClip should parse");
}

#[test]
fn test_midi_clip_parsed() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo Value="120" />
  <MasterTrack><Name Value="Master" /><DeviceChain /></MasterTrack>
  <MidiTrack>
    <Name Value="Midi" />
    <DeviceChain />
    <MidiClip>
      <Name Value="MIDI Clip" />
      <StartTime Value="0.0" />
      <EndTime Value="8.0" />
    </MidiClip>
  </MidiTrack>
</LiveSet>"#;
    assert!(parse_robust(xml), "MidiClip should parse");
}

// ─── Real Ableton Live format ────────────────────────────────────────────────
//
// Regression tests for bugs found by parsing a file that mirrors what
// Ableton Live 11/12 actually writes on disk. The legacy fixture above uses
// <AudioTrack>, which real Live never emits — these tests cover the real shape.
//
// Fixture: `fixtures/real_live_project.als` — 3 tracks, 2 returns, 3 markers, 128 BPM.

fn real_live_fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/real_live_project.als")
}

#[test]
fn test_real_live_track_count() {
    // Real Live writes <Track> inside <Tracks>, not <AudioTrack>.
    let parser = AlsParser::new(&real_live_fixture_path()).expect("fixture should parse");
    assert_eq!(parser.project.track_count, 3, "expected 3 <Track> elements");
}

#[test]
fn test_real_live_track_names() {
    // <Name Value="Drums"/> is self-closing, so it arrives as an Empty event.
    let parser = AlsParser::new(&real_live_fixture_path()).expect("fixture should parse");
    let names: Vec<&str> = parser
        .project
        .tracks
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, vec!["Drums", "Vocal", "Audio 1"]);
}

#[test]
fn test_real_live_track_type_inferred_from_chain() {
    // <MidiEffectChain> => MIDI, <AudioEffectChain> => Audio.
    let parser = AlsParser::new(&real_live_fixture_path()).expect("fixture should parse");
    assert_eq!(parser.project.tracks[0].track_type, "MIDI");
    assert_eq!(parser.project.tracks[1].track_type, "Audio");
    assert_eq!(parser.project.tracks[2].track_type, "Audio");
}

#[test]
fn test_real_live_mute_state() {
    let parser = AlsParser::new(&real_live_fixture_path()).expect("fixture should parse");
    assert!(parser.project.tracks[0].is_muted, "Drums should be muted");
    assert!(!parser.project.tracks[1].is_muted, "Vocal should not be muted");
}

#[test]
fn test_real_live_nested_track_in_mixer_does_not_end_parse() {
    // Every real track contains <Mixer><Track>...</Track></Mixer> — a nested
    // element with the SAME name. It must not terminate the track early.
    let parser = AlsParser::new(&real_live_fixture_path()).expect("fixture should parse");
    assert_eq!(parser.project.tracks.len(), 3, "nested <Track> in <Mixer> broke parsing");
}

#[test]
fn test_real_live_return_tracks() {
    let parser = AlsParser::new(&real_live_fixture_path()).expect("fixture should parse");
    assert_eq!(parser.project.return_tracks.len(), 2);
    let names: Vec<&str> = parser
        .project
        .return_tracks
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, vec!["A Reverb", "A Delay"]);
}

#[test]
fn test_real_live_markers_deduplicated() {
    // Fixture carries the same markers in <CuePoints> and <Locators>.
    let parser = AlsParser::new(&real_live_fixture_path()).expect("fixture should parse");
    assert_eq!(parser.project.markers.len(), 3, "markers should be de-duplicated");
    assert_eq!(parser.project.markers[0].name, "Intro");
    assert_eq!(parser.project.markers[0].time, "1.1.1");
}

#[test]
fn test_real_live_bpm_from_tempo_envelope() {
    // BPM lives in MasterTrack > DeviceChain > TempoEnvelope > Events > AudioPoint.
    let parser = AlsParser::new(&real_live_fixture_path()).expect("fixture should parse");
    assert!(
        (parser.project.bpm - 128.0).abs() < 0.01,
        "expected 128 BPM from tempo envelope, got {}",
        parser.project.bpm
    );
}

#[test]
fn test_tempo_from_nested_value_element() {
    // <Tempo><Value Value="140.0"/></Tempo>
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tempo><Value Value="140.0"/></Tempo>
  <Tracks></Tracks>
</LiveSet>"#;
    let p = parse_project(xml);
    assert!((p.bpm - 140.0).abs() < 0.01, "got {}", p.bpm);
}

#[test]
fn test_send_channels_parsed() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tracks>
    <Track>
      <Name Value="Lead"/>
      <DeviceChain/>
      <SendChannels>
        <SendChannel><Name Value="Reverb"/></SendChannel>
        <SendChannel><Name Value="Delay"/></SendChannel>
      </SendChannels>
    </Track>
  </Tracks>
</LiveSet>"#;
    let p = parse_project(xml);
    assert_eq!(p.tracks.len(), 1);
    assert_eq!(p.tracks[0].sends.len(), 2);
    assert!(p.tracks[0].sends.contains_key("Reverb"));
    assert!(p.tracks[0].sends.contains_key("Delay"));
}

#[test]
fn test_send_name_does_not_overwrite_track_name() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tracks>
    <Track>
      <Name Value="Lead"/>
      <DeviceChain/>
      <SendChannels>
        <SendChannel><Name Value="Reverb"/></SendChannel>
      </SendChannels>
    </Track>
  </Tracks>
</LiveSet>"#;
    let p = parse_project(xml);
    assert_eq!(p.tracks[0].name, "Lead", "send name leaked into track name");
}

// ─── Container format and error handling ────────────────────────────────────

#[test]
fn test_plain_xml_als_is_parsed() {
    // Live 8 and hand-edited files are plain XML, not gzip.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton><LiveSet>
  <Tracks>
    <Track Type="Audio" Index="0"><Name Value="Plain"/></Track>
  </Tracks>
</LiveSet></Ableton>"#;
    let tmp = std::env::temp_dir().join(format!("clavus_plain_{}.als", uuid_simple()));
    std::fs::write(&tmp, xml.as_bytes()).expect("write temp als");
    let parser = AlsParser::new(&tmp);
    let _ = std::fs::remove_file(&tmp);

    let parser = parser.expect("plain XML .als should parse");
    assert_eq!(parser.project.tracks.len(), 1);
    assert_eq!(parser.project.tracks[0].name, "Plain");
}

#[test]
fn test_gzip_and_plain_xml_agree() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tracks>
    <Track Type="Audio"><Name Value="Kick"/></Track>
    <Track Type="Midi"><Name Value="Bass"/></Track>
  </Tracks>
</LiveSet>"#;
    let gz_tmp = std::env::temp_dir().join(format!("clavus_gz_{}.als", uuid_simple()));
    let pl_tmp = std::env::temp_dir().join(format!("clavus_pl_{}.als", uuid_simple()));
    std::fs::write(&gz_tmp, gzip_encode(xml.as_bytes())).unwrap();
    std::fs::write(&pl_tmp, xml.as_bytes()).unwrap();

    let gz = AlsParser::new(&gz_tmp).expect("gzip should parse");
    let pl = AlsParser::new(&pl_tmp).expect("plain should parse");
    let _ = std::fs::remove_file(&gz_tmp);
    let _ = std::fs::remove_file(&pl_tmp);

    assert_eq!(gz.project.tracks.len(), pl.project.tracks.len());
    for (a, b) in gz.project.tracks.iter().zip(pl.project.tracks.iter()) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.track_type, b.track_type);
    }
}

#[test]
fn test_non_ableton_xml_returns_error() {
    // A well-formed XML file that is not an Ableton project.
    let xml = r#"<?xml version="1.0"?><Recipe><Notes>hello</Notes></Recipe>"#;
    let tmp = std::env::temp_dir().join(format!("clavus_notabl_{}.als", uuid_simple()));
    std::fs::write(&tmp, xml.as_bytes()).unwrap();
    let result = AlsParser::new(&tmp);
    let _ = std::fs::remove_file(&tmp);

    let err = result.expect_err("non-Ableton XML should be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("Ableton") || msg.contains("LiveSet"),
        "error should explain the problem, got: {}",
        msg
    );
}

#[test]
fn test_malformed_xml_returns_error() {
    // Root element is present but the document is not well-formed.
    let xml = r#"<?xml version="1.0"?><Ableton><LiveSet><Tracks></LiveSet></Ableton>"#;
    let tmp = std::env::temp_dir().join(format!("clavus_bad_{}.als", uuid_simple()));
    std::fs::write(&tmp, xml.as_bytes()).unwrap();
    let result = AlsParser::new(&tmp);
    let _ = std::fs::remove_file(&tmp);

    let err = result.expect_err("malformed XML should be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("malformed") || msg.contains("XML"),
        "error should mention malformed XML, got: {}",
        msg
    );
}

#[test]
fn test_empty_file_returns_error() {
    let tmp = std::env::temp_dir().join(format!("clavus_empty_{}.als", uuid_simple()));
    std::fs::write(&tmp, b"").unwrap();
    let result = AlsParser::new(&tmp);
    let _ = std::fs::remove_file(&tmp);
    assert!(result.is_err(), "empty file should be rejected");
}

#[test]
fn test_binary_garbage_returns_error() {
    let tmp = std::env::temp_dir().join(format!("clavus_bin_{}.als", uuid_simple()));
    std::fs::write(&tmp, vec![0x00u8, 0xFF, 0xFE, 0x42, 0x00, 0x7F, 0x80]).unwrap();
    let result = AlsParser::new(&tmp);
    let _ = std::fs::remove_file(&tmp);
    assert!(result.is_err(), "binary garbage should be rejected");
}

#[test]
fn test_truncated_gzip_returns_error() {
    let xml = r#"<?xml version="1.0"?><Ableton><LiveSet/></Ableton>"#;
    let mut compressed = gzip_encode(xml.as_bytes());
    compressed.truncate(compressed.len() / 2);
    let tmp = std::env::temp_dir().join(format!("clavus_trunc_{}.als", uuid_simple()));
    std::fs::write(&tmp, &compressed).unwrap();
    let result = AlsParser::new(&tmp);
    let _ = std::fs::remove_file(&tmp);
    assert!(result.is_err(), "truncated gzip should be rejected");
}

#[test]
fn test_nonexistent_file_returns_error() {
    let result = AlsParser::new(Path::new("/nonexistent/path.als"));
    assert!(result.is_err());
}

#[test]
fn test_device_display_name_is_used() {
    // Devices carry their user-assigned name in a nested <Name> element.
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tracks>
    <Track Type="Audio">
      <Name Value="Drums"/>
      <DeviceChain>
        <Devices>
          <Compressor><Name Value="Glue Compressor"/></Compressor>
          <Reverb><Name Value="Hall Reverb"/></Reverb>
        </Devices>
      </DeviceChain>
    </Track>
  </Tracks>
</LiveSet>"#;
    let p = parse_project(xml);
    let names: Vec<&str> = p.tracks[0].devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["Glue Compressor", "Hall Reverb"]);
}

#[test]
fn test_device_without_name_falls_back_to_tag() {
    let xml = r#"<?xml version="1.0"?><LiveSet>
  <Tracks>
    <Track Type="Audio">
      <DeviceChain>
        <Devices><Compressor/></Devices>
      </DeviceChain>
    </Track>
  </Tracks>
</LiveSet>"#;
    let p = parse_project(xml);
    assert_eq!(p.tracks[0].devices.len(), 1);
    assert_eq!(p.tracks[0].devices[0].name, "Compressor");
}
