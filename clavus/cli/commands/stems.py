from __future__ import annotations

"""Clavus CLI commands: stem import, list, push, pull."""
import argparse
from clavus.store import StemStore
from clavus.helpers import get_store_and_project

def cmd_stem_import(args: argparse.Namespace) -> None:
    """Import a bounced stem file and register it with the current snapshot."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    head = store.read_ref("HEAD")

    if not head:
        print("-- No snapshot yet. Run 'clavus snapshot' first.")
        return

    entry = stem_store.store_stem_file(args.file, args.track)
    manifest = stem_store.get_manifest(head) or StemManifest(snapshot_hash=head)

    # Check if this track already has a stem — replace it
    manifest.stems = [s for s in manifest.stems if s.track_name != args.track]
    manifest.stems.append(entry)
    manifest.created_at = time.time()
    stem_store.save_manifest(manifest)

    file_size_mb = entry.size / (1024 * 1024)
    print(f"Imported stem: {entry.track_name} ({entry.file_name})")
    print(f"   Hash:   {entry.hash[:12]}")
    print(f"   Size:   {file_size_mb:.1f} MB")
    print(f"   Format: {entry.format} / {entry.sample_rate}Hz / {entry.bit_depth}bit / {entry.channels}ch")
    if entry.duration_seconds:
        print(f"   Length: {entry.duration_seconds:.1f}s")
    print(f"   Snapshot: {head[:12]}")


def cmd_stem_import_folder(args: argparse.Namespace) -> None:
    """Import all WAV files from a folder as stems. Derives track names from filenames."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    head = store.read_ref("HEAD")

    if not head:
        print("-- No snapshot yet. Run 'clavus snapshot' first.")
        return

    folder = Path(args.folder).expanduser().resolve()
    if not folder.is_dir():
        print(f"-- Not a directory: {folder}")
        return

    # Support WAV (common stem format) and AIFF (Ableton default)
    audio_files = sorted(
        set(folder.glob("*.wav")) | set(folder.glob("*.WAV"))
        | set(folder.glob("*.aif")) | set(folder.glob("*.aiff"))
        | set(folder.glob("*.AIF")) | set(folder.glob("*.AIFF"))
    )
    if not audio_files:
        print(f"-- No audio files (.wav, .aif, .aiff) found in {folder}")
        return

    manifest = stem_store.get_manifest(head) or StemManifest(snapshot_hash=head)
    imported = 0
    skipped = 0
    total_size = 0

    print(f"\n  Importing {len(audio_files)} stem(s) from {folder}/ ...\n")
    for audio_path in audio_files:
        track_name = args.prefix + audio_path.stem if args.prefix else audio_path.stem
        try:
            entry = stem_store.store_stem_file(str(audio_path), track_name)
        except Exception as e:
            print(f"  [X] {audio_path.name}: {e}")
            skipped += 1
            continue

        # Replace existing stem for this track if present
        manifest.stems = [s for s in manifest.stems if s.track_name != track_name]
        manifest.stems.append(entry)
        imported += 1
        total_size += entry.size

    if imported == 0:
        print("  No stems imported.")
        return

    manifest.created_at = time.time()
    stem_store.save_manifest(manifest)

    total_mb = total_size / (1024 * 1024)
    print(f"\n  [ok] Imported {imported} stem(s)")
    if skipped:
        print(f"  [WARN]  Skipped {skipped}")
    print(f"  Total: {total_mb:.1f} MB across {len(manifest.stems)} stems")
    print(f"  Snapshot: {head[:12]}")
    print(f"\n  Ready to push: clavus stem push")


