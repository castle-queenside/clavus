# Bitwig support — design (DRAFT)

> Living document. Updated as the refactor proceeds. Goal: add first-class
> Bitwig Studio support to Clavus without breaking the existing Ableton path.

## TL;DR

The current parser is a single file (`clavus/parser.py`) that does all
Ableton-specific work. Add a parallel `clavus/parser_bitwig.py` that
implements the same `Project`/`Track`/`Marker`/`Device`/`TempoEvent`
interface, then add a tiny factory in `clavus/__init__.py` to pick
the right one. The rest of Clavus already uses the data model
opaquely — the surface area for the refactor is small.

## Why a parallel module, not a class hierarchy

`clavus/parser.py` is 533 lines of straight-line parsing code with no
class hierarchy. Trying to factor a base class out of it would be a
big diff with no behavioral gain. Easier to:
- Leave `parser.py` as-is (Ableton-only).
- Add `parser_bitwig.py` that exposes the **same function names**
  with the **same signatures** and **same return types**.
- A factory function (`parse_project(path) -> Project`) dispatches
  by file extension.

If/when a third DAW lands, we revisit and refactor at that point.

## Data model reuse

The existing `Project`/`Track`/`Device`/`Marker`/`TempoEvent` data
classes are DAW-agnostic enough. We may need:
- A `daw_type: str` field on `Project` ("ableton" | "bitwig" | …)
- A `color` representation that survives both (Ableton uses int,
  Bitwig uses hex string — normalize to int with #RRGGBB layout)
- Clip times: store as **beats** internally; convert at the edges
  (Ableton reads `<CurrentTime>` floats; Bitwig stores
  `bars.beats.sixteenths` as a string)

## File detection

```python
EXT_MAP = {
    ".als": "ableton",
    ".bwproject": "bitwig",
}

def detect_daw(path: Path) -> str:
    return EXT_MAP.get(path.suffix.lower())
```

If unknown, fall back to "ableton" + a one-time warning (today's
behavior). Add `--daw` flag to `clavus init` for explicit override.

## Bitwig parser outline

```python
# clavus/parser_bitwig.py
import zipfile
import xml.etree.ElementTree as ET
from pathlib import Path
from .parser import Project, Track, Device, Clip, Marker, TempoEvent

def parse_bwproject(file_path: str | Path) -> Project:
    """Parse a Bitwig .bwproject file (zip) into a Project."""
    path = Path(file_path)
    proj = Project(file_path=path, daw_type="bitwig")

    with zipfile.ZipFile(path) as zf:
        # project.xml — main arrangement + tracks
        with zf.open("project.xml") as f:
            tree = ET.parse(f)
            root = tree.getroot()
            _parse_tracks(root, proj)
            _parse_markers(root, proj)
            _parse_tempo(root, proj)
            # devices are nested in tracks
            # clips are in scenes/arrangement

    return proj
```

## Cue round-trip

Ableton today:
- Clavus stores cue (`bar.beat.ticks`) in its own store.
- `:inject` rewrites the `.als` XML to add `<Marker>` elements.

Bitwig will:
- Clavus stores cue in its own store (same `Cue` model).
- New: `inject_bwproject_markers(file, markers) -> bytes` rewrites
  the `<Locator>` elements inside the zipped `project.xml`.
- On restore: parse the `.bwproject`, extract existing locators,
  diff, decide conflict.

## Stems

Ableton: `<SampleRef><FileRef>` lookup inside the .als XML, with
paths in `FileRef Path/RelativePath`/`Path/Path` elements.

Bitwig: the .bwproject zip can include the `samples/` folder
directly. Extraction is just `zf.extractall(target_dir)`.

## Risks

- **File size** — `.bwproject` with samples can be huge. Maybe a
  `--no-samples` flag to skip the samples subtree for snapshots.
- **Bitwig schema is undocumented** — we're reverse-engineering.
  Pin to a specific Bitwig version once parsing works and add
  version-detection that warns on unknown formats.
- **Cross-DAW collab** — Alice pushes an Ableton snapshot, Bob
  pulls it on Bitwig. The blob is opaque so it works for storage,
  but Bob can't *open* it in Bitwig. Out of scope for v1;
  document in README.
- **Marker coords** — different time bases; convert at edges.
- **CI matrix** — needs to run on Linux + macOS + Windows
  (Bitwig supports all three).

## Phases

See `~/Vault/pi/Projects/clavus/Bitwig support.md` for the
higher-level roadmap and status.
