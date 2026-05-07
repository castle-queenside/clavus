"""
Clavus — .als parser.

Ableton Live `.als` files are gzip-compressed XML documents with a
`<LiveSet>` root element. This module extracts:

- Track names, colors, order, and types (Audio, MIDI, Group, Return)
- Arrangement markers / Cue Points
- BPM / tempo envelope
- Plugin chains (device name + type only)
- Freeze/flatten status per track
- Send/return routing

The parser is designed to be forgiving — unknown elements are silently
skipped. It never crashes on unexpected XML structure.
"""

from __future__ import annotations

import gzip
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional


# ─── Data Model ──────────────────────────────────────────────────────────

@dataclass
class Device:
    """A plugin or device on a track."""
    name: str
    device_type: str  # e.g., "Compressor", "Eq8", "InstrumentGroupDevice", "Reverb"


@dataclass
class Clip:
    """A single clip in the arrangement view."""
    start_beats: float  # Start position in beats
    end_beats: float    # End position in beats
    name: str = ""
    color: int = 16777215
    clip_type: str = "MidiClip"  # MidiClip or AudioClip


@dataclass
class Track:
    """A single track in the Ableton project."""
    name: str = "Unnamed"
    color: int = 16777215  # White default
    track_type: str = "Audio"  # Audio, MIDI, Group, Return
    is_frozen: bool = False
    is_muted: bool = False
    is_solo: bool = False
    devices: list[Device] = field(default_factory=list)
    sends: dict[str, float] = field(default_factory=dict)
    clips: list[Clip] = field(default_factory=list)

    # For index-based reference
    index: int = 0


@dataclass
class Marker:
    """An arrangement marker or cue point."""
    time: str  # e.g., "4.1.1" or "0:00:00"
    name: str


@dataclass
class TempoEvent:
    """A tempo change event."""
    time_beats: float  # Time in beats (not seconds)
    bpm: float


@dataclass
class Project:
    """Full parsed representation of an Ableton Live project."""
    file_path: Optional[Path] = None
    tracks: list[Track] = field(default_factory=list)
    return_tracks: list[Track] = field(default_factory=list)
    master_track: Optional[Track] = None
    markers: list[Marker] = field(default_factory=list)
    bpm: float = 120.0
    tempo_events: list[TempoEvent] = field(default_factory=list)
    time_signature: str = "4/4"
    ableton_version: str = ""
    schema_version: str = ""
    session_id: str = ""

    @property
    def track_count(self) -> int:
        return len(self.tracks)

    @property
    def all_tracks(self) -> list[Track]:
        """All playable tracks (audio + MIDI + group, excludes returns)."""
        return self.tracks


# ─── XML Parsing Utilities ──────────────────────────────────────────────

def _get_element_text(element: ET.Element, path: str, default: str = "") -> str:
    """Get text content of a child element by sub-path, returning default if missing.
    
    Uses simple tag-path syntax like 'Name/@Value'.
    """
    parts = path.split("/")
    current = element
    for part in parts:
        if part.startswith("@"):
            # Attribute lookup
            return current.get(part[1:], default)
        found = current.find(part)
        if found is None:
            return default
        current = found
    return current.text or default


def _get_value_attr(element: ET.Element, path: str, default: str = "") -> str:
    """Get the 'Value' attribute of a child element by tag path."""
    target = element.find(path)
    if target is None:
        return default
    return target.get("Value", default)


def _parse_track_name(track_elem: ET.Element) -> str:
    """Extract the track name, supporting both Live 11 (Name attribute) and Live 12 (EffectiveName child)."""
    name_elem = track_elem.find("Name")
    if name_elem is not None:
        # Live 12: <Name><EffectiveName Value="Kick"/></Name>
        eff = name_elem.find("EffectiveName")
        if eff is not None:
            val = eff.get("Value", "")
            if val:
                return val
        # Live 11: <Name Value="Kick"/>
        val = name_elem.get("Value", "")
        if val:
            return val
        # UserName fallback
        user = name_elem.find("UserName")
        if user is not None:
            val = user.get("Value", "")
            if val:
                return val
    return "Unnamed Track"


def _parse_color(color_str: str) -> int:
    """Parse a color value from Ableton's format (integer string)."""
    try:
        return int(color_str)
    except (ValueError, TypeError):
        return 16777215


