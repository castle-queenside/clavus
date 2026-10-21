"""Generate a realistic Bitwig Studio .bwproject fixture for testing.

A .bwproject is a ZIP archive containing `project.xml` (the arrangement) plus
optionally a `samples/` tree. Bitwig's schema is reverse-engineered, so this
fixture mirrors the shapes the Clavus parser actually has to cope with:

  - <Project VersionMajor=...> root
  - <Tracks> containing <Track> elements with a <Name Value="..."/> child
  - <DeviceChain> containing <Device Name="..."/> children
  - <Locator Position="1.1.1" Name="Intro"/> markers
  - <Tempo Bpm="128.0"/>
  - Sample references as attribute values ending in audio extensions
  - A `samples/` folder inside the archive

Run:  python3 fixtures/gen_bw_fixture.py
"""

import zipfile
from pathlib import Path

FIXTURE_DIR = Path(__file__).parent


PROJECT_XML = """<?xml version="1.0" encoding="UTF-8"?>
<Project VersionMajor="5" VersionMinor="1" Name="{name}">
  <Arrangement>
    <Timeline>
      <Locators>
{locators}
      </Locators>
    </Timeline>
  </Arrangement>
  <Tracks>
{tracks}
  </Tracks>
  <Tempo Bpm="{bpm}"/>
  <AudioFiles>
{files}
  </AudioFiles>
</Project>
"""


def _track_xml(index, name, kind, devices, muted=False, solo=False, samples=()):
    dev_xml = "\n".join(
        f'        <Device Name="{d}" />' for d in devices
    )
    clip_xml = "\n".join(
        f'        <AudioClip File="{s}" />' for s in samples
    )
    return f"""    <Track Type="{kind}" Index="{index}">
      <Name Value="{name}" />
      <DeviceChain>
{dev_xml}
      </DeviceChain>
      <Mute Value="{'true' if muted else 'false'}" />
      <Solo Value="{'true' if solo else 'false'}" />
      <Volume Value="0.8" />
{clip_xml}
    </Track>"""


def make_bwproject(
    name="BitwigTestProject",
    bpm=128,
    filename="test_project.bwproject",
    with_samples=True,
):
    """Build a .bwproject ZIP and return its path."""
    locators = "\n".join(
        f'        <Locator Position="{pos}" Name="{label}" />'
        for pos, label in [("1.1.1", "Intro"), ("17.1.1", "Verse"), ("33.1.1", "Drop")]
    )

    specs = [
        ("Drums", "Audio", ["EQ Eight", "Compressor"], True, False,
         ["/Users/music/Library/Samples/kick.wav"]),
        ("Bass", "Audio", ["Saturation"], False, False,
         ["/Users/music/Library/Samples/bass.wav"]),
        ("Keys", "Midi", ["Velocity Compressor"], False, True, []),
        ("Vox", "Audio", ["Reverb"], False, False,
         ["/Users/music/Library/Samples/vox.wav"]),
    ]

    tracks = "\n".join(
        _track_xml(i, n, k, d, m, s, smp)
        for i, (n, k, d, m, s, smp) in enumerate(specs)
    )

    files = "\n".join(
        f'    <File Path="{p}" />'
        for p in [
            "/Users/music/Library/Samples/kick.wav",
            "/Users/music/Library/Samples/bass.wav",
            "/Users/music/Library/Samples/vox.wav",
        ]
    )

    project_xml = PROJECT_XML.format(
        name=name, locators=locators, tracks=tracks, bpm=bpm, files=files
    )

    out_path = FIXTURE_DIR / filename
    with zipfile.ZipFile(out_path, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("project.xml", project_xml)
        if with_samples:
            # A minimal but valid 16-bit mono WAV so the sample audit can
            # actually resolve the referenced files.
            zf.writestr("samples/kick.wav", _wav_bytes(440))
            zf.writestr("samples/bass.wav", _wav_bytes(220))
            zf.writestr("samples/vox.wav", _wav_bytes(330))

    print(f"✅ Generated {out_path} (4 tracks, 3 locators, {bpm}bpm)")
    return out_path


def _wav_bytes(freq=440, seconds=0.1, rate=44100):
    """Build a tiny valid mono 16-bit PCM WAV file."""
    import struct

    n = int(rate * seconds)
    frames = b"".join(
        struct.pack("<h", int(10000 * ((i % 100) / 100.0 - 0.5)))
        for i in range(n)
    )
    data_len = len(frames)
    header = (
        b"RIFF"
        + struct.pack("<I", 36 + data_len)
        + b"WAVEfmt "
        + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16)
        + b"data"
        + struct.pack("<I", data_len)
    )
    return header + frames


if __name__ == "__main__":
    make_bwproject()
    # A second fixture with no `samples/` folder, to exercise the
    # "samples not embedded" path.
    make_bwproject(
        name="NoSamples",
        filename="no_samples.bwproject",
        with_samples=False,
    )
