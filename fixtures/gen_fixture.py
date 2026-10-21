"""Generate a synthetic Ableton .als file for testing."""
import gzip, xml.etree.ElementTree as ET
from pathlib import Path

FIXTURE_DIR = Path(__file__).parent

def make_als(tracks=8, markers=None, bpm=128, filename="test_project.als"):
    """Generate a realistic .als file with the given spec."""
    if markers is None:
        markers = [
            ("4.1.1", "Intro"),
            ("17.1.1", "Verse"),
            ("33.1.1", "Drop"),
            ("49.1.1", "Break"),
            ("65.1.1", "Outro"),
        ]

    root = ET.Element("LiveSet")

    # --- Tempo (top-level, what the parser reads) ---
    ET.SubElement(root, "Tempo").set("Value", str(bpm))

    # --- Master track with tempo envelope ---
    master = ET.SubElement(root, "MasterTrack")
    ET.SubElement(master, "Name").set("Value", "Master")
    device_chain = ET.SubElement(master, "DeviceChain")
    tempo_env = ET.SubElement(device_chain, "TempoEnvelope")
    tempo_events = ET.SubElement(tempo_env, "Events")
    tempo_event = ET.SubElement(tempo_events, "AudioPoint")
    tempo_event.set("Time", "0.0")
    tempo_event.set("Value", str(bpm))

    # --- Arrangement markers ---
    arranger = ET.SubElement(root, "ArrangerAutomation")
    cue_points = ET.SubElement(arranger, "CuePoints")
    for i, (time, name) in enumerate(markers):
        cue = ET.SubElement(cue_points, "CuePoint")
        ET.SubElement(cue, "Time").set("Value", time)
        ET.SubElement(cue, "Name").set("Value", name)

    # --- Tracks ---
    track_names = [
        "Kick", "Snare", "Hi-Hat", "Bass",
        "Synth Pad", "Lead", "FX", "Vocal"
    ]
    colors = [
        "13046339", "10790111", "8438015", "3001499",
        "7012595", "16777200", "16711680", "16119285"
    ]

    for i in range(tracks):
        name = track_names[i] if i < len(track_names) else f"Track {i+1}"
        color = colors[i] if i < len(colors) else "16777200"

        track = ET.SubElement(root, "AudioTrack")
        ET.SubElement(track, "Name").set("Value", name)
        ET.SubElement(track, "Color").set("Value", color)

        # Track state
        ET.SubElement(track, "IsFrozen").set("Value", "false")
        ET.SubElement(track, "Mute").set("Value", "false")
        ET.SubElement(track, "Solo").set("Value", "false")

        # Device chain with a plugin
        chain = ET.SubElement(track, "DeviceChain")
        devices = ET.SubElement(chain, "Devices")
        if i in (0, 1, 3):  # Kick, Snare, Bass get a compressor
            comp = ET.SubElement(devices, "Compressor")
            ET.SubElement(comp, "Name").set("Value", "Glue Compressor")
        if i == 2:  # Hi-hat has EQ
            eq = ET.SubElement(devices, "Eq8")
            ET.SubElement(eq, "Name").set("Value", "EQ Eight")
        if i == 4:  # Synth has a instrument
            instr = ET.SubElement(devices, "InstrumentGroupDevice")
            ET.SubElement(instr, "Name").set("Value", "Serum")
        if i == 5:  # Lead has reverb
            verb = ET.SubElement(devices, "Reverb")
            ET.SubElement(verb, "Name").set("Value", "Hall Reverb")

        # Send channels
        sends = ET.SubElement(track, "SendChannels")
        send_a = ET.SubElement(sends, "SendChannel")
        ET.SubElement(send_a, "Name").set("Value", "Reverb")
        send_b = ET.SubElement(sends, "SendChannel")
        ET.SubElement(send_b, "Name").set("Value", "Delay")

    # --- Return tracks ---
    return_names = ["Reverb", "Delay"]
    for name in return_names:
        rt = ET.SubElement(root, "ReturnTrack")
        ET.SubElement(rt, "Name").set("Value", name)

    # --- Locators (old format, some projects use both) ---
    locators = ET.SubElement(root, "Locators")
    for i, (time, name) in enumerate(markers):
        loc = ET.SubElement(locators, "Locator")
        ET.SubElement(loc, "Time").set("Value", time)
        ET.SubElement(loc, "Name").set("Value", name)

    # Write gzipped XML
    xml_str = b'<?xml version="1.0" encoding="UTF-8"?>\n' + ET.tostring(root, encoding="utf-8")
    out_path = FIXTURE_DIR / filename
    with gzip.open(out_path, "wb") as f:
        f.write(xml_str)

    print(f"✅ Generated {out_path} ({tracks} tracks, {len(markers)} markers, {bpm}bpm)")
    return out_path

