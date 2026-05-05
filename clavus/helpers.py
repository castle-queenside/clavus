"""Clavus — shared helpers used across CLI, watch, and other modules.

Avoids circular imports between cli.py and watch.py.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Optional

from clavus.store import BlobStore, ClavusProject, DEFAULT_CLAVUS_DIR


def find_als_file(path: str | Path) -> Optional[Path]:
    """Find the .als file in a directory, or use the path directly."""
    p = Path(path)
    if p.is_file() and p.suffix == ".als":
        return p
    if p.is_dir():
        als_files = list(p.glob("*.als"))
        if als_files:
            return als_files[0]
    return None


def get_store_and_project(clavus_dir: str = "") -> tuple[BlobStore, ClavusProject]:
    """Get the store and active project from the current directory."""
    store_dir = Path(clavus_dir) if clavus_dir else DEFAULT_CLAVUS_DIR
    store = BlobStore(store_dir)
    projects = store.list_projects()
    if not projects:
        print("❌ No Clavus projects found. Run 'clavus init' first.")
        sys.exit(1)

    # Prefer the last-used project
    if store.index_path.exists():
        try:
            index = json.loads(store.index_path.read_text())
            last_name = index.get("_last_project")
            if last_name:
                for p in projects:
                    if p.name == last_name:
                        return store, p
        except (json.JSONDecodeError, OSError):
            pass

    cwd = os.getcwd()
    for p in projects:
        if cwd.startswith(os.path.dirname(p.root_als)):
            return store, p

    return store, projects[0]


def resolve_snapshot(store: BlobStore, ref: str) -> Optional[str]:
    """Resolve a reference name or hash to a snapshot hash."""
    hash_str = store.read_ref(f"refs/tags/{ref}")
    if hash_str:
        return hash_str

    hash_str = store.read_ref(ref)
    if hash_str:
        return hash_str

    if len(ref) >= 8:
        for obj_dir in store.objects_dir.iterdir():
            if obj_dir.is_dir():
                for f in obj_dir.iterdir():
                    if f.is_file() and f.suffix == ".meta" and f.stem.startswith(ref):
                        return f.stem
    return None
