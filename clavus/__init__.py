"""
Clavus — .als parser supporting Ableton 10+ (Ableton-wrapped) and Live 9 (LiveSet-root) formats.

Ableton Live `.als` files are gzip-compressed XML.

Format detection is automatic based on root element.
"""
import xml.etree.ElementTree as ET
from pathlib import Path

# Keep upward compatibility — re-export everything from the base parser
from clavus.parser import (
    Device, Track, Marker, TempoEvent, Project,
    project_summary,
)

__all__ = [
    "Device", "Track", "Marker", "TempoEvent", "Project",
    "parse_als", "project_summary",
]


def parse_als(file_path: str) -> Project:
    """
    Parse an Ableton Live Set (.als) file.
    
    Auto-detects format: Ableton 10+ (<Ableton> wrapper) or Live 9 (<LiveSet> root).

    Args:
        file_path: Path to the .als file

    Returns:
        A Project dataclass
    """
    from pathlib import Path
    import gzip
    import xml.etree.ElementTree as ET

    path = Path(file_path)
    if not path.exists():
        raise FileNotFoundError(f".als file not found: {path}")

    with gzip.open(path, "rb") as f:
        raw = f.read()

    try:
        root = ET.fromstring(raw)
    except ET.ParseError as e:
        raise ValueError(f"Failed to parse XML from {path}: {e}")

    if root.tag == "Ableton":
        return _parse_ableton(root, path)
    elif root.tag == "LiveSet":
        # Old format — reuse existing parser (imported)
        import importlib
        old = importlib.import_module("clavus.parser")
        return old.parse_als(file_path)
    else:
        raise ValueError(f"Unknown root element <{root.tag}> — expected <Ableton> or <LiveSet>")


def _parse_ableton(root: ET.Element, path: Path) -> Project:
    """Parse an <Ableton>-wrapped .als file (Ableton 10+)."""
    from clavus.parser import Project, Track, Marker, Device, TempoEvent

    project = Project(file_path=path)

    # Get Ableton version info
    project.ableton_version = root.get("Creator", "Unknown")
    project.schema_version = f"{root.get('MajorVersion', '?')}.{root.get('MinorVersion', '?')}"

    live_set = root.find("LiveSet")
    if live_set is None:
        raise ValueError("No <LiveSet> inside <Ableton> wrapper")

    # OverwriteProtectionNumber = session identifier
    project.session_id = _get_text_value(live_set, "OverwriteProtectionNumber")

    # Tracks
    tracks_elem = live_set.find("Tracks")
    if tracks_elem is not None:
        idx = 0
        for child in tracks_elem:
            if child.tag in ("AudioTrack", "MidiTrack", "GroupTrack"):
                track = _parse_track_v2(child)
                track.index = idx
                idx += 1

                if child.tag == "AudioTrack":
                    track.track_type = "Audio"
                elif child.tag == "MidiTrack":
                    track.track_type = "MIDI"
                elif child.tag == "GroupTrack":
                    track.track_type = "Group"

                project.tracks.append(track)

    # Return tracks
    for rt_elem in live_set.findall("ReturnTrack"):
        track = _parse_track_v2(rt_elem)
        track.track_type = "Return"
        track.index = len(project.return_tracks)
        project.return_tracks.append(track)

    # Master track
    master_elem = live_set.find("MasterTrack")
    if master_elem is not None:
        project.master_track = _parse_track_v2(master_elem)
        project.master_track.track_type = "Master"
        project.master_track.name = "Master"

        # Tempo is inside MasterTrack > DeviceChain(outer) > Mixer > Tempo > Manual
        master_chain = master_elem.find("DeviceChain")
        if master_chain is not None:
            mixer = master_chain.find("Mixer")
            if mixer is not None:
                tempo_elem = mixer.find("Tempo")
                if tempo_elem is not None:
                    manual = tempo_elem.find("Manual")
                    if manual is not None and manual.get("Value"):
                        try:
                            project.bpm = float(manual.get("Value"))
                        except ValueError:
                            pass
                
                ts_elem = mixer.find("TimeSignature")
                if ts_elem is not None:
                    manual = ts_elem.find("Manual")
                    if manual is not None and manual.get("Value"):
                        try:
                            ts_val = int(manual.get("Value"))
                            # Ableton stores time sig as a 0-255 value.
                            # Values >= 128 mean 4/4 (common), < 128 is beats/4.
                            if ts_val >= 128:
                                project.time_signature = "4/4"
                            else:
                                sig_map = {3: "3/4", 6: "6/8", 5: "5/4", 7: "7/8", 2: "2/4"}
                                project.time_signature = sig_map.get(ts_val, f"{ts_val}/4")
                        except ValueError:
                            pass

    # Markers
    cue_points = live_set.find("CuePoints")
    if cue_points is not None:
        for cue in cue_points.findall("CuePoint"):
            time = _get_text_value(cue, "Time")
            name = _get_text_value(cue, "Name")
            if not name:
                name = f"Marker {len(project.markers) + 1}"
            project.markers.append(Marker(time=time or "0", name=name))

    # Locators (fallback)
    locators = live_set.find("Locators")
    if locators is not None:
        existing = {m.name for m in project.markers}
        for loc in locators.findall("Locator"):
            time = _get_text_value(loc, "Time")
            name = _get_text_value(loc, "Name")
            if name and name not in existing:
                project.markers.append(Marker(time=time or "0", name=name))
                existing.add(name)

    return project


