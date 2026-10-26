from __future__ import annotations

"""Clavus CLI commands: init, init_project."""
import argparse
import getpass
import json
import sys
import time
from pathlib import Path
from typing import Optional
from clavus.parser import parse_als, project_summary
from clavus.config import ClavusConfig, CONFIG_PATH
from clavus.store import BlobStore, ClavusProject
from clavus.helpers import find_als_file, get_store_and_project

def init_project(path_str: str | None, auto_confirm: bool = False) -> tuple[Optional[str], list[str]]:
    """Core init logic (non-interactive). Returns (project_name, log_lines). 
    
    If auto_confirm=True, skips all prompts and uses defaults. Used by TUI.
    Returns (None, [...]) on failure.
    """
    logs: list[str] = []
    if path_str:
        target = Path(path_str).resolve()
        als_path = find_als_file(target)
        if als_path is None:
            logs.append(f"-- No .als file found at {target}")
            return None, logs
    else:
        return None, ["-- No path provided"]

    store = BlobStore()
    store.init()

    # Check if already initialized
    existing = store.get_index(als_path.stem)
    if existing:
        logs.append(f"[WARN]  Project '{als_path.stem}' already tracked at {existing.root_als}")
        return existing.name, logs

    # Project name from .als filename
    project_name = als_path.stem
    logs.append(f"Project: {project_name}")

    # Parse the .als
    project = parse_als(als_path)
    logs.append(f"🔍 {project.track_count} tracks @ {project.bpm}bpm")

    # Copy to ~/Clavus/Projects/
    from clavus.helpers import get_projects_dir
    import shutil

    projects_root = get_projects_dir()
    target_dir = projects_root / project_name
    target_als = target_dir / f"{project_name}.als"

    if target_dir.exists():
        logs.append(f"[WARN]  '{project_name}' already exists in {projects_root}")
        return project_name, logs

    source_dir = als_path.parent
    shutil.copytree(
        source_dir, target_dir,
        ignore=shutil.ignore_patterns("Backup*", "Ableton Project Info", ".DS_Store"),
        dirs_exist_ok=True,
    )
    logs.append(f"[ok] Copied -> {target_dir}")

    # Rename .als to match project name if they differ
    if not target_als.exists():
        existing = list(target_dir.glob("*.als"))
        if existing:
            existing[0].rename(target_als)
            logs.append(f"📛 Renamed {existing[0].name} -> {target_als.name}")

    # Update als_path to the copy
    als_path = target_als
    project.file_path = str(als_path)

    config = ClavusConfig.load()
    # Detect project type from file extension
    project_type = "bitwig" if als_path.suffix.lower() == ".bwproject" else "ableton"

    clavus_proj = ClavusProject(
        name=project_name,
        root_als=str(als_path),
        project_type=project_type,
        created_at=time.time(),
        description="",
    )

    snap = store.save_snapshot(project, "Initial import", parent=None)
    store.update_ref("HEAD", snap.hash)
    store.update_ref("refs/tags/initial", snap.hash)
    store.set_project_head(clavus_proj, snap.hash, source="init")

    # Set as last project
    if store.index_path.exists():
        index = json.loads(store.index_path.read_text())
        index["_last_project"] = project_name
        store.index_path.write_text(json.dumps(index, indent=2, default=str))

    logs.append(f"[ok] Initialized '{project_name}' — {project.track_count} tracks")
    # Load the snapshot to report sample count (save_snapshot already stored it)
    snap_loaded = store.load_snapshot(snap.hash)
    sample_count = len(snap_loaded.sample_hashes) if snap_loaded and snap_loaded.sample_hashes else 0
    if sample_count:
        logs.append(f"   {sample_count} sample{'s' if sample_count != 1 else ''} indexed")
    logs.append(f"   ● snapshot {snap.short_hash()}")
    return project_name, logs


