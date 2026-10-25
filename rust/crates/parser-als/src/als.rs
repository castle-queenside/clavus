//! Parse an Ableton `.als` (gzip XML) into a `Project` struct.

use clavus_core::types::{Marker, Project, ProjectType, Track};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::fs;
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

        Self::from_bytes(path, &raw_bytes)
    }

    /// Parse an `.als` from bytes that are already in memory.
    ///
    /// A snapshot stores the bytes of the set it took. Comparing two
    /// snapshots means parsing two sets that are not on disk — they
    /// are in the store — so the parser has to be able to work from
    /// bytes as well as from a file.
    pub fn from_bytes(path: &Path, raw_bytes: &[u8]) -> Result<Self, clavus_core::ClavusError> {
        let xml_bytes = clavus_core::sample::decompress_als_bytes(raw_bytes)
            .map(|(bytes, _gzipped)| bytes)
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
        // Real Ableton Live wraps regular tracks in <Tracks> and return tracks
        // in <ReturnTracks>. Track which container we're inside so a bare
        // <Track> element is only counted as a real track inside <Tracks>.
        let mut in_tracks = false;
        let mut in_return_tracks = false;
        // Tempo can live in three places depending on Live version:
        //   <Tempo Value="128.0"/>
        //   <Tempo><Value Value="128.0"/></Tempo>
        //   MasterTrack > DeviceChain > TempoEnvelope > Events > AudioPoint
        let mut in_tempo = false;
        let mut in_tempo_envelope = false;
        // A tempo inside a scene is that scene's tempo, and Live 11 and
        // 12 give every scene one — so a tempo read without this context
        // was a scene's tempo, not the project's. A track's own tempo is
        // not a worry here: the track parser consumes the whole track
        // subtree, so the tempo of a track never reaches this loop.
        let mut in_scene = false;
        let mut tempo_events: Vec<(f64, f64)> = Vec::new();
        // The tempo the master track declared, if any.
        let mut master_tempo: Option<f64> = None;
        // Track whether we saw a real Ableton root, and capture XML errors
        // instead of silently swallowing them.
        let mut saw_root = false;
        let mut xml_error: Option<String> = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag = tag_name(&e);
                    match tag.as_str() {
                        "Ableton" => {
                            saw_root = true;
                            // The version lives on the root element:
                            // <Ableton MajorVersion="5" MinorVersion="30">.
                            if let Some(v) = attr_val(&e, "MajorVersion") {
                                if self.project.ableton_version.is_none() {
                                    self.project.ableton_version = Some(v);
                                }
                            }
                        }
                        "LiveSet" => {
                            saw_root = true;
                            in_liveset = true;
                            if let Some(v) = attr_val(&e, "MajorVersion") {
                                if self.project.ableton_version.is_none() {
                                    self.project.ableton_version = Some(v);
                                }
                            }
                        }
                        "Tracks" => in_tracks = true,
                        "ReturnTracks" => in_return_tracks = true,
                        "Scene" => in_scene = true,
                        // Real Live format: <Track> inside <Tracks>
                        "Track" if in_liveset && in_tracks && !in_return_tracks => {
                            if let Ok(track) = Self::parse_track(&mut reader, &tag, &mut buf) {
                                self.project.tracks.push(track);
                            }
                        }
                        // Live 9 and later: <AudioTrack>, <MidiTrack>,
                        // <KeyTrack> and <GroupTrack>. Live 12 renamed
                        // the master track to <MainTrack>.
                        "AudioTrack" | "MidiTrack" | "KeyTrack" | "GroupTrack" | "ReturnTrack" => {
                            if in_liveset {
                                let parsed = Self::parse_track(&mut reader, &tag, &mut buf);
                                if let Ok(track) = parsed {
                                    if tag == "ReturnTrack" {
                                        self.project.return_tracks.push(track);
                                    } else {
                                        self.project.tracks.push(track);
                                    }
                                }
                            }
                        }
                        // The audition track is not a track of the user's.
                        "PreHearTrack" => {
                            skip_to_end(&mut reader, &mut buf);
                        }
                        "MasterTrack" | "MainTrack" => {
                            let (track, tempo) =
                                Self::parse_master_track(&mut reader, &mut buf, &mut tempo_events);
                            if let Some(track) = track {
                                self.project.master_track = Some(track);
                            }
                            // The master track's tempo is the project's
                            // tempo. It wins over anything else.
                            if let Some(bpm) = tempo {
                                self.project.bpm = bpm;
                                master_tempo = Some(bpm);
                            }
                        }
                        "Tempo" => {
                            in_tempo = true;
                            // A tempo inside a scene is that scene's
                            // tempo, and Live 11 and 12 give every scene
                            // one. A track's own tempo never reaches this
                            // loop: the track parser consumes the whole
                            // track subtree.
                            if !in_scene && master_tempo.is_none() {
                                if let Some(bpm) = attr_val(&e, "Value")
                                    .and_then(|v| v.parse::<f64>().ok())
                                {
                                    self.project.bpm = bpm;
                                }
                            }
                        }
                        "TempoEnvelope" => {
                            in_tempo_envelope = true;
                        }
                        // Real Live stores arrangement markers as
                        // <CuePoint><Time Value="4.1.1"/><Name Value="Intro"/></CuePoint>
                        // (older versions used <Locator> with the same shape).
                        "CuePoint" | "Locator" => {
                            if let Some(marker) = Self::parse_marker_point(&mut reader, &mut buf) {
                                self.project.markers.push(marker);
                            }
                        }
                        "AudioPoint" if in_tempo_envelope => {
                            // Canonical Live location:
                            // <AudioPoint Time="0" Value="128.0" .../>
                            if let Some(bpm) = attr_val(&e, "Value")
                                .and_then(|v| v.parse::<f64>().ok())
                            {
                                let time = attr_val(&e, "Time")
                                    .and_then(|t| t.parse::<f64>().ok())
                                    .unwrap_or(0.0);
                                tempo_events.push((time, bpm));
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    let tag = tag_name(&e);
                    match tag.as_str() {
                        // <Tempo><Value Value="128.0"/></Tempo>
                        "Value" if in_tempo => {
                            if !in_scene && master_tempo.is_none() {
                                if let Some(bpm) = attr_val(&e, "Value")
                                    .and_then(|v| v.parse::<f64>().ok())
                                {
                                    self.project.bpm = bpm;
                                }
                            }
                        }
                        // <Tempo><Manual Value="128.0"/></Tempo> — the
                        // form Live 10 and later write.
                        "Manual" if in_tempo => {
                            if !in_scene && master_tempo.is_none() {
                                if let Some(bpm) = attr_val(&e, "Value")
                                    .and_then(|v| v.parse::<f64>().ok())
                                {
                                    self.project.bpm = bpm;
                                }
                            }
                        }
                        "Track" if in_liveset && in_tracks && !in_return_tracks => {
                            if let Ok(track) = Self::parse_track(&mut reader, &tag, &mut buf) {
                                self.project.tracks.push(track);
                            }
                        }
                        "AudioTrack" | "MidiTrack" | "KeyTrack" | "GroupTrack" | "ReturnTrack" => {
                            if in_liveset {
                                // A self-closing track element has no
                                // content to read. Handing it to the track
                                // parser would look for a closing tag that
                                // never comes and swallow the rest of the
                                // document.
                                let track = Track {
                                    name: if tag == "KeyTrack" {
                                        "MIDI Keyboard".to_string()
                                    } else {
                                        "Unnamed".to_string()
                                    },
                                    track_type: match tag.as_str() {
                                        "AudioTrack" => "Audio",
                                        "MidiTrack" | "KeyTrack" => "MIDI",
                                        "GroupTrack" => "Group",
                                        _ => "Return",
                                    }
                                    .to_string(),
                                    ..Default::default()
                                };
                                if tag == "ReturnTrack" {
                                    self.project.return_tracks.push(track);
                                } else {
                                    self.project.tracks.push(track);
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
                        "Tempo" if !in_scene && master_tempo.is_none() => {
                            if let Some(bpm) =
                                attr_val(&e, "Value").and_then(|v| v.parse::<f64>().ok())
                            {
                                self.project.bpm = bpm;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(e)) => {
                    match bytes_end_name(&e).as_str() {
                        "LiveSet" => in_liveset = false,
                        "Tracks" => in_tracks = false,
                        "ReturnTracks" => in_return_tracks = false,
                        "Scene" => in_scene = false,
                        "Tempo" => in_tempo = false,
                        "TempoEnvelope" => in_tempo_envelope = false,
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => {
                    xml_error = Some(e.to_string());
                    break;
                }
                _ => {}
            }
            buf.clear();
        }

        if let Some(err) = xml_error {
            return Err(clavus_core::ClavusError::ParseError(format!(
                "malformed XML in {}: {}",
                self.path.display(),
                err
            )));
        }

        if !saw_root {
            return Err(clavus_core::ClavusError::ParseError(format!(
                "not an Ableton Live project: no <Ableton> or <LiveSet> element in {}",
                self.path.display()
            )));
        }

        // The XML reader accepts a closing tag whose `>` is missing, so a
        // set truncated by a single byte used to parse as a complete set
        // with every track still in it — and was then snapshotted as if
        // it were healthy. A well-formed document always ends with `>`.
        if !clavus_core::als_refs::is_complete_document(&xml) {
            return Err(clavus_core::ClavusError::ParseError(format!(
                "malformed XML in {}: the document is truncated",
                self.path.display()
            )));
        }

        // Live does not always store a track's name. Its own interface
        // shows a numbered name for those — "1-MIDI", "2-Audio" — so
        // name them the same way rather than calling every one of them
        // "Unnamed".
        for (i, track) in self.project.tracks.iter_mut().enumerate() {
            track.index = i;
            if track.name == "Unnamed" {
                track.name = format!("{}-{}", i + 1, track.track_type);
            }
        }
        for (i, track) in self.project.return_tracks.iter_mut().enumerate() {
            track.index = i;
            if track.name == "Unnamed" {
                track.name = format!("{}-{}", i + 1, track.track_type);
            }
        }

        self.project.track_count = self.project.tracks.len();

        // Deduplicate markers: some projects carry the same arrangement
        // markers in both <CuePoints> and <Locators>.
        let mut seen = std::collections::HashSet::new();
        self.project
            .markers
            .retain(|m| seen.insert((m.time.clone(), m.name.clone())));

        // TempoEnvelope events are authoritative when present: the project
        // BPM is the value of the first tempo event.
        if !tempo_events.is_empty() {
            tempo_events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            self.project.bpm = tempo_events[0].1;
        }
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
            "KeyTrack" => "MIDI",
            "GroupTrack" => "Group",
            "ReturnTrack" => "Return",
            // Live 8 writes a bare <Track> and says what it is in the
            // device chain: <MidiEffectChain> against <AudioEffectChain>.
            "Track" => "Audio",
            _ => "Unknown",
        };

        // Live writes MIDI keyboard tracks as <KeyTrack> and gives them
        // no name at all, so name them the way Live's own interface does.
        let default_name = if tag == "KeyTrack" { "MIDI Keyboard" } else { "Unnamed" };

        let mut track = Track {
            name: default_name.to_string(),
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

        // Live 8 writes <Mixer><Track>...</Track></Mixer> inside every
        // track, so an element with the same name as the track can turn up
        // nested inside it. Counting how many of those are open says
        // whether a closing tag ends this track or one of its own.
        //
        // The old code counted every element instead, which meant the
        // count was never zero when the track's own closing tag arrived
        // and the track never ended — so one track swallowed the rest of
        // the file, and every real set looked like it had one track.
        let mut same_name_depth: usize = 0;

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    if etag == tag {
                        same_name_depth += 1;
                    }
                    match etag.as_str() {
                        "Name" => {
                            // Live 11 and later write the name as a
                            // container:
                            //   <Name><EffectiveName Value="SHAKER"/>
                            //         <UserName Value="SHAKER"/></Name>
                            // Live 8 and 9 write <Name Value="SHAKER"/>.
                            if let Some(v) = attr_val(&e, "Value") {
                                if !v.is_empty() {
                                    track.name = v;
                                }
                            } else {
                                // The container form. The helper reads
                                // through the closing </Name>, so there is
                                // nothing left to skip.
                                if let Some(v) = Self::parse_name_container(reader, buf) {
                                    track.name = v;
                                }
                            }
                        }
                        "EffectiveName" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if !v.is_empty() && track.name == default_name {
                                    track.name = v;
                                }
                            }
                            skip_to_end(reader, buf);
                        }
                        "DeviceChain" => {
                            let (devices, chain_kind, _tempo) = Self::parse_device_chain(reader, buf);
                            track.devices = devices;
                            // Real Live writes a bare <Track>; the chain kind
                            // tells us whether it's MIDI or audio.
                            if tag == "Track" {
                                if let Some(kind) = chain_kind {
                                    track.track_type = kind;
                                }
                            }
                        }
                        "Mute" => {
                            if attr_val(&e, "Value") == Some("true".to_string()) {
                                track.is_muted = true;
                            }
                            skip_to_end(reader, buf);
                        }
                        "Solo" => {
                            if attr_val(&e, "Value") == Some("true".to_string()) {
                                track.is_solo = true;
                            }
                            skip_to_end(reader, buf);
                        }
                        "IsFrozen" => {
                            if attr_val(&e, "Value") == Some("true".to_string()) {
                                track.is_frozen = true;
                            }
                            skip_to_end(reader, buf);
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
                        "SendChannels" => {
                            let sends = Self::parse_send_channels(reader, buf);
                            if !sends.is_empty() {
                                track.sends = sends;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    match etag.as_str() {
                        // Real Live writes <Name Value="Kick"/> self-closing,
                        // so the name arrives as an Empty event, not a Start.
                        "Name" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                // First name wins: the track's own <Name> comes
                                // before any nested send/device names.
                                if !v.is_empty() && track.name == default_name {
                                    track.name = v;
                                }
                            }
                        }
                        "EffectiveName" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                if !v.is_empty() && track.name == default_name {
                                    track.name = v;
                                }
                            }
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
                    // Only the track's own closing tag ends this track.
                    // A nested element with the same name — Live 8's
                    // <Mixer><Track> — does not.
                    if bytes_end_name(&e) == tag {
                        if same_name_depth == 0 {
                            break;
                        }
                        same_name_depth -= 1;
                    }
                }

                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        Ok(track)
    }


    /// Read a `<Name>` container and return the name it holds.
    ///
    /// Live 11 and later write a track's name as
    /// ```xml
    /// <Name>
    ///   <EffectiveName Value="SHAKER" />
    ///   <UserName Value="SHAKER" />
    /// </Name>
    /// ```
    /// `UserName` is what the user typed; `EffectiveName` is Live's own
    /// name for the track. The user's name wins when they gave one.
    fn parse_name_container(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) -> Option<String> {
        let mut effective: Option<String> = None;
        let mut user: Option<String> = None;

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    if etag == "EffectiveName" {
                        effective = attr_val(&e, "Value");
                    } else if etag == "UserName" {
                        user = attr_val(&e, "Value");
                    }
                    skip_to_end(reader, buf);
                }
                Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    if etag == "EffectiveName" {
                        effective = attr_val(&e, "Value");
                    } else if etag == "UserName" {
                        user = attr_val(&e, "Value");
                    }
                }
                Ok(Event::End(e)) => {
                    if bytes_end_name(&e) == "Name" {
                        break;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        user.filter(|u| !u.is_empty())
            .or_else(|| effective.filter(|e| !e.is_empty()))
    }

    fn parse_master_track(
        reader: &mut Reader<&[u8]>,
        buf: &mut Vec<u8>,
        tempo_events: &mut Vec<(f64, f64)>,
    ) -> (Option<Track>, Option<f64>) {
        let mut track = Track {
            name: "Master".to_string(),
            track_type: "Master".to_string(),
            ..Default::default()
        };
        // The project's tempo lives in the master track, and Live has
        // written it three different ways:
        //
        //   Live 10 and later: <Tempo><Manual Value="92"/></Tempo>
        //   Live 8 and 9:     <Tempo><ArrangerAutomation><Events>
        //                         <FloatEvent Time="..." Value="92"/>
        //                     </Events></ArrangerAutomation></Tempo>
        //   older:            <Tempo Value="92"/>
        //
        // A tempo found inside a *track* is that track's own tempo, and
        // Live 11 and 12 give every track one. Taking the last one
        // read meant every set claimed to be 120 BPM.
        let mut tempo: Option<f64> = None;
        let mut in_tempo = false;
        let mut in_tempo_events = false;

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    match etag.as_str() {
                        "DeviceChain" => {
                            let (devices, _, env_tempo) = Self::parse_device_chain(reader, buf);
                            tempo_events.extend(env_tempo);
                            track.devices = devices;
                        }
                        "Tempo" => {
                            in_tempo = true;
                            // The oldest form put the value on the tag.
                            if let Some(bpm) = attr_val(&e, "Value").and_then(|v| v.parse::<f64>().ok())
                            {
                                tempo = Some(bpm);
                            }
                        }
                        "Events" if in_tempo => {
                            in_tempo_events = true;
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    if in_tempo {
                        let value = attr_val(&e, "Value").and_then(|v| v.parse::<f64>().ok());
                        match etag.as_str() {
                            // Live 10 and later.
                            "Manual" => {
                                if let Some(bpm) = value {
                                    tempo = Some(bpm);
                                }
                            }
                            "Value" if !in_tempo_events => {
                                if let Some(bpm) = value {
                                    tempo = Some(bpm);
                                }
                            }
                            // Live 8 and 9 keep the tempo as an
                            // automation curve; the first point is the
                            // project's tempo.
                            "FloatEvent" if in_tempo_events && tempo.is_none() => {
                                if let Some(bpm) = value {
                                    tempo = Some(bpm);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Event::End(e)) => {
                    match bytes_end_name(&e).as_str() {
                        "MasterTrack" | "MainTrack" => break,
                        "Tempo" => {
                            in_tempo = false;
                            in_tempo_events = false;
                        }
                        "Events" => in_tempo_events = false,
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        (Some(track), tempo)
    }

    /// Parse a `<DeviceChain>` subtree.
    ///
    /// Returns the device list plus the chain kind, if determinable.
    /// Real Ableton Live writes either `<MidiEffectChain>` or `<AudioEffectChain>`
    /// as the first child of `<DeviceChain>`; that element is what tells us
    /// whether a `<Track>` is a MIDI or an audio track.
    fn parse_device_chain(
        reader: &mut Reader<&[u8]>,
        buf: &mut Vec<u8>,
    ) -> (
        Vec<clavus_core::types::Device>,
        Option<String>,
        Vec<(f64, f64)>,
    ) {
        // Blocklist: structural/sequencer elements whose Start tags must skip
        // their entire nested subtree so children aren't treated as devices.
        let skip_blocklist = ["AudioTrack", "MidiTrack", "GroupTrack", "ReturnTrack",
            "Track", "MasterTrack", "MasterChain",
            "MidiIO", "Sequencer", "NoteStream", "DeviceGenerator",
            "NoteRow", "ClipSlotList", "ClipSlot",
            "Mute", "Solo", "IsFrozen", "Freeze", "FreezeState",
            "Value", "PresenterState", "LockedDevices", "Locators", "Locator",
            "DrumChain", "InstrumentRef", "Sender", "Receiver", "Annotation",
            "Name", "TempoEnvelope"];

        let mut devices: Vec<clavus_core::types::Device> = Vec::new();
        // Stack of indices into `devices` for elements that are still open.
        // A <Name Value="..."/> inside a device belongs to that device, so we
        // need to know which one is innermost.
        let mut device_stack: Vec<usize> = Vec::new();
        let mut chain_kind: Option<String> = None;
        let mut tempo_events: Vec<(f64, f64)> = Vec::new();
        // Live keeps the actual devices in a `<Devices>` list inside the
        // chain. Everything else in a chain is structure — the mixer, the
        // routing, the automation lanes — and reading those as devices
        // produced a list of every element in the file.
        //
        // `devices_depth` says how far inside that list we are: 1 means
        // a device, 2 means inside a device, and 0 means not in the list
        // at all.
        let mut devices_depth = 0usize;
        // Live writes the devices in a chain *nested inside* the track's
        // own chain:
        //
        //   <DeviceChain>            <- the track's chain: mixer, routing
        //     ...
        //     <DeviceChain>          <- the devices live in here
        //       <Devices>
        //         <Eq8 Id="1">...
        //       </Devices>
        //     </DeviceChain>
        //   </DeviceChain>
        //
        // So the nested chain is descended into rather than skipped.
        let mut chain_depth = 1usize;
        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    // Extract the attributes up front so the borrow of `buf`
                    // ends before any sub-parser is invoked.
                    let e_name = attr_val(&e, "Value").unwrap_or_else(|| etag.clone());
                    let e_type = etag.clone();
                    if etag == "Devices" {
                        devices_depth = 1;
                        buf.clear();
                        continue;
                    }
                    if etag == "DeviceChain" {
                        // The nested chain that actually holds the devices.
                        chain_depth += 1;
                        buf.clear();
                        continue;
                    }
                    if etag == "MidiEffectChain" {
                        chain_kind = Some("MIDI".to_string());
                    } else if etag == "AudioEffectChain" {
                        chain_kind = Some("Audio".to_string());
                    } else if etag == "TempoEnvelope" {
                        // The master track's tempo envelope lives inside the
                        // device chain: <TempoEnvelope><Events><AudioPoint .../>
                        let mut env = Self::parse_tempo_envelope(reader, buf);
                        tempo_events.append(&mut env);
                        buf.clear();
                        continue;
                    }
                    let in_device_list = devices_depth == 1;
                    if devices_depth > 0 {
                        devices_depth += 1;
                    }
                    if etag == "Name" {
                        // A renamed device carries its name on the
                        // element itself: <Name Value="My EQ"/>.
                        if let Some(&idx) = device_stack.last() {
                            let d = &mut devices[idx];
                            if d.name == d.device_type {
                                if let Some(v) = attr_val(&e, "Value") {
                                    if !v.is_empty() {
                                        d.name = v;
                                    }
                                }
                            }
                        }
                    }
                    if skip_blocklist.contains(&etag.as_str()) {
                        // Structural element: skip all nested content
                        skip_to_end(reader, buf);
                    } else if in_device_list {
                        // Device: add it, let nested content be processed by outer loop
                        // so intermediate tags (e.g. <Name> inside <Compressor>) are
                        // handled correctly without being treated as top-level devices.
                        devices.push(clavus_core::types::Device { name: e_name, device_type: e_type });
                        device_stack.push(devices.len() - 1);
                    } else if devices_depth > 1 {
                        // Inside a device: its parameters are not devices.
                    } else {
                        // Structure that is not a device: skip it whole.
                        skip_to_end(reader, buf);
                    }
                }
                Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    if matches!(etag.as_str(), "Name" | "UserName" | "PlugName") {
                        // Where a device's name lives depends on the
                        // version. Live 9 and later write
                        // <Name Value="..."/>; Live 8 writes
                        // <UserName Value="..."/> and
                        // <PlugName Value="Maschine"/>. The first one
                        // that carries text names the device.
                        if let Some(&idx) = device_stack.last() {
                            let d = &mut devices[idx];
                            if d.name == d.device_type {
                                if let Some(v) = attr_val(&e, "Value") {
                                    if !v.is_empty() {
                                        d.name = v;
                                    }
                                }
                            }
                        }
                    } else if devices_depth == 1 && !skip_blocklist.contains(&etag.as_str()) {
                        // A device written as an empty element.
                        let name = attr_val(&e, "Value").unwrap_or_else(|| etag.clone());
                        let device_type = etag.clone();
                        devices.push(clavus_core::types::Device { name, device_type });
                        device_stack.push(devices.len() - 1);
                    }
                }
                Ok(Event::End(e)) => {
                    let ename = bytes_end_name(&e);
                    if ename == "DeviceChain" {
                        if chain_depth <= 1 {
                            // The track's own chain is closing.
                            break;
                        }
                        chain_depth -= 1;
                    } else if devices_depth > 0 {
                        // Coming out of a device: its name is finished.
                        if devices_depth == 2 {
                            device_stack.pop();
                        }
                        devices_depth -= 1;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        // Filter out container elements (not real devices)
        let devices: Vec<_> = devices
            .into_iter()
            .filter(|d| {
                d.device_type != "Devices"
                    && d.device_type != "MidiEffectChain"
                    && d.device_type != "AudioEffectChain"
                    && d.device_type != "MidiEffect"
                    && d.device_type != "AudioEffect"
            })
            .collect();
        (devices, chain_kind, tempo_events)
    }

    /// Parse a `<TempoEnvelope>` subtree, collecting `(time, bpm)` points.
    ///
    /// Real Live writes:
    /// ```xml
    /// <TempoEnvelope>
    ///   <Events>
    ///     <AudioPoint Time="0.0" Value="128.0"/>
    ///   </Events>
    /// </TempoEnvelope>
    /// ```
    fn parse_tempo_envelope(
        reader: &mut Reader<&[u8]>,
        buf: &mut Vec<u8>,
    ) -> Vec<(f64, f64)> {
        let mut events = Vec::new();
        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    if etag == "Events" {
                        // Don't skip: the <AudioPoint> children live inside.
                    } else if etag == "AudioPoint" || etag == "MidiPoint" {
                        if let Some(bpm) =
                            attr_val(&e, "Value").and_then(|v| v.parse::<f64>().ok())
                        {
                            let time = attr_val(&e, "Time")
                                .and_then(|t| t.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            events.push((time, bpm));
                        }
                        skip_to_end(reader, buf);
                    } else {
                        skip_to_end(reader, buf);
                    }
                }
                Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    if etag == "AudioPoint" || etag == "MidiPoint" {
                        if let Some(bpm) =
                            attr_val(&e, "Value").and_then(|v| v.parse::<f64>().ok())
                        {
                            let time = attr_val(&e, "Time")
                                .and_then(|t| t.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            events.push((time, bpm));
                        }
                    }
                }
                Ok(Event::End(e)) => {
                    if bytes_end_name(&e) == "TempoEnvelope" {
                        break;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        events
    }

    /// Parse a `<SendChannels>` subtree, collecting each send's name.
    ///
    /// Real Live writes:
    /// ```xml
    /// <SendChannels>
    ///   <SendChannel><Name Value="Reverb"/>...</SendChannel>
    ///   <SendChannel><Name Value="Delay"/>...</SendChannel>
    /// </SendChannels>
    /// ```
    ///
    /// The whole subtree is consumed so nested `<Name>` elements can't leak
    /// into the track's own name.
    fn parse_send_channels(
        reader: &mut Reader<&[u8]>,
        buf: &mut Vec<u8>,
    ) -> std::collections::HashMap<String, f64> {
        let mut sends = std::collections::HashMap::new();
        let mut pending_name: Option<String> = None;
        let mut pending_level: Option<f64> = None;

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    if etag == "SendChannel" {
                        pending_name = None;
                        pending_level = None;
                    } else if etag == "Name" {
                        if let Some(v) = attr_val(&e, "Value") {
                            pending_name = Some(v);
                        }
                        skip_to_end(reader, buf);
                    } else if etag == "Volume" {
                        // <Volume><Value Value="0.5"/></Volume>
                        if let Some(v) = Self::read_first_value(reader, buf) {
                            pending_level = Some(v);
                        }
                    } else {
                        skip_to_end(reader, buf);
                    }
                }
                Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    if etag == "Name" {
                        if let Some(v) = attr_val(&e, "Value") {
                            pending_name = Some(v);
                        }
                    } else if etag == "Value" {
                        // <Value Value="0.5"/>
                        if let Some(v) = attr_val(&e, "Value").and_then(|s| s.parse::<f64>().ok()) {
                            pending_level = Some(v);
                        }
                    }
                }
                Ok(Event::End(e)) => {
                    let ename = bytes_end_name(&e);
                    if ename == "SendChannel" {
                        if let Some(name) = pending_name.take() {
                            sends.insert(name, pending_level.take().unwrap_or(0.0));
                        }
                    } else if ename == "SendChannels" {
                        break;
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        sends
    }

    /// Read the first `<Value Value="..."/>` found inside the current element.
    fn read_first_value(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) -> Option<f64> {
        let mut result = None;
        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Empty(e)) => {
                    if tag_name(&e) == "Value" {
                        result = attr_val(&e, "Value").and_then(|s| s.parse::<f64>().ok());
                    }
                }
                Ok(Event::End(_)) => break,
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }
        result
    }

    /// Parse a marker container element (`<CuePoint>` / `<Locator>`) and its
    /// `<Time>` / `<Name>` children.
    ///
    /// Real Ableton Live writes:
    /// ```xml
    /// <CuePoint>
    ///   <Time Value="4.1.1"/>
    ///   <Name Value="Intro"/>
    /// </CuePoint>
    /// ```
    fn parse_marker_point(
        reader: &mut Reader<&[u8]>,
        buf: &mut Vec<u8>,
    ) -> Option<clavus_core::types::Marker> {
        let mut time = String::new();
        let mut name = String::new();

        loop {
            match reader.read_event_into(buf) {
                Ok(Event::Start(e)) => {
                    let etag = tag_name(&e);
                    match etag.as_str() {
                        "Time" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                time = v;
                            }
                            skip_to_end(reader, buf);
                        }
                        "Name" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                name = v;
                            }
                            skip_to_end(reader, buf);
                        }
                        _ => {
                            skip_to_end(reader, buf);
                        }
                    }
                }
                Ok(Event::Empty(e)) => {
                    let etag = tag_name(&e);
                    match etag.as_str() {
                        "Time" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                time = v;
                            }
                        }
                        "Name" => {
                            if let Some(v) = attr_val(&e, "Value") {
                                name = v;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::End(_)) => break,
                Ok(Event::Eof) => break,
                _ => {}
            }
            buf.clear();
        }

        if time.is_empty() && name.is_empty() {
            None
        } else {
            Some(clavus_core::types::Marker { time, name })
        }
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

/// Skip all events until the matching `End` tag for `target`.
/// Correctly handles arbitrarily nested content.
fn skip_to_end(reader: &mut Reader<&[u8]>, buf: &mut Vec<u8>) {
    // The depth says how far inside the element we are. Every start tag
    // goes one deeper and every end tag comes back out.
    //
    // The old version only counted the end tags that shared the
    // target's name, so a single ordinary element inside the skipped
    // subtree pushed the counter up and it never came back down. The
    // skip then ran to the end of the document and swallowed it.
    //
    // That is not a theoretical problem. Every real Ableton Live set
    // has a `<DeviceChain>` with ordinary elements inside it, so every
    // real set parsed as one track with every element in the file
    // listed as a device — and its tempo was the per-track tempo
    // rather than the project's.
    let mut depth = 1usize;
    loop {
        match reader.read_event_into(buf) {
            Ok(Event::Start(_)) => { depth += 1; }
            Ok(Event::End(e)) => {
                if depth == 1 {
                    // The element we were skipping is closing. If it is
                    // not the one we were looking for, the document is
                    // not what we thought it was; stop here rather than
                    // swallow the rest of it.
                    let _ = bytes_end_name(&e);
                    break;
                }
                depth -= 1;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}
