"""Clavus — file watcher daemon.

Auto-snapshots an Ableton project every time the .als file is modified on disk.
Uses file polling (cross-platform, no extra deps) with configurable debounce.
"""

from __future__ import annotations

import hashlib
import os
import time
from pathlib import Path
from typing import Optional

from clavus import parse_als
from clavus.helpers import get_store_and_project
from clavus.store import BlobStore, ClavusProject, diff_projects, format_diff


# ─── Config ──────────────────────────────────────────────────────────────

DEFAULT_COOLDOWN = 30  # seconds to wait after last mtime change before snapshotting
POLL_INTERVAL = 2      # seconds between file system checks


def _file_hash(path: Path) -> str:
    """Quick SHA256 of the .als file content for change detection."""
    with open(path, "rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def _auto_message() -> str:
    """Generate an auto-snapshot message."""
    return f"Auto-snapshot @ {time.strftime('%Y-%m-%d %H:%M:%S')}"


def watch(
    store: BlobStore,
    proj: ClavusProject,
    cooldown: int = DEFAULT_COOLDOWN,
    verbose: bool = True,
    once: bool = False,
) -> None:
    """Watch the project's .als file for changes and auto-snapshot.

    Args:
        store: BlobStore instance
        proj: Active ClavusProject
        cooldown: Seconds to wait after last detected change before snapshotting
        verbose: Print snapshot info on each auto-snapshot
        once: If True, take one snapshot and return (for cron usage)
    """
    als_path = Path(proj.root_als)
    if not als_path.exists():
        print(f"❌ .als file not found: {als_path}")
        return

    # Track state
    previous_hash = _file_hash(als_path)
    last_change = time.time()
    last_snapshot = time.time()
    cooldown_remaining = 0

    if verbose:
        print(f"👁  Watching '{proj.name}' — {als_path.name}")
        print(f"   Cooldown: {cooldown}s  Poll: {POLL_INTERVAL}s")
        if once:
            print(f"   Mode: one-shot")
        else:
            print(f"   Press Ctrl+C to stop")
        print()

    try:
        while True:
            time.sleep(POLL_INTERVAL)
            current_hash = _file_hash(als_path)

            if current_hash != previous_hash:
                # File changed — update timer
                previous_hash = current_hash
                last_change = time.time()
                if verbose:
                    remaining = max(0, cooldown - (last_change - last_snapshot))
                    print(f"   ✏️  Change detected — snapshot in {remaining:.0f}s..." if remaining > 0 else "   ✏️  Change detected — snapshotting...")
                cooldown_remaining = cooldown
            elif cooldown_remaining > 0:
                # In cooldown — countdown
                elapsed = time.time() - last_change
                cooldown_remaining = max(0, cooldown - elapsed)

                if cooldown_remaining == 0 and (time.time() - last_snapshot) >= 1:
                    # Cooldown expired — take snapshot
                    _take_snapshot(store, proj, verbose)
                    last_snapshot = time.time()
                    if once:
                        return
            else:
                # No changes, no cooldown — refresh UI
                if verbose:
                    poll_count = 0

    except KeyboardInterrupt:
        if verbose:
            print()
            print("👋 Watch stopped.")


def _take_snapshot(store: BlobStore, proj: ClavusProject, verbose: bool = True) -> None:
    """Parse the current .als and create a snapshot."""
    als_path = Path(proj.root_als)
    try:
        project = parse_als(als_path)
    except Exception as e:
        print(f"❌ Failed to parse .als: {e}")
        return

    # Check if anything actually changed (compare raw .als hashes)
    import hashlib
    raw_bytes = als_path.read_bytes()
    current_als_hash = hashlib.sha256(raw_bytes).hexdigest()
    prev = store.load_snapshot(proj.head) if proj.head else None
    if prev and prev.als_hash == current_als_hash:
        return  # identical content, no snapshot needed

    snap = store.save_snapshot(
        project,
        message=_auto_message(),
        parent=proj.head,
    )

    # Skip if no changes (redundant — pre-check already caught this)
    if snap.als_hash == current_als_hash:
        return

    # Update references
    store.update_ref("HEAD", snap.hash)
    proj.head = snap.hash
    store.set_index(proj)

    if verbose:
        if prev:
            prev_project = store.load_project(prev.hash)
            if prev_project:
                diff = diff_projects(prev_project, project)
                summary = diff.summary
            else:
                summary = f"{project.track_count} tracks @ {project.bpm}bpm"
        else:
            summary = f"{project.track_count} tracks @ {project.bpm}bpm"
        print(f"📸 Auto-snapshot: {snap.short_hash()} — {summary}")


def watch_once(
    store: BlobStore,
    proj: ClavusProject,
    verbose: bool = True,
) -> bool:
    """Take a single auto-snapshot if the file has changed since last snapshot.
    
    Returns True if a snapshot was taken.
    """
    als_path = Path(proj.root_als)
    if not als_path.exists():
        return False

    import hashlib
    current_als_hash = hashlib.sha256(als_path.read_bytes()).hexdigest()
    prev = store.load_snapshot(proj.head) if proj.head else None
    if prev and prev.als_hash == current_als_hash:
        return False  # no change
    
    try:
        project = parse_als(als_path)
    except Exception:
        return False
    
    snap = store.save_snapshot(
        project,
        message=_auto_message(),
        parent=proj.head,
    )
    
    if snap.als_hash == current_als_hash:
        return False
    
    store.update_ref("HEAD", snap.hash)
    proj.head = snap.hash
    store.set_index(proj)
    
    if verbose:
        if prev:
            prev_project = store.load_project(prev.hash)
            if prev_project:
                diff = diff_projects(prev_project, project)
                summary = diff.summary
            else:
                summary = f"{project.track_count} tracks @ {project.bpm}bpm"
        else:
            summary = f"{project.track_count} tracks @ {project.bpm}bpm"
        print(f"📸 Auto-snapshot: {snap.short_hash()} — {summary}")
    
    return True
