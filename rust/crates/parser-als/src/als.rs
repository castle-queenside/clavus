//! Parse an Ableton `.als` (gzip XML) into a `Project` struct.

use clavus_core::types::{Marker, Project, ProjectType, Track};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::fs;
use std::io::Read;
use std::path::Path;
use tracing::debug;

/// A parsed Ableton Live Set.
#[derive(Debug)]
pub struct AlsParser {
    pub path: std::path::PathBuf,
    /// Raw decompressed XML bytes (kept for injection).
    pub xml_bytes: Vec<u8>,
    /// Parsed project data.
    pub project: Project,
}

impl AlsParser {
    /// Parse an `.als` file.
    pub fn new(path: &Path) -> Result<Self, clavus_core::ClavusError> {
        let raw_bytes = fs::read(path)
            .map_err(|e| clavus_core::ClavusError::ParseError(format!("read {}: {}", path.display(), e)))?;

        let xml_bytes = flate2_decode(&raw_bytes)
            .map_err(|e| clavus_core::ClavusError::ParseError(format!("gzip: {}", e)))?;

        let mut parser = Self {
            path: path.to_path_buf(),
            xml_bytes,
            project: Project {
                path: path.to_path_buf(),
                project_type: ProjectType::Ableton,
                name: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                head: None,
                track_count: 0,
                bpm: 120.0,
                time_signature: "4/4".to_string(),
                ableton_version: None,
                schema_version: None,
                tracks: vec![],
                return_tracks: vec![],
                master_track: None,
                markers: vec![],
                tempo_events: vec![],
            },
        };

        parser.parse_xml()?;
        Ok(parser)
    }

