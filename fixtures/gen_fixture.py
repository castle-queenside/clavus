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

    # --- Master track with tempo ---
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

if __name__ == "__main__":
    make_als()