def cmd_stem_list(args: argparse.Namespace) -> None:
    """List stems for the current (or specified) snapshot."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    head = args.snapshot or store.read_ref("HEAD")

    if not head:
        print("-- No snapshot to show stems for.")
        return

    manifest = stem_store.get_manifest(head)
    if not manifest or not manifest.stems:
        print(f"No stems registered for snapshot {head[:12]}")
        return

    print(f"Stems for snapshot {head[:12]}:\n")
    total_size = 0
    for entry in manifest.stems:
        size_mb = entry.size / (1024 * 1024)
        total_size += entry.size
        duration = f"{entry.duration_seconds:.1f}s" if entry.duration_seconds else "?"
        print(f"  {entry.track_name:20s}  {entry.file_name:25s}  {size_mb:6.1f} MB  {duration}  {entry.hash[:12]}")
    print(f"\n  Total: {total_size / (1024 * 1024):.1f} MB across {len(manifest.stems)} stems")


def cmd_stem_pull(args: argparse.Namespace) -> None:
    """Pull stem files from remotes (materialize working tree)."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    remotes = load_remotes(store)

    if not remotes:
        print("-- No remotes configured. Use 'clavus remote add' first.")
        return

    head = store.read_ref("HEAD")
    if not head:
        print("-- No snapshot yet.")
        return

    # Fetch stems from remotes via the sync extension
    from clavus.sync import pull_stems_from_remote
    total = 0
    manifest_data = None
    for remote in remotes:
        print(f"  Pulling stems from '{remote.name}'...")
        count, mdata = pull_stems_from_remote(store, proj, remote)
        total += count
        if count:
            print(f"    Downloaded {count} stem(s)")
            manifest_data = mdata
        else:
            print(f"    No new stems")

    # Materialize working tree using the snapshot that actually has stems
    snap_hash = (manifest_data.get("snapshot_hash", "") if manifest_data else "") or head
    manifest = stem_store.get_manifest(snap_hash)
    if manifest and manifest.stems:
        paths = stem_store.materialize_stems(snap_hash)
        print(f"\n  Materialized {len(paths)} stem(s) to ~/.clavus/stems/{proj.name}/{snap_hash[:12]}/")
    else:
        print(f"\n  No stem manifest found on relay for this project.")


def cmd_stem_push(args: argparse.Namespace) -> None:
    """Push stem files to all remotes."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    remotes = load_remotes(store)

    if not remotes:
        print("-- No remotes configured.")
        return

    head = store.read_ref("HEAD")
    if not head:
        print("-- No snapshot yet.")
        return

    manifest = stem_store.get_manifest(head)
    if not manifest or not manifest.stems:
        print("No stems to push for current snapshot.")
        return

    from clavus.sync import push_stems_to_remote, SyncClient
    for remote in remotes:
        print(f"  Pushing to '{remote.name}'...")
        try:
            client = SyncClient(remote.url)
            if not client.fast_ping(timeout=3.0):
                print(f"    ⏭  Skipping — unreachable")
                client.close()
                continue
            count = push_stems_to_remote(store, proj, remote, stem_store, head)
            print(f"    Pushed {count} stem(s)")
        except Exception as e:
            print(f"    ⏭  Skipping — {e}")


# ─── CLI Banner ────────────────────────────────────────────────────────────

BANNER_LINES = [
    "  ┌─⬡────────────────────────────────────────────────────┐",
    "  │                                                        │",
    "  │   ██████╗  ██████╗ ██╗   ██╗██╗      ██████╗ ██╗    ██╗  │",
    "  │   ██╔══██╗██╔═══██╗██║   ██║██║     ██╔═══██╗██║    ██║  │",
    "  │   ██████╔╝██║   ██║██║   ██║██║     ██║   ██║██║ █╗ ██║  │",
    "  │   ██╔══██╗██║   ██║██║   ██║██║     ██║   ██║██║███╗██║  │",
    "  │   ██║  ██║╚██████╔╝╚██████╔╝███████╗╚██████╔╝╚███╔███╔╝  │",
    "  │   ╚═╝  ╚═╝ ╚═════╝  ╚═════╝ ╚══════╝ ╚═════╝  ╚══╝╚══╝   │",
    "  │                                                        │",
    "  │   snapshot · sync · collaborate on Ableton Live        │",
    "  └─────────────────────────────────────────────────────────┘",
]


# ─── Main Entry Point ──────────────────────────────────────────────────