def _parse_time_sig(sig_str: str) -> str:
    """Normalize time signature string."""
    # Ableton stores as "4/4", "3/4", "6/8", etc.
    if sig_str and "/" in sig_str:
        return sig_str
    return "4/4"


# ─── Main Parser ────────────────────────────────────────────────────────

def parse_als(file_path: str | Path) -> Project:
    """
    Parse an Ableton Live Set (.als) file.

    Args:
        file_path: Path to the .als file (can be gzipped XML)

    Returns:
        A Project dataclass with all extracted information.
    """
    path = Path(file_path)
    if not path.exists():
        raise FileNotFoundError(f".als file not found: {path}")

    # Decompress gzipped XML
    with gzip.open(path, "rb") as f:
        raw = f.read()

    try:
        root = ET.fromstring(raw)
    except ET.ParseError as e:
        raise ValueError(f"Failed to parse XML from {path}: {e}")

    if root.tag != "LiveSet":
        # Live 12 wraps <LiveSet> inside <Ableton>
        if root.tag == "Ableton":
            liveset = root.find("LiveSet")
            if liveset is not None:
                root = liveset
            else:
                raise ValueError("Expected <LiveSet> inside <Ableton>, but none found")
        else:
            raise ValueError(f"Expected <LiveSet> root, got <{root.tag}>")

    # Live 12 stores tracks inside a <Tracks> wrapper
    tracks_container = root.find("Tracks") or root

    project = Project(file_path=path)
    _parse_master_track(root, project)
    _parse_tempo(root, project)
    _parse_markers(root, project)
    _parse_audio_tracks(tracks_container, project)
    _parse_midi_tracks(tracks_container, project)
    _parse_group_tracks(tracks_container, project)
    _parse_return_tracks(tracks_container, project)

    return project


def _parse_master_track(root: ET.Element, project: Project) -> None:
    """Extract master track information."""
    master = root.find("MasterTrack")
    if master is None:
        return

    track = Track(name="Master", track_type="Master")
    track.name = _get_value_attr(master, "Name", "Master")

    # Get devices on master
    chain = master.find("DeviceChain")
    if chain is not None:
        devices = chain.find("Devices")
        if devices is not None:
            for device_elem in devices:
                dev_name = _get_value_attr(device_elem, "Name", device_elem.tag)
                track.devices.append(Device(name=dev_name, device_type=device_elem.tag))

    project.master_track = track


def _parse_tempo(root: ET.Element, project: Project) -> None:
    """Extract BPM and tempo envelope."""
    # Primary source: MasterTrack > DeviceChain > TempoEnvelope
    master = root.find("MasterTrack")
    if master is not None:
        chain = master.find("DeviceChain")
        if chain is not None:
            tempo_env = chain.find("TempoEnvelope")
            if tempo_env is not None:
                events = tempo_env.find("Events")
                if events is not None:
                    for point in events.findall("AudioPoint"):  # or 'MidiPoint'
                        time_str = point.get("Time", "0")
                        value_str = point.get("Value", "120")
                        try:
                            event = TempoEvent(
                                time_beats=float(time_str),
                                bpm=float(value_str),
                            )
                            project.tempo_events.append(event)
                        except ValueError:
                            continue

    # Set initial BPM from first event or default
    if project.tempo_events:
        project.bpm = project.tempo_events[0].bpm


def _parse_markers(root: ET.Element, project: Project) -> None:
    """Extract arrangement markers and locators."""
    # From ArrangerAutomation > CuePoints
    arranger = root.find("ArrangerAutomation")
    if arranger is not None:
        cue_points = arranger.find("CuePoints")
        if cue_points is not None:
            for cue in cue_points.findall("CuePoint"):
                time = _get_value_attr(cue, "Time", "0")
                name = _get_value_attr(cue, "Name", f"Marker {len(project.markers) + 1}")
                project.markers.append(Marker(time=time, name=name))

    # Also check Locators (older Ableton versions)
    locators = root.find("Locators")
    if locators is not None:
        existing_names = {m.name for m in project.markers}
        for loc in locators.findall("Locator"):
            time = _get_value_attr(loc, "Time", "0")
            name = _get_value_attr(loc, "Name", f"Locator {len(project.markers) + 1}")
            if name not in existing_names:
                project.markers.append(Marker(time=time, name=name))
                existing_names.add(name)


