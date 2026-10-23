//! Regression tests for what a real Ableton Live library actually looks
//! like.
//!
//! Every test here was written after reading a real set from a real
//! studio drive, because the parser was quietly wrong for every one of
//! them: it reported one track, 120 BPM, no devices, and no track names.
//!
//! The shapes below are the ones Live writes. They differ by version,
//! and the differences matter:
//!
//!   Live 8    `<Name Value="Kick"/>` on the track, `<PlugName>` on the
//!             device, the devices in a nested `<DeviceChain>`.
//!   Live 9    `<Name><EffectiveName/><UserName/></Name>` on the track,
//!             the tempo in the master track's `<Mixer>`.
//!   Live 11   as Live 9, plus `<MainTrack>` in place of `<MasterTrack>`.
//!   Live 12   as Live 11.

use clavus_parser_als::AlsParser;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn gzip_encode(xml: &[u8]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut enc, xml).ok();
    enc.finish().unwrap_or_default()
}

/// A temp file name that no other test can collide with.
fn unique_name(prefix: &str) -> String {
    use std::sync::atomic::AtomicU64 as Ctr;
    static CTR: Ctr = Ctr::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = CTR.fetch_add(1, Ordering::SeqCst);
    format!("clavus_{prefix}_{}_{}_{}.als", std::process::id(), nanos, n)
}

fn parse_xml(xml: &str) -> Result<clavus_core::types::Project, String> {
    let tmp = std::env::temp_dir().join(unique_name("real"));
    std::fs::write(&tmp, gzip_encode(xml.as_bytes())).expect("write temp als");
    let parsed = AlsParser::new(&tmp)
        .map(|p| p.project)
        .map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    parsed
}

fn parse_ok(xml: &str) -> clavus_core::types::Project {
    parse_xml(xml).expect("should parse")
}

// ─── The document that was being swallowed ───────────────────────────────────

/// Live 9 writes a track's device chain with the devices in a chain
/// nested inside the track's own chain, and a great deal of structure
/// around it.
const LIVE_9_TRACK: &str = r#"
    <AudioTrack Id="41">
      <LomId Value="0" />
      <Name>
        <EffectiveName Value="3-Audio" />
        <UserName Value="" />
      </Name>
      <ColorIndex Value="164" />
      <DevicesListWrapper LomId="0" />
      <Mixer>
        <LomId Value="0" />
        <Track>
          <Volume><Value Value="0.8" /></Volume>
        </Track>
        <Tempo>
          <Manual Value="118" />
        </Tempo>
      </Mixer>
      <DeviceChain>
        <AutomationLanes>
          <AutomationLanes>
            <AutomationLane>
              <SelectedDevice Value="1" />
            </AutomationLane>
          </AutomationLanes>
        </AutomationLanes>
        <EnvelopeChooser><SelectedDevice Value="1" /></EnvelopeChooser>
        <AudioInputRouting><Target Value="AudioIn/External/S0" /></AudioInputRouting>
        <Mixer>
          <LomId Value="0" />
          <Track>
            <Volume><Value Value="0.9" /></Volume>
          </Track>
        </Mixer>
        <FreezeSequencer>
          <Recorder><IsArmed Value="false" /></Recorder>
        </FreezeSequencer>
        <DeviceChain>
          <Devices>
            <AutoFilter Id="3">
              <LomId Value="0" />
              <On><Manual Value="true" /></On>
            </AutoFilter>
            <Eq8 Id="4">
              <LomId Value="0" />
              <UserName Value="Air" />
            </Eq8>
          </Devices>
        </DeviceChain>
      </DeviceChain>
    </AudioTrack>"#;

fn live_9_set(tracks: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="5" MinorVersion="30.3" Creator="Live 9">
  <LiveSet>
    <Tracks>
{tracks}
    </Tracks>
    <MasterTrack>
      <LomId Value="0" />
      <Name>
        <EffectiveName Value="Master" />
        <UserName Value="" />
      </Name>
      <Mixer>
        <LomId Value="0" />
        <Track><Volume><Value Value="0.85" /></Volume></Track>
        <Tempo>
          <Manual Value="92" />
        </Tempo>
      </Mixer>
      <DeviceChain>
        <Devices>
          <PluginDevice Id="0">
            <LomId Value="0" />
          </PluginDevice>
        </Devices>
      </DeviceChain>
    </MasterTrack>
  </LiveSet>
</Ableton>"#
    )
}

#[test]
fn every_track_is_parsed_not_just_the_first() {
    let xml = live_9_set(&format!(
        "{}\n{}\n{}",
        LIVE_9_TRACK.replace("3-Audio", "Kick"),
        LIVE_9_TRACK.replace("3-Audio", "Snare"),
        LIVE_9_TRACK.replace("3-Audio", "Hat")
    ));
    let p = parse_ok(&xml);
    assert_eq!(
        p.tracks.len(),
        3,
        "every track in the set should be found, not one track that swallowed the rest"
    );
    let names: Vec<&str> = p.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["Kick", "Snare", "Hat"]);
}

