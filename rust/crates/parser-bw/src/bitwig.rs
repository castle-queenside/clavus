//! Parse a Bitwig `.bwproject` ZIP archive.

use clavus_core::types::{Marker, Project, ProjectType, Track};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::io::Read;
use std::path::Path;
use tracing::debug;
use zip::ZipArchive;

/// A parsed Bitwig Studio project.
#[derive(Debug)]
pub struct BitwigParser {
    pub path: std::path::PathBuf,
    pub xml_bytes: Vec<u8>,
    pub project: Project,
}

impl BitwigParser {
    /// Parse a `.bwproject` ZIP archive.
    pub fn new(path: &Path) -> Result<Self, clavus_core::ClavusError> {
        let file = std::fs::File::open(path)
            .map_err(|e| clavus_core::ClavusError::ParseError(format!("open {}: {}", path.display(), e)))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|e| clavus_core::ClavusError::ParseError(format!("zip open: {}", e)))?;

        let mut xml_bytes = Vec::new();
        let mut found = false;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)
                .map_err(|e| clavus_core::ClavusError::ParseError(format!("zip entry: {}", e)))?;
            let name = entry.name().to_string();
            if name == "project.xml" || name.ends_with("/project.xml") || name.ends_with("\\project.xml") {
                entry.read_to_end(&mut xml_bytes)
                    .map_err(|e| clavus_core::ClavusError::ParseError(format!("read project.xml: {}", e)))?;
                found = true;
                break;
            }
        }

        if !found || xml_bytes.is_empty() {
            return Err(clavus_core::ClavusError::ParseError(
                "No project.xml found in .bwproject".into(),
            ));
        }

        let mut parser = Self {
            path: path.to_path_buf(),
            xml_bytes,
            project: Project {
                path: path.to_path_buf(),
                project_type: ProjectType::Bitwig,
                name: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                head: None,
                track_count: 0,
                bpm: 120.0,
                time_signature: "4/4".to_string(),
                ableton_version: Some("Bitwig".to_string()),
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

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let tag = tag_name(&e);
                    match tag.as_str() {
                        "Project" => {
                            if let Some(v) = attr_val(&e, "VersionMajor") {
                                if self.project.schema_version.is_none() {
                                    self.project.schema_version = Some(v);
                                }
                            }
                        }
                        "Track" => {
                            if let Ok(track) = Self::parse_track(&mut reader, &mut buf) {
                                self.project.tracks.push(track);
                            }
                        }
                        "Locator" | "Marker" => {
                            let time = attr_val(&e, "Time")
                                .or_else(|| attr_val(&e, "Position"))
                                .unwrap_or_default();
                            let name = attr_val(&e, "Name")
                                .or_else(|| attr_val(&e, "Label"))
                                .unwrap_or_default();
                            if !time.is_empty() || !name.is_empty() {
                                self.project.markers.push(Marker { time, name });
                            }
                        }
                        "Tempo" => {
                            if let Some(bpm) = attr_val(&e, "Bpm")
                                .or_else(|| attr_val(&e, "Value"))
                                .and_then(|v| v.parse::<f64>().ok())
                            {
                                self.project.bpm = bpm;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        self.project.track_count = self.project.tracks.len();
        debug!(
            "Parsed Bitwig: {} tracks, {} markers, BPM {}",
            self.project.tracks.len(),
            self.project.markers.len(),
            self.project.bpm
        );
        Ok(())
    }

    fn parse_track(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) -> Result<Track, ()> {
        let mut track = Track::default();

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let tag = tag_name(&e);
                    match tag.as_str() {
                        "Name" | "EffectiveName" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if !v.is_empty() && track.name.is_empty() {
                                    track.name = v;
                                }
                            }
                        }
                        "Type" | "ChannelType" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if !v.is_empty() {
                                    track.track_type = v;
                                }
                            }
                        }
                        "DeviceChain" | "Device" => {
                            let name = attr_val(&e, "Value").unwrap_or_else(|| tag.clone());
                            track.devices.push(clavus_core::types::Device {
                                name,
                                device_type: tag,
                            });
                        }
                        "Mute" | "Muted" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if v == "true" || v == "1" {
                                    track.is_muted = true;
                                }
                            }
                        }
                        "Solo" | "Soloed" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if v == "true" || v == "1" {
                                    track.is_solo = true;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    if bytes_end_name(&e) == "Track" {
                        break;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        if track.name.is_empty() {
            track.name = "Unnamed".to_string();
        }
        if track.track_type.is_empty() {
            track.track_type = "Audio".to_string();
        }
        Ok(track)
    }
}

// ─── Helper functions ────────────────────────────────────────────────

fn tag_name(e: &quick_xml::events::BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.name().as_ref()).to_string()
}

fn bytes_end_name(e: &quick_xml::events::BytesEnd<'_>) -> String {
    String::from_utf8_lossy(e.name().as_ref()).to_string()
}

fn attr_val(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|attr| String::from_utf8_lossy(attr.key.as_ref()) == key)
        .map(|attr| String::from_utf8_lossy(attr.value.as_ref()).to_string())
}