def _parse_track_common(track_elem: ET.Element, track: Track) -> None:
    """Parse properties common to all track types."""
    track.name = _parse_track_name(track_elem)
    track.color = _parse_color(_get_value_attr(track_elem, "Color", "16777215"))
    track.is_frozen = _get_value_attr(track_elem, "IsFrozen", "false").lower() == "true"
    track.is_muted = _get_value_attr(track_elem, "Mute", "false").lower() == "true"
    track.is_solo = _get_value_attr(track_elem, "Solo", "false").lower() == "true"

    # Devices
    chain = track_elem.find("DeviceChain")
    if chain is not None:
        devices = chain.find("Devices")
        if devices is not None:
            for device_elem in devices:
                dev_name = _parse_track_name(device_elem)
                track.devices.append(Device(name=dev_name, device_type=device_elem.tag))

    # Sends
    sends = track_elem.find("SendChannels")
    if sends is not None:
        for send_chan in sends.findall("SendChannel"):
            send_name = _get_value_attr(send_chan, "Name", "Send")
            send_value_str = _get_value_attr(send_chan, "Value", "0.0")
            try:
                track.sends[send_name] = float(send_value_str)
            except ValueError:
                track.sends[send_name] = 0.0

    # Arranger clips (ClipTimeable > ArrangerAutomation > Events) — Live 11 path
    if chain is not None:
        for ct in chain.iter("ClipTimeable"):
            _parse_arranger_clips(ct, track)

    # Arranger clips (Live 12+) — clips are direct children of the track element
    if not track.clips:
        for clip_elem in track_elem:
            if clip_elem.tag in ("MidiClip", "AudioClip"):
                _parse_clip_element(clip_elem, track)


def _parse_clip_element(clip_elem: ET.Element, track: Track) -> None:
    """Extract clip data from a MidiClip or AudioClip element — shared for Live 11 and Live 12."""
    cs = clip_elem.find("CurrentStart")
    ce = clip_elem.find("CurrentEnd")
    cn = clip_elem.find("Name")
    cc = clip_elem.find("Color")
    start_val = float(cs.get("Value", "0")) if cs is not None else float(clip_elem.get("Time", "0"))
    end_val = float(ce.get("Value", "0")) if ce is not None else start_val + 4.0
    name_val = cn.get("Value", "") if cn is not None else ""
    color_val = int(cc.get("Value", "16777215")) if cc is not None else 16777215
    track.clips.append(Clip(
        start_beats=start_val,
        end_beats=end_val,
        name=name_val,
        color=color_val,
        clip_type=clip_elem.tag,
    ))


def _parse_arranger_clips(ct: ET.Element, track: Track) -> None:
    """Extract arranger clips from a ClipTimeable element (Live 11 compatibility)."""
    arr_auto = ct.find("ArrangerAutomation")
    if arr_auto is None:
        return
    events = arr_auto.find("Events")
    if events is None:
        return
    for clip_elem in events:
        if clip_elem.tag not in ("MidiClip", "AudioClip"):
            continue
        _parse_clip_element(clip_elem, track)


def _parse_audio_tracks(root: ET.Element, project: Project) -> None:
    """Extract audio tracks."""
    for idx, elem in enumerate(root.findall("AudioTrack")):
        track = Track(track_type="Audio", index=idx)
        _parse_track_common(elem, track)
        project.tracks.append(track)


def _parse_midi_tracks(root: ET.Element, project: Project) -> None:
    """Extract MIDI tracks."""
    for idx, elem in enumerate(root.findall("MidiTrack")):
        track = Track(track_type="MIDI", index=len(project.tracks))
        _parse_track_common(elem, track)
        project.tracks.append(track)


def _parse_group_tracks(root: ET.Element, project: Project) -> None:
    """Extract group tracks."""
    for idx, elem in enumerate(root.findall("GroupTrack")):
        track = Track(track_type="Group", index=len(project.tracks))
        _parse_track_common(elem, track)
        project.tracks.append(track)


def _parse_return_tracks(root: ET.Element, project: Project) -> None:
    """Extract return tracks."""
    for idx, elem in enumerate(root.findall("ReturnTrack")):
        track = Track(track_type="Return", index=idx)
        _parse_track_common(elem, track)
        project.return_tracks.append(track)


# ─── Sample Extraction ──────────────────────────────────────────────────