    fn parse_xml(&mut self) -> Result<(), clavus_core::ClavusError> {
        let xml = String::from_utf8(self.xml_bytes.clone())
            .map_err(clavus_core::ClavusError::Utf8Error)?;

        let mut reader = Reader::from_str(&xml);
        reader.trim_text(true);
        let mut buf = Vec::new();
        let mut in_liveset = false;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag = tag_name(&e);
                    match tag.as_str() {
                        "LiveSet" => {
                            in_liveset = true;
                            if let Some(v) = attr_val(&e, "MajorVersion") {
                                if self.project.ableton_version.is_none() {
                                    self.project.ableton_version = Some(v);
                                }
                            }
                        }
                        "AudioTrack" | "MidiTrack" | "GroupTrack" | "ReturnTrack" => {
                            if in_liveset {
                                if let Ok(track) = Self::parse_track(&mut reader, &tag, &mut buf) {
                                    if tag == "ReturnTrack" {
                                        self.project.return_tracks.push(track);
                                    } else {
                                        self.project.tracks.push(track);
                                    }
                                }
                            }
                        }
                        "MasterTrack" => {
                            if let Some(track) = Self::parse_master_track(&mut reader, &mut buf) {
                                self.project.master_track = Some(track);
                            }
                        }
                        "Tempo" => {
                            if let Some(bpm) = attr_val(&e, "Value")
                                .and_then(|v| v.parse::<f64>().ok())
                            {
                                self.project.bpm = bpm;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    let tag = tag_name(&e);
                    match tag.as_str() {
                        "AudioTrack" | "MidiTrack" | "GroupTrack" | "ReturnTrack" => {
                            if in_liveset {
                                if let Ok(track) = Self::parse_track(&mut reader, &tag, &mut buf) {
                                    if tag == "ReturnTrack" {
                                        self.project.return_tracks.push(track);
                                    } else {
                                        self.project.tracks.push(track);
                                    }
                                }
                            }
                        }
                        "Marker" | "SingleNoteMarker" | "ArrangementMarker" => {
                            let time = attr_val(&e, "Time").unwrap_or_default();
                            let name = attr_val(&e, "Name").unwrap_or_default();
                            if !time.is_empty() || !name.is_empty() {
                                self.project.markers.push(Marker { time, name });
                            }
                        }
                        "Tempo" => {
                            if let Some(bpm) = attr_val(&e, "Value")
                                .and_then(|v| v.parse::<f64>().ok())
                            {
                                self.project.bpm = bpm;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    if bytes_end_name(&e) == "LiveSet" {
                        in_liveset = false;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        self.project.track_count = self.project.tracks.len();
        debug!(
            "Parsed {} tracks, {} returns, {} markers",
            self.project.tracks.len(),
            self.project.return_tracks.len(),
            self.project.markers.len()
        );
        Ok(())
    }

    fn parse_track(reader: &mut Reader<&[u8]>, tag: &str, buf: &mut Vec<u8>) -> Result<Track, ()> {
        let track_type = match tag {
            "AudioTrack" => "Audio",
            "MidiTrack" => "MIDI",
            "GroupTrack" => "Group",
            "ReturnTrack" => "Return",
            _ => "Unknown",
        };

        let mut track = Track {
            name: "Unnamed".to_string(),
            track_type: track_type.to_string(),
            index: 0,
            color: 16777215,
            is_frozen: false,
            is_muted: false,
            is_solo: false,
            devices: vec![],
            sends: std::collections::HashMap::new(),
            clips: vec![],
        };

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    match etag.as_str() {
                        "Name" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if !v.is_empty() {
                                    track.name = v;
                                }
                            }
                        }
                        "EffectiveName" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if !v.is_empty() && track.name == "Unnamed" {
                                    track.name = v;
                                }
                            }
                        }
                        "DeviceChain" => {
                            track.devices = Self::parse_device_chain(reader, buf);
                        }
                        "Mute" => {
                            if attr_val(&e, "Value") == Some("true".to_string()) {
                                track.is_muted = true;
                            }
                        }
                        "Solo" => {
                            if attr_val(&e, "Value") == Some("true".to_string()) {
                                track.is_solo = true;
                            }
                        }
                        "IsFrozen" => {
                            if attr_val(&e, "Value") == Some("true".to_string()) {
                                track.is_frozen = true;
                            }
                        }
                        "Color" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if let Ok(c) = v.parse::<u32>() {
                                    track.color = c;
                                }
                            }
                        }
                        "MidiClip" | "AudioClip" => {
                            if let Some(clip) = Self::parse_clip(reader, &etag, buf) {
                                track.clips.push(clip);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    if bytes_end_name(&e) == tag {
                        break;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        Ok(track)
    }

    fn parse_master_track(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) -> Option<Track> {
        let mut track = Track {
            name: "Master".to_string(),
            track_type: "Master".to_string(),
            ..Default::default()
        };

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    if etag == "DeviceChain" {
                        track.devices = Self::parse_device_chain(reader, buf);
                    }
                }
                Ok(Event::End(e)) => {
                    if bytes_end_name(&e) == "MasterTrack" {
                        break;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        Some(track)
    }

    fn parse_device_chain(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) -> Vec<clavus_core::types::Device> {
        let mut devices = Vec::new();
        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    // Skip structural/sequencer elements — collect plugin/device names
                    if !["AudioTrack", "MidiTrack", "GroupTrack", "ReturnTrack",
                         "DeviceChain", "Track", "MasterTrack", "MasterChain",
                         "MidiIO", "Sequencer", "NoteStream", "DeviceGenerator",
                         "Track", "NoteRow", "ClipSlotList", "ClipSlot"].contains(&etag.as_str()) {
                        let name = attr_val(&e, "Value").unwrap_or_else(|| etag.clone());
                        let device_type = etag.clone();
                        devices.push(clavus_core::types::Device {
                            name,
                            device_type,
                        });
                    }

                    // Skip nested content of structural elements
                    if let Ok(Event::Start(_)) = reader.read_event_into(buf) {
                        loop {
                            match reader.read_event_into(buf) {
                                Ok(Event::End(e)) => {
                                    if bytes_end_name(&e) == etag { break; }
                                }
                                Ok(Event::Eof) => break,
                                _ => {}
                            }
                            buf.clear();
                        }
                    }
                }
                Ok(Event::End(e)) => {
                    if bytes_end_name(&e) == "DeviceChain" {
                        break;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        devices
    }

    fn parse_clip(reader: &mut Reader<&[u8]>, tag: &str, buf: &mut Vec<u8>) -> Option<clavus_core::types::Clip> {
        let mut start_beats = 0.0f64;
        let mut end_beats = 4.0f64;
        let mut name = String::new();
        let mut color = 16777215u32;

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    match etag.as_str() {
                        "Time" | "CurrentStart" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if let Ok(val) = v.parse::<f64>() {
                                    start_beats = val;
                                }
                            }
                        }
                        "CurrentEnd" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if let Ok(val) = v.parse::<f64>() {
                                    end_beats = val;
                                }
                            }
                        }
                        "Name" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                name = v;
                            }
                        }
                        "Color" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if let Ok(c) = v.parse::<u32>() {
                                    color = c;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    if bytes_end_name(&e) == tag {
                        return Some(clavus_core::types::Clip {
                            start_beats,
                            end_beats,
                            name,
                            color,
                            clip_type: tag.to_string(),
                        });
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        None
    }
}

/// Decompress gzip.
fn flate2_decode(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

// ─── Helper functions ────────────────────────────────────────────────

/// Get the tag name as a String from a BytesStart event.
fn tag_name(e: &quick_xml::events::BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.name().as_ref()).to_string()
}

/// Get the tag name from a BytesEnd event.
fn bytes_end_name(e: &quick_xml::events::BytesEnd<'_>) -> String {
    String::from_utf8_lossy(e.name().as_ref()).to_string()
}

/// Get the value of an attribute by key name from a BytesStart element.
fn attr_val(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|attr| String::from_utf8_lossy(attr.key.as_ref()) == key)
        .map(|attr| String::from_utf8_lossy(attr.value.as_ref()).to_string())
}