def make_real_live_als(tracks=3, markers=None, bpm=128, filename="real_live_project.als"):
    """Generate an .als that mirrors REAL Ableton Live 11/12 output.

    Key differences from the legacy fixture (and from what naive parsers assume):
      - Tracks are <Track> elements inside <Tracks> (not <AudioTrack>)
      - Track type comes from <MidiEffectChain> vs <AudioEffectChain>
      - <Name Value="..."/> is self-closing (an "empty" XML event)
      - Every track has <Mixer><Track>...</Track></Mixer> (nested same-name tag)
      - Returns live in <ReturnTracks> as <ReturnTrack>
      - BPM comes from MasterTrack > DeviceChain > TempoEnvelope
      - Markers live in <ArrangerAutomation><CuePoints><CuePoint>
    """
    if markers is None:
        markers = [("1.1.1", "Intro"), ("17.1.1", "Verse"), ("33.1.1", "Drop")]

    root = ET.Element("LiveSet")
    root.set("MajorVersion", "12")
    root.set("MinorVersion", "0")

    # --- Master track with tempo envelope (canonical BPM location) ---
    master = ET.SubElement(root, "MasterTrack")
    ET.SubElement(master, "Name").set("Value", "Master")
    master_chain = ET.SubElement(master, "DeviceChain")
    master_audio = ET.SubElement(master_chain, "AudioEffectChain")
    master_effect = ET.SubElement(master_audio, "AudioEffect")
    ET.SubElement(master_effect, "Compressor").set("Value", "Master Comp")
    tempo_env = ET.SubElement(master_chain, "TempoEnvelope")
    tempo_events = ET.SubElement(tempo_env, "Events")
    ap = ET.SubElement(tempo_events, "AudioPoint")
    ap.set("Time", "0.0")
    ap.set("Value", str(bpm))

    # --- Arrangement markers (CuePoint shape) ---
    arranger = ET.SubElement(root, "ArrangerAutomation")
    cue_points = ET.SubElement(arranger, "CuePoints")
    for time, name in markers:
        cue = ET.SubElement(cue_points, "CuePoint")
        ET.SubElement(cue, "Time").set("Value", time)
        ET.SubElement(cue, "Name").set("Value", name)

    # --- Tracks: <Tracks> > <Track> ---
    tracks_el = ET.SubElement(root, "Tracks")
    specs = [
        ("Drums", "midi", "Glue Comp", True),
        ("Vocal", "audio", "Hall Reverb", False),
        ("Audio 1", "audio", None, False),
    ]
    for i, (name, kind, device, muted) in enumerate(specs):
        if i >= tracks:
            break
        track = ET.SubElement(tracks_el, "Track")
        chain = ET.SubElement(track, "DeviceChain")
        if kind == "midi":
            eff_chain = ET.SubElement(chain, "MidiEffectChain")
        else:
            eff_chain = ET.SubElement(chain, "AudioEffectChain")
        if device:
            eff = ET.SubElement(eff_chain, "MidiEffect" if kind == "midi" else "AudioEffect")
            comp = ET.SubElement(eff, "Compressor" if kind == "midi" else "Reverb")
            ET.SubElement(comp, "Name").set("Value", device)

        # <Mixer> contains a nested <Track> — the classic parser trap.
        mixer = ET.SubElement(track, "Mixer")
        mixer_track = ET.SubElement(mixer, "Track")
        vol = ET.SubElement(mixer_track, "Volume")
        ET.SubElement(vol, "Value").set("Value", str(0.8 - i * 0.1))

        ET.SubElement(track, "Mute").set("Value", "true" if muted else "false")
        ET.SubElement(track, "Solo").set("Value", "false")
        # Name comes AFTER DeviceChain/Mixer in real Live output.
        ET.SubElement(track, "Name").set("Value", name)

    # --- Return tracks: <ReturnTracks> > <ReturnTrack> ---
    returns_el = ET.SubElement(root, "ReturnTracks")
    for rname in ("A Reverb", "A Delay"):
        rt = ET.SubElement(returns_el, "ReturnTrack")
        rchain = ET.SubElement(rt, "DeviceChain")
        reff = ET.SubElement(rchain, "AudioEffectChain")
        reff2 = ET.SubElement(reff, "AudioEffect")
        ET.SubElement(reff2, "Reverb").set("Value", rname)
        ET.SubElement(rt, "Name").set("Value", rname)

    # --- Locators (older shape) — same markers, must be de-duplicated ---
    locators = ET.SubElement(root, "Locators")
    for time, name in markers:
        loc = ET.SubElement(locators, "Locator")
        ET.SubElement(loc, "Time").set("Value", time)
        ET.SubElement(loc, "Name").set("Value", name)

    xml_str = b'<?xml version="1.0" encoding="UTF-8"?>\n' + ET.tostring(root, encoding="utf-8")
    out_path = FIXTURE_DIR / filename
    with gzip.open(out_path, "wb") as f:
        f.write(xml_str)

    print(f"✅ Generated {out_path} (real Live format: {tracks} tracks, {len(markers)} markers, {bpm}bpm)")
    return out_path


if __name__ == "__main__":
    make_als()
    make_real_live_als()