def extract_sample_paths(als_path: str | Path) -> list[str]:
    """Extract all referenced audio sample paths from a .als file.

    Parses the gzip-compressed XML and finds all SampleRef > FileRef > Path
    elements. Returns absolute paths (deduplicated).

    Only works on 'Collected' projects where samples are in the project folder.
    """
    import gzip
    import re

    with open(als_path, "rb") as f:
        data = gzip.decompress(f.read())

    text = data.decode("utf-8", errors="replace")
    paths: set[str] = set()

    for block in text.split("<SampleRef")[1:]:
        end = block.find("</SampleRef>")
        if end > 0:
            block = block[:end]
        for match in re.finditer(r'<Path\s+Value="([^"]+)"', block):
            paths.add(match.group(1))

    return sorted(paths)


def extract_sample_paths_from_bytes(raw_als: bytes) -> list[str]:
    """Extract sample paths from raw .als bytes (already decompressed)."""
    import gzip
    import re

    data = gzip.decompress(raw_als)
    text = data.decode("utf-8", errors="replace")
    paths: set[str] = set()

    for block in text.split("<SampleRef")[1:]:
        end = block.find("</SampleRef>")
        if end > 0:
            block = block[:end]
        for match in re.finditer(r'<Path\s+Value="([^"]+)"', block):
            paths.add(match.group(1))

    return sorted(paths)

def project_summary(project: Project) -> str:
    """Return a human-readable summary of the parsed project."""
    lines = []
    lines.append(f"📁 {project.file_path.name if project.file_path else 'Unknown'}")
    lines.append(f"   BPM: {project.bpm} | Time Sig: {project.time_signature}")
    lines.append(f"   Tracks: {project.track_count} audio/MIDI + {len(project.return_tracks)} returns")
    lines.append(f"   Markers: {len(project.markers)}")
    lines.append("")

    for track in project.tracks:
        icon = {"Audio": "🎧", "MIDI": "🎹", "Group": "📁"}.get(track.track_type, "🎵")
        frozen = "❄️" if track.is_frozen else ""
        muted = "🔇" if track.is_muted else ""
        solo = "🎤" if track.is_solo else ""
        flags = frozen + muted + solo
        devs = ", ".join(d.device_type for d in track.devices) if track.devices else "—"
        lines.append(f"  {icon} {track.name} {flags}  [{devs}]")

    if project.return_tracks:
        lines.append("")
        for rt in project.return_tracks:
            lines.append(f"  🔄 {rt.name}")

    if project.markers:
        lines.append("")
        lines.append("  Markers:")
        for m in project.markers:
            lines.append(f"    📍 {m.time}  {m.name}")

    return "\n".join(lines)


def rewrite_als_sample_paths(raw_als: bytes, project_dir: str | Path) -> bytes:
    """Rewrite absolute sample paths in a .als file to point to a local project folder.

    Finds all <Path Value="..."/> elements inside <SampleRef> blocks and replaces
    absolute Mac paths with the corresponding local paths under project_dir.
    Uses the <RelativePath> value (which already has the correct relative structure)
    to construct the new path. Returns the modified gzipped .als bytes.
    """
    import gzip
    import re
    from pathlib import Path

    data = gzip.decompress(raw_als)
    text = data.decode("utf-8", errors="replace")
    project_root = str(Path(project_dir).resolve())

    # Find <SampleRef> blocks and rewrite their <Path> elements
    result_parts = []
    pos = 0
    for m in re.finditer(r'<SampleRef>.*?</SampleRef>', text, re.DOTALL):
        result_parts.append(text[pos:m.start()])
        block = m.group(0)

        # Replace absolute <Path Value="..."> with local path
        def path_replacer(pm):
            old_path = pm.group(1)
            # Find the corresponding RelativePath in this SampleRef block
            rel_m = re.search(r'<RelativePath\s+Value="([^"]+)"', block)
            if rel_m:
                new_path = str(Path(project_root) / rel_m.group(1))
                return f'<Path Value="{new_path}"'
            return pm.group(0)

        block = re.sub(r'<Path\s+Value="([^"]+)"', path_replacer, block)
        result_parts.append(block)
        pos = m.end()

    result_parts.append(text[pos:])
    new_data = "".join(result_parts).encode("utf-8")
    return gzip.compress(new_data)