def cmd_init(args: argparse.Namespace) -> None:
    """Initialize a new Clavus project — friendly, guided setup."""
    if args.path:
        target = Path(args.path).resolve()
        als_path = find_als_file(target)
        if als_path is None:
            print(f"-- No .als file found at {target}")
            print("   Specify the path to an .als file or a directory containing one.")
            sys.exit(1)
    else:
        # Auto-detect: scan common locations for .als files
        search_dirs = [
            Path.home() / "Desktop",
            Path.home() / "Documents",
            Path.home() / "Music" / "Ableton",
            Path.home() / "Ableton",
        ]
        als_files = []
        for d in search_dirs:
            if d.exists():
                for f in d.rglob("*.als"):
                    if "Backup" not in str(f) and "Ableton Project Info" not in str(f):
                        als_files.append(f)

        if not als_files:
            print("-- No .als files found on Desktop, Documents, or Music/Ableton.")
            print("   Usage: clavus init /path/to/project.als")
            sys.exit(1)

        if len(als_files) == 1:
            als_path = als_files[0]
            print(f"Found: {als_path.name}")
        else:
            print(f"Found multiple .als files:")
            for i, f in enumerate(als_files[:20], 1):
                print(f"  {i}. {f.name}  ({f.parent})")
            print()
            print("  Enter number (or 0 to cancel): ", end="")
            try:
                choice = int(input().strip())
                if 1 <= choice <= len(als_files):
                    als_path = als_files[choice - 1]
                else:
                    print("-- Cancelled.")
                    sys.exit(1)
            except (ValueError, EOFError):
                print("-- Cancelled.")
                sys.exit(1)

    # ── Config check: prompt for author if not set ──
    config = ClavusConfig.load()
    # Only ask when there is someone to ask. A script or a test has no
    # terminal to read from, and the default author is the right answer
    # for both. Every other prompt below is guarded the same way.
    if (not config.author or config.author == getpass.getuser()) and sys.stdin.isatty():
        print("👋 First, who's the author for this project?")
        author_input = input(f"   Author [{config.author or getpass.getuser()}]: ").strip()
        if author_input:
            config.author = author_input
            config.save()
            print(f"   Saved to {CONFIG_PATH}")
        else:
            # Keep the default (getpass.getuser())
            pass
        print()

    # ── Welcome ──
    print("╭──────────────────────────────────────────────╮")
    print("│  Clavus — Ableton Live collaboration            │")
    print("╰──────────────────────────────────────────────╯")
    print()

    store = BlobStore()
    store.init()

    # Check if already initialized
    existing = store.get_index(als_path.stem)
    if existing:
        print(f"[WARN]  Project '{als_path.stem}' already tracked at {existing.root_als}")
        return

    # ── Project name ──
    suggested = als_path.stem
    print(f"Project: {suggested}")
    if sys.stdin.isatty():
        name_input = input(f"   Name [{suggested}]: ").strip()
        project_name = name_input if name_input else suggested
    else:
        project_name = suggested
    print()

    # ── Description (optional) ──
    print("📝 Optional description (what is this project?)")
    if sys.stdin.isatty():
        desc_input = input(f"   Description []: ").strip()
    else:
        desc_input = ""
    print()

    # ── Parse ──
    print("🔍 Scanning .als file...")
    from clavus.progress import Spinner
    with Spinner("parsing project file..."):
        project = parse_als(als_path)
    print()

    # ── Summary ──
    print(project_summary(project))
    print()

    # ── Confirm ──
    if sys.stdin.isatty():
        confirm = input("   Ready to track this project? [Y/n]: ").strip().lower()
        if confirm and confirm not in ("y", "yes", ""):
            print("✋ Cancelled. Nothing was saved.")
            return
    print()

    # ── Init ──
    # Copy project to ~/Clavus/Projects/ so everything lives in one place
    from clavus.helpers import get_projects_dir
    import shutil

    projects_root = get_projects_dir()
    target_dir = projects_root / project_name
    target_als = target_dir / f"{project_name}.als"

    if target_dir.exists():
        print(f"[WARN]  '{project_name}' already exists in {projects_root}")
        print(f"   Use 'clavus project \"{project_name}\"' to switch to it.")
        return

    print(f"Copying project to {target_dir}...")
    source_dir = als_path.parent
    base_ignore = shutil.ignore_patterns("Backup*", "Ableton Project Info", ".DS_Store")

    # Both paths are resolved before they are compared: on macOS a
    # temporary directory is reached through /var, which is a symlink to
    # /private/var, and an unresolved path would not match the resolved
    # one. The check would then miss, and the copy would go on forever.
    resolved_target = target_dir.resolve()

    def _ignore(dir_path, names):
        """Skip the usual noise, and never the directory being filled.

        The projects directory can sit inside the folder being copied —
        a set that already lives in ~/Clavus/Projects, or a test that
        keeps both in one temporary directory. Copying it into itself
        would never end, so every folder on the way to the target is
        left out.
        """
        skipped = set(base_ignore(dir_path, names))
        for name in names:
            candidate = (Path(dir_path) / name).resolve()
            if candidate == resolved_target or resolved_target.is_relative_to(candidate):
                skipped.add(name)
        return skipped

    shutil.copytree(
        source_dir, target_dir,
        ignore=_ignore,
        dirs_exist_ok=True,
    )

    # Rename .als to match project name if they differ
    if not target_als.exists():
        existing = list(target_dir.glob("*.als"))
        if existing:
            existing[0].rename(target_als)
            print(f"   📛 Renamed {existing[0].name} -> {target_als.name}")

    # Update als_path to the copy and re-parse
    als_path = target_als
    project.file_path = str(als_path)  # update path in parsed project
    print(f"   [ok] Copied -> {target_dir}")
    print()

    # Detect project type from file extension
    project_type = "bitwig" if als_path.suffix.lower() == ".bwproject" else "ableton"

    clavus_proj = ClavusProject(
        name=project_name,
        root_als=str(als_path),
        project_type=project_type,
        created_at=time.time(),
        description=desc_input,
    )

    snap = store.save_snapshot(project, "Initial import", parent=None)
    store.update_ref("HEAD", snap.hash)
    store.update_ref(f"refs/tags/initial", snap.hash)
    store.set_project_head(clavus_proj, snap.hash, source="init-cli")

    # ── Done ──
    print(f"[ok] Initialized Clavus project '{clavus_proj.name}'")
    print(f"   .als: {als_path}")
    print(f"   Tracks: {project.track_count} @ {project.bpm}bpm")
    print(f"   Snapshot: {snap.short_hash()}")
    if clavus_proj.description:
        print(f"   Notes: {clavus_proj.description}")
    print()
    print("   Next steps:")
    print(f"     open {target_dir}                Open in Finder")
    print(f'     clavus snapshot "my changes"     Save a snapshot')
    print(f"     clavus log                       View history")
    print(f"     clavus push                      Share with collaborators")
    print()
    print(f"   [TIP] Your original at {source_dir} is untouched.")
    print(f"      Work from {target_dir} going forward.")