def _parse_track_v2(elem: ET.Element) -> Track:
    """Parse a track element from the Ableton 10+ format."""
    from clavus.parser import Track, Device

    track = Track()

    # Name: use EffectiveName, fall back to UserName
    name_elem = elem.find("Name")
    if name_elem is not None:
        effective = name_elem.find("EffectiveName")
        if effective is not None:
            track.name = effective.get("Value", "Unnamed")
        else:
            user = name_elem.find("UserName")
            if user is not None and user.get("Value"):
                track.name = user.get("Value")

    # Color via ColorIndex (Ableton 10+ palette)
    color_index = _get_text_value(elem, "ColorIndex")
    if color_index:
        # ColorIndex maps to an Ableton palette, not a raw color value
        # Store it as negative for now to distinguish from direct colors
        try:
            track.color = int(color_index)
        except ValueError:
            pass

    # Also check for raw Color value (some versions)
    color_val = _get_text_value(elem, "Color")
    if color_val:
        try:
            track.color = int(color_val)
        except ValueError:
            pass

    track.is_frozen = (_get_text_value(elem, "Freeze") or "false").lower() == "true"

    # Mute/Solo
    mute = elem.find("Mute")
    if mute is not None:
        track.is_muted = mute.get("Value", "false").lower() == "true"

    solo = elem.find("Solo")
    if solo is not None:
        track.is_solo = solo.get("Value", "false").lower() == "true"

    # Devices — in Ableton 10, there's a nested DeviceChain inside the outer one
    chain = elem.find("DeviceChain")
    if chain is not None:
        # Try the inner DeviceChain first (Ableton 10+)
        inner_chain = chain.find("DeviceChain")
        if inner_chain is not None:
            devices = inner_chain.find("Devices")
        else:
            devices = chain.find("Devices")
        
        if devices is not None:
            for child in devices:
                name = child.find("Name")
                if name is not None and name.get("Value"):
                    dev_name = name.get("Value")
                else:
                    dev_name = child.tag
                track.devices.append(Device(name=dev_name, device_type=child.tag))

    return track


def _get_text_value(parent: ET.Element, tag: str) -> str:
    """Get the 'Value' attribute of a child element, or empty string."""
    child = parent.find(tag)
    if child is None:
        return ""
    return child.get("Value", "")
