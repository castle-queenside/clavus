from __future__ import annotations

"""Clavus CLI commands: restore, open."""
import argparse
from clavus.helpers import get_store_and_project

def cmd_restore(args: argparse.Namespace) -> None:
    """Restore a snapshot's .als file from the stored raw bytes."""
    store, proj = get_store_and_project()

    if args.hash:
        hash_str = resolve_snapshot(store, args.hash)
        if not hash_str:
            print(f"-- Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = store.read_ref("HEAD")
        if not hash_str:
            print("-- No snapshots to restore from.")
            sys.exit(1)

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"-- Snapshot not found: {hash_str}")
        sys.exit(1)

    if not snap.als_hash:
        print(f"-- Snapshot {snap.short_hash()} has no raw .als backup.")
        print("   Only snapshots created *after* the restore feature was built")
        print("   store raw .als data. Create a fresh snapshot first.")
        sys.exit(1)

    raw_als = store.get_object(snap.als_hash)
    if not raw_als:
        print(f"-- Raw .als data missing for snapshot {snap.short_hash()}.")
        print(f"   Looked for blob: {snap.als_hash[:16]}...")
        sys.exit(1)

    als_path = Path(proj.root_als)
    if not als_path.exists():
        print(f"-- Project .als file not found at {als_path}")
        sys.exit(1)

    # Confirmation
    snap_time = time.strftime("%Y-%m-%d %H:%M", time.localtime(snap.timestamp))
    print(f"[note] Restore snapshot: {snap.short_hash()}")
    print(f"   Message: {snap.message}")
    print(f"   Captured: {snap_time}")
    print(f"   Tracks: {snap.track_count}  BPM: {snap.bpm}")
    print(f"   Will overwrite: {als_path}")
    print()
    print(f"[WARN]  This will overwrite your current .als file. A backup will be saved as:")
    print(f"   {als_path}.bak (if one doesn't already exist)")
    print()
    if not args.yes:
        confirm = input("  Continue? (y/N): ").strip().lower()
        if confirm != "y":
            print("-- Restore cancelled.")
            return

    # Backup existing .als (only first time)
    bak_path = als_path.with_suffix(".als.bak")
    if not bak_path.exists():
        bak_path.write_bytes(als_path.read_bytes())
        print(f"Current .als backed up to: {bak_path}")

    # Write the restored .als
    als_path.write_bytes(raw_als)
    print(f"[ok] Restored {als_path.name} to snapshot {snap.short_hash()}")
    print(f"   '{snap.message}' — from {snap_time}")

    # Update HEAD to point at this snapshot
    store.update_ref("HEAD", hash_str)
    store.set_project_head(proj, hash_str, source="restore-cli")
    print(f"   HEAD updated to {snap.short_hash()}")


def cmd_open(args: argparse.Namespace) -> None:
    """Open the latest (or specified) .als snapshot in Ableton Live."""
    import subprocess as sp
    import platform

    store, proj = get_store_and_project()

    # Resolve hash
    if args.hash:
        hash_str = resolve_snapshot(store, args.hash)
        if not hash_str:
            print(f"-- Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = proj.head
        if not hash_str:
            print("-- No snapshots. Push from another machine or take a snapshot first.")
            sys.exit(1)

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"-- Snapshot not found: {hash_str}")
        sys.exit(1)

    if not snap.als_hash:
        print(f"-- Snapshot {snap.short_hash()} has no .als backup.")
        sys.exit(1)

    raw_als = store.get_object(snap.als_hash)
    if not raw_als:
        print(f"-- .als blob missing. Try pulling again to fetch it.")
        sys.exit(1)

    # Determine output path — Ableton project folder convention:
    # "Song.als" must live inside "Song Project/" or Ableton auto-creates
    # a copy there, making Clavus's root_als stale.
    project_name = proj.name.replace(" ", " ")
    if args.output:
        out_path = Path(args.output)
    else:
        project_dir = get_projects_dir() / project_name
        als_project = project_dir / f"{project_name} Project"
        out_path = als_project / f"{project_name}.als"

    out_path.parent.mkdir(parents=True, exist_ok=True)
    # Create Ableton project folder scaffolding so Ableton recognizes
    # this as a valid project folder and doesn't auto-create nested copies
    (out_path.parent / "Samples").mkdir(exist_ok=True)
    (out_path.parent / "Backup").mkdir(exist_ok=True)
    (out_path.parent / "Ableton Project Info").mkdir(exist_ok=True)

    # Materialize audio samples into the project folder first (so they exist)
    sample_written = 0
    if snap.sample_hashes:
        # Extract filename -> RelativePath mapping from the original .als
        import gzip as _gzip, re as _re
        _xml = _gzip.decompress(raw_als).decode("utf-8", errors="replace")
        _als_relpaths: dict[str, str] = {}
        for _m in _re.finditer(r'<RelativePath\s+Value="([^"]+)"', _xml):
            _rp = _m.group(1)
            _fn = _rp.rsplit("/", 1)[-1].rsplit("\\", 1)[-1]
            if _fn not in _als_relpaths:
                _als_relpaths[_fn] = _rp

        base_dir = out_path.parent  # Project folder root
        for sh in snap.sample_hashes:
            fname = store.get_sample_filename(sh)
            relpath = _als_relpaths.get(fname) or store.get_sample_relpath(sh) or ""
            if fname and store.has_object(sh):
                try:
                    store.materialize_sample(sh, base_dir, fname, relpath)
                    sample_written += 1
                except Exception:
                    pass

    # NOTE: Path rewriting DISABLED — it corrupts .als XML on Windows.
    # Write the raw blob as-is. Samples are materialized alongside; Ableton
    # finds them once you point at any one sample in the project folder.
    # from clavus.parser import rewrite_als_sample_paths
    # raw_als = rewrite_als_sample_paths(raw_als, out_path.parent)

    out_path.write_bytes(raw_als)
    print(f"[ok] {project_name}.als -> {out_path}")
    print(f"   Snapshot: {snap.short_hash()} — {snap.message or '(no message)'}")
    print(f"   {snap.track_count} tracks, {snap.bpm} BPM")
    if sample_written:
        print(f"   🎵 {sample_written} audio sample{'s' if sample_written != 1 else ''} -> {base_dir}")

    # Launch Ableton
    system = platform.system()
    if system == "Darwin":
        ableton_path = None
        for candidate in Path("/Applications").glob("Ableton Live*.app"):
            ableton_path = str(candidate)
            break
        if ableton_path:
            result = sp.run(["open", "-a", ableton_path, str(out_path)])
            if result.returncode == 0:
                print(f"   [KEYBOARD] Launched Ableton Live")
            else:
                print(f"   [WARN]  Ableton Live returned exit code {result.returncode}")
                print(f"      Try opening manually: {out_path}")
        else:
            print(f"   [WARN]  Ableton Live not found in /Applications")
            print(f"      Install Ableton Live, then run: open {out_path}")
    elif system == "Windows":
        sp.run(["start", "", str(out_path)], shell=True)
        print(f"   [KEYBOARD] Opened .als with default app")
    else:
        print(f"   [info]  Open manually: {out_path}")

    # Helpful tip for first-open sample resolution
    if sample_written:
        print()
        print("   [TIP] If samples show as offline, point Ableton at any one —")
        print("      all others will resolve automatically.")


# ─── Backup / Restore Store ──────────────────────────────────────────────