#[test]
fn the_track_type_comes_from_the_element() {
    let xml = live_9_set(&format!(
        "{}\n{}\n{}",
        LIVE_9_TRACK
            .replace("3-Audio", "Kick")
            .replace("AudioTrack", "MidiTrack"),
        LIVE_9_TRACK.replace("3-Audio", "Snare"),
        LIVE_9_TRACK
            .replace("3-Audio", "Bus")
            .replace("AudioTrack", "GroupTrack"),
    ));
    let p = parse_ok(&xml);
    let kinds: Vec<&str> = p.tracks.iter().map(|t| t.track_type.as_str()).collect();
    assert_eq!(kinds, vec!["MIDI", "Audio", "Group"]);
}

#[test]
fn the_project_tempo_comes_from_the_master_track() {
    let xml = live_9_set(LIVE_9_TRACK);
    let p = parse_ok(&xml);
    assert!(
        (p.bpm - 92.0).abs() < 0.1,
        "the master's tempo is the project's tempo, got {}",
        p.bpm
    );
}

#[test]
fn a_track_tempo_does_not_become_the_project_tempo() {
    // The track above carries its own tempo of 118 in its mixer.
    let xml = live_9_set(LIVE_9_TRACK);
    let p = parse_ok(&xml);
    assert!(
        (p.bpm - 118.0).abs() > 0.1,
        "a track's own tempo must not be read as the project's, got {}",
        p.bpm
    );
}

#[test]
fn the_devices_are_the_devices_and_not_the_whole_document() {
    let xml = live_9_set(LIVE_9_TRACK);
    let p = parse_ok(&xml);
    let devices = &p.tracks[0].devices;
    let names: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["AutoFilter", "Air"],
        "the device list should hold the two devices, not every element in the chain"
    );
}

#[test]
fn a_renamed_device_keeps_its_name() {
    let xml = live_9_set(LIVE_9_TRACK);
    let p = parse_ok(&xml);
    assert_eq!(p.tracks[0].devices[1].name, "Air");
    assert_eq!(p.tracks[0].devices[1].device_type, "Eq8");
}

#[test]
fn the_user_name_wins_over_the_effective_name() {
    let xml = live_9_set(&LIVE_9_TRACK.replace(
        "<EffectiveName Value=\"3-Audio\" />\n        <UserName Value=\"\" />",
        "<EffectiveName Value=\"3-Audio\" />\n        <UserName Value=\"Room\" />",
    ));
    let p = parse_ok(&xml);
    assert_eq!(p.tracks[0].name, "Room");
}

#[test]
fn the_track_name_is_read_from_the_name_container() {
    let xml = live_9_set(LIVE_9_TRACK);
    let p = parse_ok(&xml);
    assert_eq!(p.tracks[0].name, "3-Audio");
}

#[test]
fn the_live_version_is_read() {
    let xml = live_9_set(LIVE_9_TRACK);
    let p = parse_ok(&xml);
    assert_eq!(p.ableton_version.as_deref(), Some("5"));
}

// ─── Live 8 shapes ───────────────────────────────────────────────────────────

#[test]
fn live_8_names_the_device_by_its_plug_name() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="4" MinorVersion="9.0_305">
  <LiveSet>
    <Tracks>
      <MidiTrack Id="20">
        <Name Value="Maschine" />
        <DeviceChain>
          <Mixer><Track><Volume><Value Value="0.8" /></Volume></Track></Mixer>
          <DeviceChain>
            <Devices>
              <PluginDevice Id="0">
                <LomId Value="0" />
                <PlugName Value="Maschine" />
                <UserName Value="" />
              </PluginDevice>
            </Devices>
          </DeviceChain>
        </DeviceChain>
      </MidiTrack>
    </Tracks>
    <MasterTrack>
      <Mixer>
        <Tempo>
          <LomId Value="0" />
          <Manual Value="128" />
        </Tempo>
      </Mixer>
    </MasterTrack>
  </LiveSet>
</Ableton>"#;
    let p = parse_ok(xml);
    assert_eq!(p.tracks.len(), 1);
    assert_eq!(p.tracks[0].name, "Maschine");
    assert_eq!(p.tracks[0].devices.len(), 1);
    assert_eq!(p.tracks[0].devices[0].name, "Maschine");
    assert!((p.bpm - 128.0).abs() < 0.1, "got {}", p.bpm);
}

