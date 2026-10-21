"""Differential test: Rust parser vs the Python reference implementation.

Runs both parsers over the same .als fixtures and reports any divergence in
track count, track names/types, BPM, return tracks, or markers.

The Rust implementation is the one being validated, so where the two
disagree the Python result is NOT automatically correct — the script just
flags the difference for a human to adjudicate.

Usage:
    python3 fixtures/diff_test.py            # run against all fixtures
    python3 fixtures/diff_test.py --verbose  # show full details
"""

import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).parent.parent
RUST = REPO / "rust"
PY_CLAVUS = REPO.parent / "clavus"  # sibling checkout with the Python implementation
CARGO = Path.home() / ".cargo" / "bin" / "cargo"

FIXTURES = [
    REPO / "fixtures" / "test_project.als",
    REPO / "fixtures" / "real_live_project.als",
]

# Differences that are known and expected, with the reason.
# Keyed by fixture name -> list of (substring, explanation).
KNOWN_DIFFS = {
    "real_live_project.als": [
        (
            "track_count",
            "Python only understands <AudioTrack>; real Live writes <Track>. "
            "The Rust parser handles both, so it is correct here.",
        ),
        (
            "return_tracks",
            "Python only understands <ReturnTrack> at the top level; real Live "
            "nests them in <ReturnTracks>. Rust handles both.",
        ),
    ],
}


def rust_parse(path: Path):
    """Run the Rust dump_json example and return its parsed output."""
    exe = RUST / "target" / "debug" / "examples" / "dump_json"
    if not exe.exists():
        subprocess.run(
            [str(CARGO), "build", "-p", "clavus-parser-als", "--example", "dump_json"],
            cwd=RUST,
            check=True,
            stdout=subprocess.DEVNULL,
        )
    out = subprocess.run(
        [str(exe), str(path)], capture_output=True, text=True, cwd=RUST
    )
    if out.returncode != 0:
        raise RuntimeError(f"rust parse failed: {out.stderr.strip()}")
    return json.loads(out.stdout)


def python_parse(path: Path):
    """Run the Python reference parser and normalize its output."""
    sys.path.insert(0, str(PY_CLAVUS))
    from clavus.parser import parse_als

    p = parse_als(path)
    return {
        "track_count": p.track_count,
        "bpm": p.bpm,
        "has_master": p.master_track is not None,
        "tracks": [
            {
                "name": t.name,
                "type": t.track_type,
                "muted": t.is_muted,
                "solo": t.is_solo,
                "frozen": t.is_frozen,
                "devices": [d.name for d in t.devices],
                "sends": sorted(t.sends.keys()),
            }
            for t in p.tracks
        ],
        "return_tracks": [
            {"name": t.name, "type": t.track_type} for t in p.return_tracks
        ],
        "markers": [{"time": m.time, "name": m.name} for m in p.markers],
    }


def compare(fixture: Path, verbose: bool = False):
    """Compare both parsers on one fixture. Returns a list of difference strings."""
    diffs = []

    rust = rust_parse(fixture)
    try:
        py = python_parse(fixture)
    except Exception as exc:  # noqa: BLE001
        return [f"python parser failed: {exc}"]

    if rust["track_count"] != py["track_count"]:
        diffs.append(
            f"track_count: rust={rust['track_count']} python={py['track_count']}"
        )

    if abs(rust["bpm"] - py["bpm"]) > 0.01:
        diffs.append(f"bpm: rust={rust['bpm']} python={py['bpm']}")

    if rust["has_master"] != py["has_master"]:
        diffs.append(
            f"has_master: rust={rust['has_master']} python={py['has_master']}"
        )

    # Track-by-track comparison, up to the shorter of the two lists.
    for i, (r, p) in enumerate(zip(rust["tracks"], py["tracks"])):
        for field in ("name", "type", "muted", "solo", "frozen"):
            if r[field] != p[field]:
                diffs.append(
                    f"track[{i}].{field}: rust={r[field]!r} python={p[field]!r}"
                )
        if sorted(r["devices"]) != sorted(p["devices"]):
            diffs.append(
                f"track[{i}].devices: rust={sorted(r['devices'])} "
                f"python={sorted(p['devices'])}"
            )
        if sorted(r["sends"]) != sorted(p["sends"]):
            diffs.append(
                f"track[{i}].sends: rust={sorted(r['sends'])} "
                f"python={sorted(p['sends'])}"
            )

    if len(rust["tracks"]) != len(py["tracks"]) and verbose:
        diffs.append(
            f"  rust track names: {[t['name'] for t in rust['tracks']]}\n"
            f"  python track names: {[t['name'] for t in py['tracks']]}"
        )

    r_names = [m["name"] for m in rust["markers"]]
    p_names = [m["name"] for m in py["markers"]]
    if r_names != p_names:
        diffs.append(f"markers: rust={r_names} python={p_names}")

    r_ret = [t["name"] for t in rust["return_tracks"]]
    p_ret = [t["name"] for t in py["return_tracks"]]
    if r_ret != p_ret:
        diffs.append(f"return_tracks: rust={r_ret} python={p_ret}")

    return diffs


def main():
    verbose = "--verbose" in sys.argv
    if not PY_CLAVUS.exists():
        print(f"⚠ Python reference not found at {PY_CLAVUS} — skipping")
        return 0

    total_diffs = 0
    known_count = 0
    for fixture in FIXTURES:
        if not fixture.exists():
            print(f"⚠ {fixture.name} missing — skipping")
            continue
        diffs = compare(fixture, verbose)
        known = KNOWN_DIFFS.get(fixture.name, [])

        unexpected = []
        for d in diffs:
            match = next(
                ((sub, why) for sub, why in known if d.startswith(sub)), None
            )
            if match:
                known_count += 1
                print(f"◦ {fixture.name}: known difference — {d}")
                print(f"    reason: {match[1]}")
            else:
                unexpected.append(d)

        if unexpected:
            total_diffs += len(unexpected)
            print(f"✗ {fixture.name}: {len(unexpected)} unexpected difference(s)")
            for d in unexpected:
                print(f"    {d}")
        else:
            print(f"✓ {fixture.name}: no unexpected differences")

    if total_diffs:
        print(f"\n{total_diffs} unexpected difference(s) — review each one.")
        return 1
    print(f"\nNo unexpected differences ({known_count} known difference(s) explained).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
