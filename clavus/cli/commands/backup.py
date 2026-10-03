from __future__ import annotations

"""Clavus CLI commands: backup, backups, restore-store."""
import argparse
from clavus.helpers import get_store_and_project

def cmd_backup(args: argparse.Namespace) -> None:
    """Backup the entire Clavus store (cues, snapshots, refs, config)."""
    from clavus.progress import Spinner
    from clavus.store import BlobStore
    store = BlobStore()
    with Spinner("creating backup..."):
        archive_path = store.backup_store()
    size_mb = archive_path.stat().st_size / (1024 * 1024)
    print(f"Backup saved: {archive_path}")
    print(f"   Size: {size_mb:.1f} MB")
    print(f"   To restore: clavus restore --store {archive_path}")


def cmd_list_backups(args: argparse.Namespace) -> None:
    """List available store backups."""
    from clavus.store import BlobStore
    store = BlobStore()
    backups = store.list_backups()
    if not backups:
        print("  No backups found.")
        print("  Create one with: clavus backup")
        return
    print(f"Available backups:")
    for b in backups:
        size_kb = b.stat().st_size / 1024
        print(f"     {b.name}  ({size_kb:.0f} KB)")


def cmd_restore_store(args: argparse.Namespace) -> None:
    """Restore Clavus store from a backup archive."""
    from clavus.store import BlobStore
    store = BlobStore()
    if not args.archive:
        backups = store.list_backups()
        if not backups:
            print("-- No backups found. Run 'clavus backup' first.")
            return
        archive_path = backups[0]
        print(f"  Using latest backup: {archive_path.name}")
    else:
        archive_path = Path(args.archive)
    store.restore_store(archive_path)


# ─── Stem Commands ─────────────────────────────────────────────────────