#[test]
fn live_8_tracks_without_a_name_get_lives_numbered_name() {
    // Live 8 often stores no name at all. Live's own interface shows
    // "1-MIDI", so Clavus does the same rather than saying "Unnamed".
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="4" MinorVersion="9.0_305">
  <LiveSet>
    <Tracks>
      <MidiTrack Id="20">
        <Name><UserName Value="" /></Name>
      </MidiTrack>
      <AudioTrack Id="21">
        <Name><UserName Value="" /></Name>
      </AudioTrack>
    </Tracks>
  </LiveSet>
</Ableton>"#;
    let p = parse_ok(xml);
    let names: Vec<&str> = p.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["1-MIDI", "2-Audio"]);
}

#[test]
fn a_midi_keyboard_track_is_named() {
    // Live writes MIDI keyboard tracks as <KeyTrack> and gives them no
    // name at all.
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="5" MinorVersion="30.3">
  <LiveSet>
    <Tracks>
      <KeyTrack Id="7" />
      <AudioTrack Id="8">
        <Name><EffectiveName Value="Audio" /><UserName Value="" /></Name>
      </AudioTrack>
    </Tracks>
  </LiveSet>
</Ableton>"#;
    let p = parse_ok(xml);
    assert_eq!(p.tracks.len(), 2);
    assert_eq!(p.tracks[0].name, "MIDI Keyboard");
    assert_eq!(p.tracks[0].track_type, "MIDI");
}

// ─── Live 12 shapes ──────────────────────────────────────────────────────────

#[test]
fn live_12_uses_main_track_for_the_master() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="8" MinorVersion="44.1">
  <LiveSet>
    <Tracks>
      <AudioTrack Id="3">
        <Name><EffectiveName Value="1-Audio" /><UserName Value="" /></Name>
      </AudioTrack>
    </Tracks>
    <MainTrack>
      <Mixer>
        <Tempo><Manual Value="140" /></Tempo>
      </Mixer>
    </MainTrack>
  </LiveSet>
</Ableton>"#;
    let p = parse_ok(xml);
    assert!(p.master_track.is_some(), "the master track should be found");
    assert!((p.bpm - 140.0).abs() < 0.1, "got {}", p.bpm);
    assert_eq!(p.tracks.len(), 1);
}

// ─── The old binary format ───────────────────────────────────────────────────

#[test]
fn the_old_live_binary_format_is_reported_clearly() {
    // Live wrote some sets in its own compressed format before gzip.
    // It is a binary serialisation, not XML, so it cannot be read — but
    // the reason should be said plainly rather than reported as a
    // decoding failure.
    let tmp = std::env::temp_dir().join(unique_name("old"));
    let mut raw = vec![0xabu8, 0x1eu8, b'V', b'x', 0x03, 0x7c];
    raw.extend_from_slice(&[0u8; 64]);
    std::fs::write(&tmp, &raw).expect("write temp als");
    let err = AlsParser::new(&tmp)
        .expect_err("should not parse")
        .to_string();
    let _ = std::fs::remove_file(&tmp);
    assert!(
        err.contains("old compressed format"),
        "the error should name the problem, got: {err}"
    );
}

// ─── The tempo envelope ──────────────────────────────────────────────────────

#[test]
fn the_master_tempo_envelope_is_read() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="5" MinorVersion="30.3">
  <LiveSet>
    <Tracks>
      <AudioTrack Id="1">
        <Name><EffectiveName Value="Audio" /><UserName Value="" /></Name>
      </AudioTrack>
    </Tracks>
    <MasterTrack>
      <DeviceChain>
        <TempoEnvelope>
          <Events>
            <AudioPoint Time="0.0" Value="115.0" />
          </Events>
        </TempoEnvelope>
      </DeviceChain>
    </MasterTrack>
  </LiveSet>
</Ableton>"#;
    let p = parse_ok(xml);
    assert!(
        (p.bpm - 115.0).abs() < 0.1,
        "the tempo envelope's first point is the project's tempo, got {}",
        p.bpm
    );
}

// ─── The document must not be swallowed ─────────────────────────────────────

#[test]
fn a_track_after_a_device_chain_is_still_found() {
    // The bug: skipping a device chain used to consume the rest of the
    // document, so only the first track in a set was ever seen.
    let xml = live_9_set(&format!(
        "{}\n{}\n{}\n{}",
        LIVE_9_TRACK.replace("3-Audio", "One"),
        LIVE_9_TRACK.replace("3-Audio", "Two"),
        LIVE_9_TRACK.replace("3-Audio", "Three"),
        LIVE_9_TRACK.replace("3-Audio", "Four"),
    ));
    let p = parse_ok(&xml);
    assert_eq!(p.tracks.len(), 4);
    assert_eq!(p.track_count, 4);
}

// ─── The file must be found at all ───────────────────────────────────────────

#[test]
fn the_fixture_file_exists() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/test_project.als");
    assert!(fixture.exists(), "fixture should exist");
}
