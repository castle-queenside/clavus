#!/usr/bin/env python3
"""Make the two-machine relay fixture.

Three machines, one relay:

- **A** — the studio. Has the set and its audio, takes snapshots, adds
  a note, and pushes.
- **B** — a collaborator. Has *the same set* at a different path and a
  different depth, which is the normal case for someone working on the
  same project. Pulls, and restores.
- **C** — a collaborator whose audio never arrived. Has the set and
  nothing else. Pulls, and the audio has to come over the wire.

The bytes have to be *exactly* the same for a collaborator to be
recognised, so the compression is deterministic: gzip writes the time
it was made into its header, and two compressions of the same text are
not the same file.
"""

import gzip
import math
import pathlib
import struct
import sys


def gz(text: str) -> bytes:
    """Gzip with no timestamp, so the same text is the same bytes."""
    return gzip.compress(text.encode(), compresslevel=9, mtime=0)


def als_text(tag: str) -> str:
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tempo Value="128" />
    <Tracks>
      <AudioTrack>
        <Name Value="Drums {tag}" />
        <DeviceChain>
          <SampleRef><FileRef>
            <Path Value="Samples/loop.wav" />
            <Name Value="loop.wav" />
            <Type Value="1" />
          </FileRef></SampleRef>
        </DeviceChain>
      </AudioTrack>
    </Tracks>
  </LiveSet>
</Ableton>
"""


def wav(freq: float, seconds: float = 0.25, rate: int = 8000) -> bytes:
    n = int(rate * seconds)
    frames = b"".join(
        struct.pack("<h", int(12000 * math.sin(2 * math.pi * freq * i / rate)))
        for i in range(n)
    )
    return (
        b"RIFF"
        + struct.pack("<I", 36 + len(frames))
        + b"WAVEfmt "
        + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16)
        + b"data"
        + struct.pack("<I", len(frames))
        + frames
    )


def write(path: pathlib.Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "relaytest")
    root.mkdir(parents=True, exist_ok=True)

    v1 = gz(als_text("v1"))
    v2 = gz(als_text("v2"))
    audio = wav(440)

    # A — the studio.
    write(root / "machineA/Set/Set Project/Set.als", v1)
    write(root / "machineA/Set/Set Project/Samples/loop.wav", audio)

    # B — a collaborator, same set, different path and depth.
    write(root / "machineB/Some/Other/Place/Set.als", v1)
    write(root / "machineB/Some/Other/Place/Samples/loop.wav", audio)

    # C — a collaborator whose audio never arrived.
    write(root / "machineC/Work/Set/Set.als", v1)

    print("machineA v1 sha256:", __import__("hashlib").sha256(v1).hexdigest()[:16])
    print("machineB v1 sha256:", __import__("hashlib").sha256(v1).hexdigest()[:16])
    print("machineC v1 sha256:", __import__("hashlib").sha256(v1).hexdigest()[:16])
    print("fixture written to", root)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
