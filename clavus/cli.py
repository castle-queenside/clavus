"""
clavus CLI — the primary user interface.

Commands:
  clavus init [path]          Initialize + git init
  clavus projects             List all tracked projects
  clavus project <name>       Switch active project
  clavus snapshot "message"   Snapshot + git commit .als
  clavus log                  Show clavus + git history
  clavus log --graph          Show branch topology
  clavus branch               List / create branches (clavus + git)
  clavus checkout <name>      Switch branches (clavus + git)
  clavus merge <branch>       Merge branches (clavus + git)
  clavus diff [hash]          Show what changed in a snapshot
  clavus status               Show current project state
  clavus cue "text" @time     Add a timeline-anchored comment
  clavus cues                 List all pending cues
  clavus remote               Manage remotes
  clavus push                 Push to remotes + git push
  clavus pull                 Pull from remotes + git pull
  clavus sync                 Start auto-sync daemon
"""

from __future__ import annotations

import argparse
import getpass
import os
import sys
import time
from pathlib import Path
from typing import Optional

from clavus.parser import parse_als, project_summary
from clavus.config import ClavusConfig, CONFIG_PATH
from clavus.store import (
    BlobStore, ClavusProject, diff_projects, format_diff,
    DEFAULT_CLAVUS_DIR,
)
from clavus.cues import (
    CueStore, CueFilter, format_cue, format_cue_list,
    render_cues_as_markers, add_cue_command,
)
from clavus.helpers import find_als_file, get_store_and_project, resolve_snapshot, get_desktop_path, get_projects_dir
from clavus.watch import watch as cmd_watch_daemon
from clavus.store import (
    StemStore, StemEntry, StemManifest,
)
from clavus.sync import (
    load_remotes, save_remotes, Remote, push_to_remote, pull_from_remote, SyncDaemon,
)


# ─── Commands ──────────────────────────────────────────────────────────

def cmd_repair(args: argparse.Namespace) -> None:
    """Repair Clavus storage — recover from backup, cues, refs, or partial data."""
    from clavus.store import BlobStore, ClavusProject
    from dataclasses import asdict
    import json, time, shutil
    from pathlib import Path

    store = BlobStore()
    index_path = store.index_path
    bak_paths = [
        index_path.with_suffix(index_path.suffix + s)
        for s in [".bak", ".bak2", ".bak3"]
    ]

    print("🔧 Clavus Repair")
    print("───")
    print()

    # Phase 1: Check current index health
    if index_path.exists() and not args.force:
        try:
            data = json.loads(index_path.read_text())
            valid = sum(1 for v in data.values() if isinstance(v, dict) and "root_als" in v)
            print(f"✅ index.json exists with {valid} project(s).")
            print("   Use --force to re-scan from scratch.")
            return
        except json.JSONDecodeError:
            print(f"⚠️  index.json is corrupt (will rebuild).")
    elif args.force:
        print("🔨 Force mode: re-scanning from scratch.")

    print("🔍 Scanning for recoverable data...")
    recovered = {}

    # Scan refs/ for HEAD
    head_hash = store.read_ref("HEAD")
    if head_hash:
        print(f"   📍 HEAD ref: {head_hash[:16]}...")
    else:
        print("   📍 No HEAD ref found.")

    # Scan backups
    for bak in bak_paths:
        if bak.exists():
            try:
                data = json.loads(bak.read_text())
                valid = {k: v for k, v in data.items()
                         if isinstance(v, dict) and "root_als" in v}
                if valid:
                    print(f"   💾 Found backup: {bak.name} ({len(valid)} project(s))")
                    recovered.update(valid)
            except (json.JSONDecodeError, OSError):
                continue

    # Scan cues/ directories for project names
    cues_root = Path(store.root) / "cues"
    if cues_root.exists():
        for proj_dir in sorted(cues_root.iterdir()):
            if proj_dir.is_dir() and not proj_dir.name.startswith("."):
                cue_count = len(list(proj_dir.glob("*.json")))
                if cue_count > 0 and proj_dir.name not in recovered:
                    print(f"   📋 Found cues for '{proj_dir.name}' ({cue_count} cue(s))")
                    proj = ClavusProject(
                        name=proj_dir.name,
                        root_als="",
                        created_at=time.time(),
                        head=head_hash,
                        description="(recovered — set .als path with --set-als)",
                    )
                    recovered[proj.name] = asdict(proj)
                elif cue_count > 0:
                    print(f"   📋 Cues for '{proj_dir.name}' ({cue_count} cue(s)) — already in backup")
                else:
                    print(f"   📋 Empty cues dir for '{proj_dir.name}' — skipping")

    if not recovered:
        print("❌ Nothing to recover. No backups, cues, or refs found.")
        return

    # Phase 2: apply --set-als mappings
    if args.set_als:
        if args.set_als.startswith("all="):
            all_path = args.set_als[4:]
            for proj_name in recovered:
                recovered[proj_name]["root_als"] = all_path
            print(f"   🎯 Set .als path for ALL projects: {all_path}")
        elif "=" in args.set_als:
            name, path = args.set_als.split("=", 1)
            if name in recovered:
                recovered[name]["root_als"] = path
                print(f"   🎯 Set .als path for '{name}': {path}")
            else:
                print(f"   ⚠️  Project '{name}' not found in recovered data.")

    # Phase 3: find .als files matching project names
    for proj_name, proj_data in recovered.items():
        if proj_data.get("root_als"):
            continue  # already has a path
        # Search typical locations
        from pathlib import Path as P
        candidates = list(P.home().glob(f"Desktop/{proj_name}*.als")) \
                   + list(P.home().glob(f"Desktop/**/{proj_name}*.als")) \
                   + list(P.home().glob(f"Documents/**/{proj_name}*.als"))
        if candidates:
            best = str(candidates[0])
            recovered[proj_name]["root_als"] = best
            print(f"   🔍 Auto-detected .als for '{proj_name}': {best}")

    # Phase 4: write recovered index
    recovered["_last_project"] = store.read_ref("_last_project") or list(recovered.keys())[0]
    store._backup_index()
    store._write_json(index_path, recovered)
    print()
    print(f"✅ Recovered {len([k for k in recovered if isinstance(recovered[k], dict)])} project(s) to {index_path}")
    print()
    for name, data in recovered.items():
        if not isinstance(data, dict):
            continue
        als = data.get("root_als", "") or "(no path)"
        head = data.get("head", "")[:12] if data.get("head") else "(no snapshots)"
        print(f"   {name:<25} {als}")
    print()
    if any(not data.get("root_als") for data in recovered.values() if isinstance(data, dict)):
        print("💡 Tip: Set .als paths with: clavus repair --set-als 'ProjectName=/path/to/project.als'")
        print("   Or for all at once:    clavus repair --set-als 'all=/path/to/ProjectDir/'")
    else:
        print("✅ All projects have .als paths. Run 'clavus projects' to verify.")


def cmd_doctor(args: argparse.Namespace) -> None:
    """Diagnose Clavus store health — read-only check."""
    from clavus.store import BlobStore
    from pathlib import Path
    import json

    store = BlobStore()
    print(f"🔍 Clavus Doctor")
    print(f"   Store: {store.root}")
    print()

    # 1. Check index
    if not store.index_path.exists():
        print(f"  ❌ index.json: MISSING")
    else:
        try:
            data = json.loads(store.index_path.read_text())
            projects = [k for k in data if k != "_last_project"]
            last = data.get("_last_project", "(none)")
            print(f"  ✅ index.json: {len(projects)} project(s), last: {last}")
            for name in projects:
                p = data.get(name)
                if not p or not isinstance(p, dict):
                    print(f"    ⚠️  {name}  — corrupt entry (null), needs repair")
                    continue
                als = p.get("root_als", "")
                als_ok = "✅" if (als and Path(als).exists()) else "⚠️"
                head = (p.get("head") or "")[:12] or "(none)"
                print(f"    {als_ok} {name}  @ {head}  {als[:50] if als else '(no path)'}")
        except (json.JSONDecodeError, OSError) as e:
            print(f"  ❌ index.json: CORRUPT — {e}")

    # 2. Check backups
    backups = store.list_backups()
    if backups:
        print(f"  ✅ Backups: {len(backups)} available (latest: {backups[0].name})")
    else:
        print(f"  ⚠️  No backups — run 'clavus backup' to create one")

    # 3. Check refs
    head = store.read_ref("HEAD")
    if head:
        print(f"  ✅ HEAD: {head[:16]}...")
    else:
        print(f"  ⚠️  No HEAD ref")

    # 4. Check objects
    obj_count = sum(1 for f in store.objects_dir.rglob("*") if f.is_file())
    print(f"  ✅ Objects: {obj_count} blob(s)")

    # 5. Check cues
    cues_root = store.root / "cues"
    if cues_root.exists():
        cue_count = sum(1 for f in cues_root.rglob("*.json"))
        print(f"  ✅ Cues: {cue_count} file(s)")
    else:
        print(f"  ⚠️  No cues directory")

    print()
    if args.verbose:
        print(f"  Backups:")
        for b in backups[:5]:
            size = b.stat().st_size / 1024
            print(f"    {b.name}  ({size:.0f} KB)")
        print()
    print(f"  💡 Run 'clavus backup' to create a full backup")
    print(f"  💡 Run 'clavus repair' to recover from corruption")
    print(f"  💡 Run 'clavus restore-store' to restore from backup")


def cmd_setup(args: argparse.Namespace) -> None:
    """Guided first-run setup — author, port, Tailscale, Ableton detection."""
    import platform, socket, subprocess
    from clavus.config import ClavusConfig
    from clavus.store import BlobStore

    print("🎹 Clavus Setup")
    print("───")
    print()

    cfg = ClavusConfig.load()

    # 1. Author name
    current = cfg.author or os.environ.get("USER") or os.environ.get("USERNAME") or ""
    print(f"👤 Author name [{current}]: ", end="")
    try:
        author = input().strip()
    except (EOFError, KeyboardInterrupt):
        author = ""
    cfg.set("author", author or current)

    # 2. Port
    port = cfg.port or 7890
    print(f"🔌 Relay port [{port}]: ", end="")
    try:
        pi = input().strip()
        if pi:
            port = int(pi)
    except (ValueError, EOFError, KeyboardInterrupt):
        pass
    cfg.set("port", port)

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    in_use = sock.connect_ex(("127.0.0.1", port)) == 0
    sock.close()
    if in_use:
        print(f"   ⚠️  Port {port} in use — choose another: clavus config set port <N>")

    # 3. Tailscale check
    print()
    print("🌐 Network discovery...")
    try:
        r = subprocess.run(["tailscale", "version"], capture_output=True, text=True, timeout=5)
        if r.returncode == 0:
            print("   ✅ Tailscale found")
            r2 = subprocess.run(["tailscale", "ip", "-4"], capture_output=True, text=True, timeout=5)
            ts_ip = r2.stdout.strip()
            if ts_ip:
                print(f"   📡 Tailscale IP: {ts_ip}")
                cfg.set("tailscale_ip", ts_ip)
        else:
            print("   ℹ️  Tailscale not found — LAN-only collab")
    except (FileNotFoundError, subprocess.TimeoutExpired):
        print("   ℹ️  Tailscale not found — LAN-only collab")

    # 4. Projects folder
    from clavus.config import DEFAULT_PROJECTS_DIR
    current_dir = cfg.projects_dir or DEFAULT_PROJECTS_DIR
    print(f"📁 Projects folder [{current_dir}]: ", end="")
    try:
        pd = input().strip()
        if pd:
            cfg.set("projects_dir", pd)
            Path(pd).mkdir(parents=True, exist_ok=True)
    except (EOFError, KeyboardInterrupt):
        pass
    print(f"   All synced projects go here")

    # 5. Ableton detection
    print()
    ableton = None
    if platform.system() == "Darwin":
        for name in ["Ableton Live 12 Suite", "Ableton Live 12 Intro", "Ableton Live 12 Standard",
                      "Ableton Live 11 Suite", "Ableton Live 11 Intro"]:
            path = f"/Applications/{name}.app"
            if os.path.exists(path):
                ableton = path
                break
    elif platform.system() == "Windows":
        try:
            import winreg
            for ver in [12, 11]:
                try:
                    key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, rf"SOFTWARE\Ableton\Live {ver}")
                    ableton = winreg.QueryValueEx(key, "InstallDir")[0]
                    break
                except Exception:
                    pass
        except Exception:
            pass

    if ableton:
        print(f"   ✅ Ableton: {ableton}")
        cfg.set("ableton_path", ableton)
    else:
        print("   ℹ️  Ableton not auto-detected")
        print("      Set: clavus config set ableton_path /path/to/Ableton")

    # Save
    cfg.save()
    store = BlobStore()
    store.init()

    print()
    print("✅ Setup complete!")
    print(f"   Config: {CONFIG_PATH}")
    print(f"   Data:   {store.root}")
    print()
    print("   Next:")
    print("     clavus init               — track a project")
    print("     clavus share              — share with collaborator")
    print("     clavus tui                — terminal dashboard")
    print("     clavus doctor             — check system health")
    print(f"     clavus config             — show/edit settings")


def cmd_init(args: argparse.Namespace) -> None:
    """Initialize a new Clavus project — friendly, guided setup."""
    if args.path:
        target = Path(args.path).resolve()
        als_path = find_als_file(target)
        if als_path is None:
            print(f"❌ No .als file found at {target}")
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
            print("❌ No .als files found on Desktop, Documents, or Music/Ableton.")
            print("   Usage: clavus init /path/to/project.als")
            sys.exit(1)

        if len(als_files) == 1:
            als_path = als_files[0]
            print(f"📁 Found: {als_path.name}")
        else:
            print("📁 Found multiple .als files:")
            for i, f in enumerate(als_files[:20], 1):
                print(f"  {i}. {f.name}  ({f.parent})")
            print()
            print("  Enter number (or 0 to cancel): ", end="")
            try:
                choice = int(input().strip())
                if 1 <= choice <= len(als_files):
                    als_path = als_files[choice - 1]
                else:
                    print("❌ Cancelled.")
                    sys.exit(1)
            except (ValueError, EOFError):
                print("❌ Cancelled.")
                sys.exit(1)

    # ── Config check: prompt for author if not set ──
    config = ClavusConfig.load()
    if not config.author or config.author == getpass.getuser():
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
    print("│  🎹  Clavus — Ableton Live collaboration     │")
    print("╰──────────────────────────────────────────────╯")
    print()

    store = BlobStore()
    store.init()

    # Check if already initialized
    existing = store.get_index(als_path.stem)
    if existing:
        print(f"⚠️  Project '{als_path.stem}' already tracked at {existing.root_als}")
        return

    # ── Project name ──
    suggested = als_path.stem
    print(f"📁 Project: {suggested}")
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
    clavus_proj = ClavusProject(
        name=project_name,
        root_als=str(als_path),
        created_at=time.time(),
        description=desc_input,
    )

    snap = store.save_snapshot(project, "Initial import", parent=None)
    clavus_proj.head = snap.hash
    store.update_ref("HEAD", snap.hash)
    store.update_ref(f"refs/tags/initial", snap.hash)
    store.set_index(clavus_proj)

    # ── Done ──
    print(f"✅ Initialized Clavus project '{clavus_proj.name}'")
    print(f"   .als: {als_path}")
    print(f"   Tracks: {project.track_count} @ {project.bpm}bpm")
    print(f"   Snapshot: {snap.short_hash()}")
    if clavus_proj.description:
        print(f"   Notes: {clavus_proj.description}")
    print()
    print("   Next steps:")
    print(f"     clavus project \"{project_name}\"    Switch to this project")
    print(f'     clavus snapshot "my changes"     Save a snapshot')
    print(f"     clavus log                       View history")
    print(f"     clavus --help                    All commands")


def cmd_projects(args: argparse.Namespace) -> None:
    """List all tracked projects."""
    store = BlobStore()
    projects = store.list_projects()
    if not projects:
        print("📁 No Clavus projects found.")
        print("   Run 'clavus init <path>' to add one.")
        return

    print(f"📁 Clavus projects ({len(projects)}):")
    print()
    for p in sorted(projects, key=lambda x: x.name):
        head_str = f" @ {p.head[:8]}" if p.head else " (no snapshots)"
        als_exists = "✅" if Path(p.root_als).exists() else "❌"
        print(f"  {p.name:<30} {als_exists} {p.root_als}{head_str}")
    print()
    print(f"  Current: {store.read_ref('_last_project') or 'none'}")

    # Show which is active
    try:
        _, active = get_store_and_project()
        print(f"  Active: {active.name}")
    except SystemExit:
        pass


def cmd_project(args: argparse.Namespace) -> None:
    """Switch the active project."""
    store = BlobStore()
    proj = store.get_index(args.name)
    if not proj:
        print(f"❌ Project '{args.name}' not found.")
        print("   Run 'clavus projects' to see available projects.")
        sys.exit(1)
    store.set_index(proj)
    print(f"✅ Switched to project '{args.name}'")
    print(f"   Path: {proj.root_als}")
    if proj.head:
        print(f"   HEAD: {proj.head[:8]}")
    else:
        print(f"   (no snapshots yet)")
    print(f"   Branch: {proj.branch}")


def cmd_snapshot(args: argparse.Namespace) -> None:
    """Create a new snapshot of the current project state."""
    store, proj = get_store_and_project()
    als_path = Path(proj.root_als)
    if not als_path.exists():
        print(f"❌ .als file not found: {als_path}")
        sys.exit(1)

    # Prompt for message if not provided
    message = args.message
    if not message:
        try:
            message = input("  Snapshot message (or blank to cancel): ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\n❌ Snapshot cancelled.")
            return
        if not message:
            print("❌ Snapshot cancelled.")
            return

    # Parse current state
    project = parse_als(als_path)

    # Create snapshot
    snap = store.save_snapshot(
        project,
        message=message,
        parent=proj.head,
        tags=args.tag.split(",") if args.tag else [],
    )

    # Check if anything actually changed
    if snap.hash == proj.head and proj.head is not None:
        print(f"⚠️  No changes detected — project state is identical to last snapshot.")
        print(f"   Current HEAD: {snap.short_hash()} — '{store.load_snapshot(proj.head).message if store.load_snapshot(proj.head) else ''}'")
        return

    # Update references
    store.update_ref("HEAD", snap.hash)
    proj.head = snap.hash
    store.set_index(proj)

    # Show diff from previous snapshot
    if args.parent:
        parent_snap = store.load_snapshot(args.parent)
    elif snap.parent:
        parent_snap = store.load_snapshot(snap.parent)
    else:
        parent_snap = None

    if parent_snap:
        parent_project = store.load_project(parent_snap.hash)
        if parent_project:
            diff = diff_projects(parent_project, project)
            print(f"📸 Snapshot: {snap.short_hash()} — '{snap.message}'")
            print(f"   {diff.summary}")
            if args.verbose:
                print()
                print(format_diff(diff, verbose=True))
        else:
            print(f"📸 Snapshot: {snap.short_hash()} — '{snap.message}'")
    else:
        print(f"📸 Snapshot: {snap.short_hash()} — '{snap.message}'")
        print(f"   {project.track_count} tracks @ {project.bpm}bpm (initial state)")

    print(f"   📋 Run 'clavus log' to see history.")


def cmd_log(args: argparse.Namespace) -> None:
    """Show snapshot history."""
    store, proj = get_store_and_project()
    current_hash = proj.head
    count = 0

    if not current_hash:
        print("📋 No snapshots yet. Run 'clavus snapshot' to create one.")
        return

    # Collect all branch refs for graph display
    branch_refs: dict[str, str] = {"HEAD": current_hash}
    if store.refs_dir.exists():
        for ref_file in store.refs_dir.glob("heads/*"):
            branch_refs[ref_file.name] = ref_file.read_text().strip()

    print(f"📋 Snapshot history for '{proj.branch}'")
    print()

    # Build a set of all snapshots for the graph
    all_snaps: dict[str, Snapshot] = {}
    def collect(h: str):
        while h and h not in all_snaps:
            snap = store.load_snapshot(h)
            if snap:
                all_snaps[h] = snap
                h = snap.parent if snap.parent else ""
            else:
                break
    for h in branch_refs.values():
        collect(h)

    # Show graph or linear history
    if args.graph:
        # Build a simple graph with branches
        sorted_hashes = sorted(all_snaps.keys(),
                              key=lambda h: all_snaps[h].timestamp if h in all_snaps else 0,
                              reverse=True)

        # Which branches point to each snapshot
        hash_branches: dict[str, list[str]] = {}
        for bname, bh in branch_refs.items():
            h = bh
            while h:
                if h not in hash_branches:
                    hash_branches[h] = []
                if bname not in hash_branches[h]:
                    hash_branches[h].append(bname)
                snap = all_snaps.get(h)
                h = snap.parent if snap else ""

        for i, h in enumerate(sorted_hashes):
            if count >= (args.limit or 30):
                break
            snap = all_snaps.get(h)
            if not snap:
                continue
            count += 1

            # Graph lines
            graph = "|"
            if i > 0:
                prev = sorted_hashes[i-1]
                if prev in all_snaps and all_snaps[prev].parent != h:
                    graph = "\\"

            time_str = time.strftime("%m/%d %H:%M", time.localtime(snap.timestamp))

            # Show branch labels
            labels = hash_branches.get(h, [])
            label_str = ""
            for lb in labels:
                if lb in ("HEAD", proj.branch):
                    label_str += f" *{lb}*"
                else:
                    label_str += f" {lb}"

            print(f"  {graph} {snap.short_hash()}  {time_str}{label_str}")
            if args.verbose:
                print(f"  |    Tracks: {snap.track_count}  BPM: {snap.bpm}")
            print(f"  |    {snap.message[:70]}")

        if count == 0:
            print("  (no snapshots to show)")
    else:
        # Linear history (original behavior)
        while current_hash and count < (args.limit or 20):
            snap = store.load_snapshot(current_hash)
            if not snap:
                break

            is_head = "➡ " if current_hash == store.read_ref("HEAD") else "  "
            time_str = time.strftime("%Y-%m-%d %H:%M", time.localtime(snap.timestamp))

            tags = ""
            if snap.tags:
                tags = f" [{', '.join(snap.tags[:3])}]"

            print(f"  {is_head}{snap.short_hash()}  {time_str}")
            if args.verbose:
                print(f"     Tracks: {snap.track_count}  BPM: {snap.bpm}")
            print(f"     {snap.message}{tags}")

            current_hash = snap.parent
            count += 1

            if count < (args.limit or 20) and current_hash:
                print()

        if current_hash and count >= (args.limit or 20):
            print()
            print(f"  ... and more. Use --limit to show more.")



def cmd_diff(args: argparse.Namespace) -> None:
    """Show what changed in a snapshot vs its parent."""
    store, proj = get_store_and_project()

    if args.hash:
        hash_str = resolve_snapshot(store, args.hash)
        if not hash_str:
            print(f"❌ Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = store.read_ref("HEAD")
        if not hash_str:
            print("📋 No snapshots yet.")
            return

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"❌ Snapshot not found (resolved: '{hash_str}', meta: {store.objects_dir / hash_str[:2] / (hash_str + '.meta')})")
        return

    current_project = store.load_project(hash_str)
    if not current_project:
        print(f"❌ Could not load project data for {hash_str}")
        return

    if snap.parent:
        parent_project = store.load_project(snap.parent)
        if parent_project:
            diff = diff_projects(parent_project, current_project)

            if args.visual:
                # Render visual timeline diff with clip-level detail
                try:
                    from clavus.visual_diff import render_diff_cli
                    print(f"📊 {snap.short_hash()} — '{snap.message}'")
                    diff = diff_projects(parent_project, current_project)
                    print(render_diff_cli(
                        diff=diff,
                        before_proj=parent_project,
                        after_proj=current_project,
                    ))
                    return
                except ImportError:
                    pass  # Fall through to text diff

            print(f"📊 {snap.short_hash()} — '{snap.message}'")
            print()
            print(format_diff(diff, verbose=args.verbose))
            return

    # No parent — show basic info
    print(f"📊 {snap.short_hash()} — '{snap.message}'")
    print(f"   {snap.track_count} tracks @ {snap.bpm}bpm (initial state)")


def cmd_status(args: argparse.Namespace) -> None:
    """Show current project status."""
    store, proj = get_store_and_project()

    als_path = Path(proj.root_als)
    als_exists = als_path.exists()
    last_snap = store.load_snapshot(proj.head) if proj.head else None

    print(f"📁 '{proj.name}'")
    print(f"   Path: {proj.root_als}")
    print(f"   Branch: {proj.branch}")
    print(f"   Status: {'✅ exists' if als_exists else '❌ missing'}")
    print()

    if last_snap:
        project = None
        if als_exists:
            project = parse_als(als_path)
            old_project = store.load_project(last_snap.hash)
            if old_project and project:
                diff = diff_projects(old_project, project)
                print(f"   HEAD: {last_snap.short_hash()} — '{last_snap.message}'")
                if diff.summary != "No changes":
                    print(f"   ⚠️  Unsaved changes detected:")
                    print(f"      {diff.summary}")
                else:
                    print(f"   ✅ Up to date with last snapshot")
        else:
            print(f"   HEAD: {last_snap.short_hash()} — '{last_snap.message}'")
    else:
        print(f"   No snapshots yet.")


def cmd_watch(args: argparse.Namespace) -> None:
    """Start the file watcher daemon."""
    store, proj = get_store_and_project()
    cmd_watch_daemon(
        store,
        proj,
        cooldown=args.cooldown,
        verbose=not args.quiet,
        once=args.once,
    )


def cmd_relay(args: argparse.Namespace) -> None:
    """Start the Clavus relay server for collaboration.

    Provides the HTTP API and WebSocket hub that peers (TUI or CLI)
    connect to for sync, cues, snapshots, and stem transfer.
    Designed to run on a VPS, Raspberry Pi, or always-on machine.

    Other Clavus clients connect using:
      clavus remote add <name> http://<relay-ip>:7890

    The relay prints its LAN and Tailscale IPs on startup."""
    try:
        from clavus.web import run_relay_server
    except ImportError:
        print("❌ Relay server requires fastapi and uvicorn.")
        print("   Install with: pip install fastapi uvicorn")
        sys.exit(1)
    cfg = ClavusConfig.load()
    host = args.host or cfg.host
    port = args.port or cfg.port
    run_relay_server(host=host, port=port)


def cmd_share(args: argparse.Namespace) -> None:
    """Start a share session — relay + auto-discovery.

    Starts the relay server with a share code, advertises via mDNS
    (LAN) and Tailscale API, and waits for someone to connect.

    Your friend runs 'clavus join' and they'll find your relay
    automatically — they just need to verify the share code matches.

    Works over LAN (same WiFi) or Tailscale (anywhere).
    """
    try:
        from clavus.web import run_relay_server, set_share_code
    except ImportError:
        print("❌ Relay server requires fastapi and uvicorn.")
        print("   Install with: pip install fastapi uvicorn")
        sys.exit(1)

    from clavus.discovery import generate_share_code

    cfg = ClavusConfig.load()
    host = args.host or cfg.host
    port = args.port or cfg.port
    share_code = generate_share_code()

    # Check if port is already in use — try to kill stale Clavus relay
    import socket, platform, signal
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    if sock.connect_ex(("127.0.0.1", port)) == 0:
        sock.close()
        print(f"   ⚠️  Port {port} in use — checking for stale Clavus relay...")
        try:
            import httpx
            r = httpx.get(f"http://127.0.0.1:{port}/api/sync/pull", params={"name": "_"}, timeout=2)
            if r.status_code in (200, 404):  # 404 = Clavus responding, just no project
                print(f"   🔄 Restarting stale Clavus relay on port {port}...")
                import subprocess
                if platform.system() == "Windows":
                    subprocess.run(["taskkill", "/f", "/fi", f"WINDOWTITLE eq *clavus*relay*"], capture_output=True)
                else:
                    result = subprocess.run(["lsof", "-ti", f":{port}"], capture_output=True, text=True)
                    for pid in result.stdout.strip().split("\n"):
                        if pid:
                            try:
                                os.kill(int(pid), signal.SIGTERM)
                            except Exception:
                                pass
                import time
                time.sleep(1)
        except Exception:
            print(f"   ❌ Port {port} in use by another app. Use: clavus share --port <N>")
            sys.exit(1)
    else:
        sock.close()

    print(f"  🎹 Clavus Share")
    print(f"  {'─' * 40}")
    print(f"  🔗 Share code: {share_code}")
    print()
    print(f"  Tell a friend to run:")
    print(f"    clavus join")
    print()
    print(f"  They'll find you automatically if:")
    print(f"    • Same WiFi (LAN broadcast)")
    print(f"    • Connected to the same Tailscale network")
    print(f"  {'─' * 40}")
    print()

    # Start mDNS advertising with share code
    try:
        from clavus.discovery import ClavusAdvertiser
        from clavus.store import BlobStore

        store = BlobStore()
        projects = store.list_projects()
        proj_name = projects[0].name if projects else ""

        advertiser = ClavusAdvertiser()
        advertiser.start(
            port=port,
            project=proj_name,
            user=cfg.author,
            version="0.7.0-beta",
            share_code=share_code,
        )
    except ImportError:
        advertiser = None
        print("  ⚠️  LAN advertising unavailable (install zeroconf)")
    except Exception as e:
        advertiser = None
        print(f"  ⚠️  LAN advertising failed: {e}")

    # Start relay (this blocks)
    try:
        run_relay_server(host=host, port=port, share_code=share_code)
    finally:
        if advertiser:
            advertiser.stop()


def cmd_join(args: argparse.Namespace) -> None:
    """Connect to a Clavus relay — by URL, share code, or auto-discovery.

    Direct URL join (fastest, no scanning):
      clavus join http://192.168.12.196:7890
      clavus join http://100.127.1.109:7890

    Share code join (scans LAN/Tailscale):
      clavus join GROOVE-BEAR-5

    Auto-discover (scans LAN/Tailscale, lists sessions):
      clavus join
    """
    from clavus.sync import save_remotes, Remote, load_remotes, pull_from_remote, pull_snapshot_blobs
    from clavus.store import BlobStore
    import concurrent.futures
    from urllib.parse import urlparse

    # ── Direct URL join ──────────────────────────────────────────────
    if args.code and (args.code.startswith("http://") or args.code.startswith("https://")):
        url = args.code.rstrip("/")
        parsed = urlparse(url)
        host = parsed.hostname or "localhost"
        port = parsed.port or 7890
        base = f"http://{host}:{port}"

        print(f"🔗 Connecting to {base}...")

        # Hit /api/share to verify and get project info
        from clavus.sync import SyncClient
        try:
            client = SyncClient(base)
            r = client.client.get(f"{base}/api/share", timeout=10)
            info = r.json() if r.status_code == 200 and r.text else {}
            client.close()
        except Exception as e:
            print(f"  ❌ Cannot reach {base}: {e}")
            return

        code = info.get("share_code", "") or "(no code)"
        author = info.get("author", "") or host
        project = info.get("project", {})
        proj_name = project.get("name", "?") if project else "?"

        print(f"  Relay:  {author} — {proj_name}")
        print(f"  Code:   {code}")
        print()

        store = BlobStore()
        remotes = load_remotes(store)
        # Use hostname as remote name, deduplicate
        name = info.get("hostname", host).lower().replace(" ", "-")
        remotes = [r for r in remotes if r.name != name]
        remotes.append(Remote(name=name, url=base))
        save_remotes(store, remotes)

        # Ensure the project exists locally
        proj_data = store.get_index(proj_name)
        if not proj_data:
            proj_data = ClavusProject(
                name=proj_name,
                root_als="",
                head=None,
                created_at=time.time(),
                description=f"Joined from {author}",
            )
            store.set_index(proj_data)

        # Pull project data
        print(f"  📥 Pulling from '{name}'...")
        result = pull_from_remote(store, proj_data, Remote(name=name, url=base))
        if result.get("error"):
            print(f"  ❌ {result['error']}")
        else:
            print(f"  ✅ {result['cues']} cues, {result['snapshots']} snapshots")
            blob_count = pull_snapshot_blobs(store, proj_data, Remote(name=name, url=base))
            if blob_count:
                print(f"  📦 {blob_count} blob(s) downloaded")
        print()
        print(f"  Done. Run 'clavus log' to see history.")
        return

    # ── Share code / auto-discovery (existing path) ──────────────────
    from clavus.discovery import scan_for_share_codes

    timeout = args.timeout

    # Determine scan modes
    scan_tailscale = not args.lan  # Default: on. Off if --lan is set.
    scan_lan = not args.tailscale  # Default: on. Off if --tailscale is set.

    if not scan_tailscale and not scan_lan:
        print("❌ Can't use both --lan and --tailscale (that's the default)")
        return

    scan_label = []
    if scan_lan: scan_label.append("LAN")
    if scan_tailscale: scan_label.append("Tailscale")

    if args.code:
        print(f"🔍 Looking for relay with code '{args.code}'...")
    else:
        print(f"🔍 Scanning for Clavus share sessions...")

    peers = scan_for_share_codes(
        timeout=timeout,
        scan_tailscale=scan_tailscale,
        scan_lan=scan_lan,
    )

    if not peers:
        print()
        print("  No Clavus relays found.")
        print()
        print("  Make sure:")
        print("    • The other person is running 'clavus share'")
        print("    • You're on the same WiFi (LAN)")
        print("    • Or you're both connected to Tailscale")
        print()
        print("  To scan only one method:")
        print("    clavus join --lan       (scan LAN only)")
        print("    clavus join --tailscale (scan Tailscale only)")
        return

    # Try to get share codes by hitting their /api/share endpoint
    from clavus.sync import SyncClient
    import concurrent.futures

    def _get_share_info(peer):
        try:
            client = SyncClient(f"http://{peer.host}:{peer.port}")
            r = client.client.get(
                f"http://{peer.host}:{peer.port}/api/share",
                timeout=10,
            )
            if r.status_code == 200:
                return r.json()
        except Exception as e:
            # Debug: log the failure silently for now
            _ = e
        return None

    relay_info = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
        fut_map = {pool.submit(_get_share_info, p): p for p in peers}
        for fut in concurrent.futures.as_completed(fut_map, timeout=timeout + 2):
            peer = fut_map[fut]
            try:
                info = fut.result()
                if info and info.get("share_code"):
                    relay_info.append((peer, info))
            except Exception:
                continue

    if not relay_info:
        print()
        print(f"  Found {len(peers)} Clavus server(s), but none in share mode.")
        print()
        print("  The other person needs to run 'clavus share' instead")
        print("  of 'clavus relay'.")
        return

    # Filter by code if specified
    if args.code:
        code_upper = args.code.upper()
        relay_info = [
            (p, info) for p, info in relay_info
            if info.get("share_code", "").upper() == code_upper
        ]
        if not relay_info:
            print(f"  ❌ No relay found with code '{args.code}'")
            return

    print()
    print(f"  Found {len(relay_info)} share session(s):")
    print()

    for peer, info in relay_info:
        code = info.get("share_code", "???")
        author = info.get("author", "?")
        project = info.get("project", {})
        proj_name = project.get("name", "?") if project else "?"
        host = peer.host
        port = peer.port
        print(f"  #{relay_info.index((peer, info)) + 1}")
        print(f"    Code:    {code}")
        print(f"    Host:    {author} — {proj_name}")
        print(f"    URL:     http://{host}:{port}")
        print()

    # Auto-connect if only one found, or if code was specified
    if len(relay_info) == 1 or args.code:
        peer, info = relay_info[0]
        code = info.get("share_code", "???")
        author = info.get("author", "?")
        host = peer.host
        port = peer.port

        print(f"  Connecting to '{author}' ({code})...")

        # Add remote and pull
        from clavus.store import BlobStore
        from clavus.sync import save_remotes, Remote, load_remotes, pull_from_remote, pull_snapshot_blobs

        store = BlobStore()
        name = info.get("hostname", author).lower().replace(" ", "-")

        remotes = load_remotes(store)
        # Remove existing remote with same name
        remotes = [r for r in remotes if r.name != name]
        remotes.append(Remote(name=name, url=f"http://{host}:{port}"))
        save_remotes(store, remotes)
        print(f"  ✅ Remote added: '{name}' (http://{host}:{port})")

        # Try to find and pull into matching project
        projects = store.list_projects()
        proj_name = info.get("project", {}).get("name", "")
        matched_proj = next((p for p in projects if p.name == proj_name), None)

        if matched_proj:
            print(f"  📥 Pulling from '{name}'...")
            result = pull_from_remote(store, matched_proj, Remote(name=name, url=f"http://{host}:{port}"))
            if result.get("cues") or result.get("snapshots"):
                print(f"     Got {result['cues']} cues, {result['snapshots']} snapshots")
            # Pull blobs too
            blob_count = pull_snapshot_blobs(store, matched_proj, Remote(name=name, url=f"http://{host}:{port}"))
            if blob_count:
                print(f"     Downloaded {blob_count} blob(s)")
            print(f"  ✅ Synced with '{author}'")
        else:
            print(f"  💡 No matching local project '{proj_name}' found.")
            print(f"     Run 'clavus pull' after setting up the project.")

        print()
        print(f"  To sync again: clavus push")
        print(f"  For live sync: clavus sync")
    else:
        # Multiple found — ask which one
        print(f"  Multiple sessions found. Connect to one:")
        print(f"    clavus join --code <SHARE-CODE>")
        print(f"  Or ask your friend for their code.")


def cmd_tui(args: argparse.Namespace) -> None:
    """Launch the Textual TUI."""
    try:
        from clavus.tui import run_tui
    except ImportError:
        print("❌ TUI requires textual and httpx.")
        print("   Install with: pip install textual httpx")
        sys.exit(1)
    cfg = ClavusConfig.load()
    url = args.connect or cfg.default_server
    run_tui(url=url)


def cmd_cue(args: argparse.Namespace) -> None:
    """Add a timeline-anchored comment."""
    store, proj = get_store_and_project()
    head = store.read_ref("HEAD")

    position = args.position or "0.0.0"
    # Strip @ prefix if present
    if position.startswith("@"):
        position = position[1:]

    cue = add_cue_command(
        text=args.text,
        position=position,
        track=args.track or "",
        author=args.author or "",
        store=store,
    )

    if cue:
        print(f"💬 Cue added at @{position}")
        print(f"   \"{cue.text}\"")
        print(f"   id: {cue.id}")
        if args.track:
            print(f"   Track: {args.track}")
        if args.author:
            print(f"   Author: {args.author}")
        print(f"   📋 Reply: clavus cue reply {cue.id} \"your response\"")


def cmd_cue_reply(args: argparse.Namespace) -> None:
    """Reply to a cue thread."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    head = store.read_ref("HEAD")

    reply = cues.reply(args.cue_id, args.text, snapshot_hash=head or "")
    if reply:
        print(f"💬 Reply added to cue {args.cue_id}")
        print(f"   \"{args.text}\"")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_resolve(args: argparse.Namespace) -> None:
    """Resolve a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    cue = cues.resolve(args.cue_id, note=args.note or "")
    if cue:
        print(f"✅ Cue {args.cue_id} resolved")
        if args.note:
            print(f"   \"{args.note}\"")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_skip(args: argparse.Namespace) -> None:
    """Skip a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    cue = cues.skip(args.cue_id, reason=args.reason or "")
    if cue:
        print(f"⏭ Cue {args.cue_id} skipped")
        if args.reason:
            print(f"   \"{args.reason}\"")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cues(args: argparse.Namespace) -> None:
    """List all cues."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    filter_ = CueFilter()
    if args.pending_only:
        filter_.status = "pending"
    if args.author:
        filter_.author = args.author

    all_cues = cues.list_cues(filter_)
    print(format_cue_list(all_cues, verbose=args.verbose))


def cmd_config(args: argparse.Namespace) -> None:
    """View or edit clavus configuration."""
    cfg = ClavusConfig.load()

    # ── Set a value ──
    if args.key and args.value is not None:
        valid_keys = {"author", "port", "host", "default_server", "default_project"}
        if args.key not in valid_keys:
            print(f"❌ Unknown setting '{args.key}'.")
            print(f"   Valid keys: {', '.join(sorted(valid_keys))}")
            return
        setattr(cfg, args.key, args.value)
        cfg.save()
        print(f"✅ {args.key} = {args.value}")
        return

    # ── Show single value ──
    if args.key:
        val = getattr(cfg, args.key, "")
        if isinstance(val, str):
            val = f"'{val}'"
        print(f"  {args.key} = {val}")
        return

    # ── Show all ──
    print(f"  Clavus Configuration ({CONFIG_PATH})")
    print()
    for k, v in cfg.to_dict().items():
        print(f"    {k} = {v}")
    print()
    print(f"  Run 'clavus setup' for interactive setup.")
    print(f"  Run 'clavus config <key> <value>' to set a value.")


def cmd_cue_assign(args: argparse.Namespace) -> None:
    """Assign a cue to someone."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.assign(args.cue_id, args.name)
    if result:
        print(f"👤 {args.name} assigned to cue {args.cue_id}")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_unassign(args: argparse.Namespace) -> None:
    """Remove assignee from a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.unassign(args.cue_id)
    if result:
        print(f"👤 Unassigned cue {args.cue_id}")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_start(args: argparse.Namespace) -> None:
    """Mark a cue as in-progress."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.start(args.cue_id)
    if result:
        print(f"▶ Cue {args.cue_id} marked as in-progress")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_stop(args: argparse.Namespace) -> None:
    """Mark a cue as no longer in-progress."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.stop(args.cue_id)
    if result:
        print(f"⏸ Cue {args.cue_id} no longer in-progress")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_delete(args: argparse.Namespace) -> None:
    """Permanently delete a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    if cues.delete(args.cue_id):
        print(f"🗑 Deleted cue {args.cue_id}")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_archive(args: argparse.Namespace) -> None:
    """Archive a specific cue, or all cues."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    if args.cue_id:
        ok = cues.archive(args.cue_id)
        if ok:
            print(f"📦 Archived cue {args.cue_id}")
        else:
            print(f"❌ Cue '{args.cue_id}' not found.")
    else:
        count = cues.archive_resolved()
        print(f"📦 Archived {count} cue(s).")


def cmd_branch(args: argparse.Namespace) -> None:
    """List or create branches."""
    store, proj = get_store_and_project()

    if args.delete:
        # Delete a branch
        branch_ref = f"heads/{args.delete}"
        existing = store.read_ref(branch_ref)
        if not existing:
            print(f"❌ Branch '{args.delete}' not found.")
            return
        if args.delete == proj.branch:
            print(f"❌ Cannot delete the current branch '{args.delete}'.")
            return
        store.delete_ref(branch_ref)
        print(f"🗑 Deleted branch '{args.delete}'")
        return

    if args.list or not args.name:
        # List all branches
        branches = []
        if store.refs_dir.exists():
            for ref_file in store.refs_dir.glob("heads/*"):
                branches.append(ref_file.name)
        if not branches:
            branches = ["main"]
        print(f"🌿 Branches for '{proj.name}'")
        print()
        for b in sorted(branches):
            prefix = "* " if b == proj.branch else "  "
            snap = store.read_ref(f"heads/{b}")
            snap_str = f" ({snap[:8]})" if snap else ""
            print(f"  {prefix}{b}{snap_str}")
        return

    # Create a new branch at current HEAD
    branch_ref = f"heads/{args.name}"
    head = store.read_ref("HEAD")
    if not head:
        print(f"❌ No snapshots yet. Create one first with 'clavus snapshot'.")
        return

    # Check if branch already exists
    existing = store.read_ref(branch_ref)
    if existing:
        print(f"⚠️  Branch '{args.name}' already exists (at {existing[:8]})")
        return

    store.update_ref(branch_ref, head)

    print(f"🌿 Created branch '{args.name}' at {head[:8]}")


def cmd_checkout(args: argparse.Namespace) -> None:
    """Switch branches."""
    store, proj = get_store_and_project()

    if args.b:
        # Create and switch
        head = store.read_ref("HEAD")
        if not head:
            print(f"❌ No snapshots yet.")
            return
        store.update_ref(f"heads/{args.name}", head)

    branch_ref = f"heads/{args.name}"
    branch_head = store.read_ref(branch_ref)
    if not branch_head:
        print(f"❌ Branch '{args.name}' not found.")
        print(f"   Available: {[f.stem for f in store.refs_dir.glob('heads/*')]}")
        return

    # Update project to point to new branch
    proj.branch = args.name
    proj.head = branch_head
    store.update_ref("HEAD", branch_head)
    store.set_index(proj)

    snap = store.load_snapshot(branch_head)
    msg = f" — '{snap.message}'" if snap else ""
    print(f"✅ Switched to branch '{args.name}' at {branch_head[:8]}{msg}")


def cmd_merge(args: argparse.Namespace) -> None:
    """Merge another branch into the current branch."""
    store, proj = get_store_and_project()

    # Get current HEAD
    current_head = store.read_ref("HEAD")
    if not current_head:
        print(f"❌ No snapshots on current branch.")
        return

    # Get the branch to merge
    merge_ref = f"heads/{args.branch}"
    merge_head = store.read_ref(merge_ref)
    if not merge_head:
        print(f"❌ Branch '{args.branch}' not found.")
        return

    if merge_head == current_head:
        print(f"✅ Already up to date — branches are at the same snapshot.")
        return

    current_snap = store.load_snapshot(current_head)
    merge_snap = store.load_snapshot(merge_head)

    # Walk back to find merge base
    def ancestors(hash_str: str) -> list[str]:
        chain = []
        while hash_str:
            chain.append(hash_str)
            snap = store.load_snapshot(hash_str)
            hash_str = snap.parent if snap else None
        return chain

    current_ancestors = ancestors(current_head)
    merge_ancestors = ancestors(merge_head)

    # Find merge base (first common ancestor)
    merge_base = None
    for ca in current_ancestors:
        if ca in merge_ancestors:
            merge_base = ca
            break

    if merge_base is None:
        print(f"⚠️  No common ancestor found. Branches have diverged completely.")
        return

    if merge_base == merge_head:
        # Fast-forward: merge_branch is ancestor of current
        print(f"✅ Already up to date — '{args.branch}' is behind current branch.")
        return

    if merge_base == current_head:
        # Fast-forward: current is ancestor of merge_branch
        if not args.no_ff:
            # Fast-forward: just move HEAD forward
            proj.head = merge_head
            store.update_ref("HEAD", merge_head)
            store.set_index(proj)
            print(f"⏩ Fast-forward merged '{args.branch}' into '{proj.branch}'")
            print(f"   {current_head[:8]}..{merge_head[:8]}")
            return

    # Create a merge commit

    # Parse the current .als for the merge snapshot
    als_path = Path(proj.root_als)
    project = None
    if als_path.exists():
        project = parse_als(als_path)

    if project is None:
        print(f"❌ Could not parse .als file: {proj.root_als}")
        return

    # Save snapshot with TWO parents
    snap = store.save_snapshot(
        project,
        message=args.message or f"Merge branch '{args.branch}' into '{proj.branch}'",
        parent=current_head,
        tags=["merge"],
    )

    # Store the second parent in a sidecar file
    meta_path = store.objects_dir / snap.hash[:2] / f"{snap.hash}.meta"
    if meta_path.exists():
        import json
        data = json.loads(meta_path.read_text())
        data["parent2"] = merge_head
        meta_path.write_text(json.dumps(data, indent=2))

    # Update refs
    proj.head = snap.hash
    store.update_ref("HEAD", snap.hash)
    store.update_ref(f"heads/{proj.branch}", snap.hash)
    store.set_index(proj)

    base_msg = f" ({merge_base[:8]})" if merge_base else ""
    print(f"🔀 Merged '{args.branch}' into '{proj.branch}'")
    print(f"   Merge commit: {snap.short_hash()}")
    print(f"   Base{base_msg} | Parents: {current_head[:8]} + {merge_head[:8]}")


def cmd_remote(args: argparse.Namespace) -> None:
    """Manage remote clavus servers."""
    from clavus.store import BlobStore
    store = BlobStore()
    proj = None

    # Try to load project context, but don't fail if none exists
    try:
        from clavus.helpers import get_store_and_project
        # Suppress stdout to avoid 'no project found' noise
        import io
        old_stdout = sys.stdout
        sys.stdout = io.StringIO()
        try:
            _, proj = get_store_and_project()
        except SystemExit:
            proj = None
        finally:
            sys.stdout = old_stdout
    except Exception:
        proj = None

    remotes = load_remotes(store)

    # ── remote projects <name> — list projects on a remote ──
    if args.action == "projects":
        remote_name = args.name
        if not remote_name:
            print("❌ Specify a remote name: clavus remote projects <name>")
            return
        match = next((r for r in remotes if r.name == remote_name), None)
        if not match:
            print(f"❌ Remote '{remote_name}' not found.")
            return

        from clavus.sync import SyncClient
        client = SyncClient(match.url)
        try:
            r = client.client.get(f"{match.url}/api/projects", timeout=10)
            if r.status_code != 200:
                print(f"❌ Could not reach remote '{remote_name}' at {match.url}")
                return
            data = r.json()
            projects = data.get("projects", [])
            if not projects:
                print(f"📡 No projects on '{remote_name}'")
                return
            print(f"📡 Projects on '{remote_name}' ({match.url}):")
            print()
            for p in projects:
                head_str = f" @ {p.get('head', '?')}" if p.get("head") else " (no snapshots)"
                print(f"  {p.get('name', '?'):<30} branch: {p.get('branch', 'main')}{head_str}")
        finally:
            client.close()
        return

    # ── remote pull <name> [project] — pull a project from remote ──
    if args.action == "pull":
        remote_name = args.name
        remote_project = args.url  # Reusing url as optional project name

        if not remote_name:
            print("❌ Specify a remote name: clavus remote pull <name> [project]")
            return

        match = next((r for r in remotes if r.name == remote_name), None)
        if not match:
            print(f"❌ Remote '{remote_name}' not found.")
            return

        from clavus.sync import SyncClient, pull_from_remote, pull_snapshot_blobs

        client = SyncClient(match.url)

        # If no project specified, list available and pick first
        if not remote_project:
            try:
                r = client.client.get(f"{match.url}/api/projects", timeout=10)
                if r.status_code == 200:
                    projects = r.json().get("projects", [])
                    if not projects:
                        print(f"📡 No projects on '{remote_name}'")
                        return
                    if len(projects) == 1:
                        remote_project = projects[0]["name"]
                    else:
                        print(f"📡 Multiple projects on '{remote_name}':")
                        for p in projects:
                            print(f"  {p.get('name', '?')}")
                        print()
                        print(f"  Specify one: clavus remote pull {remote_name} <name>")
                        return
            except Exception:
                pass

        if not remote_project:
            print("❌ No project specified and couldn't auto-detect.")
            return

        print(f"📥 Pulling project '{remote_project}' from '{remote_name}'...")

        # Check if we already have this project locally
        existing = store.get_index(remote_project)

        if not existing:
            # Get project info from remote to init locally
            try:
                r = client.client.get(
                    f"{match.url}/api/sync/pull",
                    params={"name": remote_project},
                    timeout=30,
                )
                if r.status_code != 200:
                    print(f"❌ Project '{remote_project}' not found on remote.")
                    return
                data = r.json()
                remote_info = data.get("project", {})

                # Auto-init a local project entry
                from clavus.store import ClavusProject
                new_proj = ClavusProject(
                    name=remote_project,
                    root_als=remote_info.get("root_als", f"~/{remote_project}/{remote_project}.als"),
                    created_at=time.time(),
                )
                store.set_index(new_proj)
                print(f"   Created local project '{remote_project}'")

                # Switch to the new project
                proj = new_proj
            except Exception as e:
                print(f"❌ Failed to get remote project info: {e}")
                return
        else:
            # Switch to existing project
            store.set_index(existing)
            proj = existing

        print(f"   Syncing data...")

        # Pull cues + snapshots + blobs
        remote_ref = Remote(name=match.name, url=match.url)
        result = pull_from_remote(store, proj, remote_ref)
        parts = []
        if result.get("cues"):
            parts.append(f"{result['cues']} cues")
        if result.get("snapshots"):
            parts.append(f"{result['snapshots']} snapshots")

        blob_count = pull_snapshot_blobs(store, proj, remote_ref)
        if blob_count:
            parts.append(f"{blob_count} blob(s)")

        if parts:
            print(f"   Got {', '.join(parts)}")
        else:
            print(f"   Already up to date")

        print(f"✅ Synced '{remote_project}' from '{remote_name}'")
        print(f"   Switch to it: clavus project '{remote_project}'")
        print(f"   Pull again:   clavus remote pull {remote_name} {remote_project}")
        client.close()
        return

    # ── Existing behavior: add / remove / list ──
    name = args.add or (args.name if args.action == "add" else "")
    remove_name = args.remove or (args.name if args.action == "remove" else "")

    if name:
        # Add a remote
        for r in remotes:
            if r.name == name:
                print(f"⚠️  Remote '{name}' already exists")
                return
        url = args.url or f"http://{name}.local:7890"
        remotes.append(Remote(name=name, url=url))
        save_remotes(store, remotes)
        print(f"🌐 Added remote '{name}' → {url}")
        return

    if remove_name:
        remotes = [r for r in remotes if r.name != remove_name]
        save_remotes(store, remotes)
        print(f"🗑 Removed remote '{remove_name}'")
        return

    # List remotes
    if not remotes:
        print(f"📡 No remotes configured.")
        print(f"   Use 'clavus remote add <name> <url>' to add one.")
        return

    print(f"📡 Remotes for '{proj.name}'")
    print()
    for r in remotes:
        last_sync = time.strftime("%m/%d %H:%M", time.localtime(r.last_sync)) if r.last_sync else "never"
        print(f"  {r.name:<20} {r.url}")
        print(f"  {'':<20} last sync: {last_sync}")
        print()


def cmd_find(args: argparse.Namespace) -> None:
    """Discover Clavus servers on the LAN or Tailscale tailnet."""
    peers = []

    if args.tailscale:
        try:
            from clavus.discovery import discover_tailscale_peers
            print(f"📡 Scanning your Tailscale tailnet for Clavus servers ({args.timeout}s)...")
            peers = discover_tailscale_peers(timeout=args.timeout)
            if not peers:
                print()
                print("  No Clavus servers found on Tailscale.")
                print()
                print("  Make sure you're connected to Tailscale and your friends")
                print("  are running 'clavus relay'.")
                return
        except ImportError:
            peers = []
    else:
        try:
            from clavus.discovery import discover_peers
        except ImportError:
            print("❌ LAN discovery requires zeroconf. Install: pip install zeroconf")
            return

        print(f"📡 Scanning for Clavus servers on LAN ({args.timeout}s)...")
        peers = discover_peers(timeout=args.timeout)

    if not peers:
        if args.tailscale:
            print()
            print("  No Clavus servers found on Tailscale.")
            print()
            print("  Make sure you're connected to Tailscale and your friends")
            print("  are running 'clavus relay'.")
        else:
            print("  No Clavus servers found.")
            print()
            print("  Make sure you or a friend is running 'clavus relay'.")
            print("  Clavus advertises via mDNS (Bonjour) on the local network.")
            print("  Both machines must be on the same subnet.")
        return

    print(f"  Found {len(peers)} Clavus server(s):")
    print()
    for peer in peers:
        host = peer.host or "?"
        port = peer.port or 7890
        proj = peer.project or "unknown project"
        user = f" [{peer.user}]" if peer.user else ""
        print(f"  {peer.name:<20} {host:>15}:{port:<5}  {proj}{user}")

    print()
    print("  To connect:")
    for peer in peers:
        print(f"    clavus remote add {peer.name} http://{peer.host}:{peer.port}")
    print()
    print("    Or launch the TUI pointing at one:")
    for peer in peers:
        print(f"    clavus tui --connect http://{peer.host}:{peer.port}")

    # Auto-pair if requested
    if args.pair:
        matches = [p for p in peers if p.name.lower() == args.pair.lower()]
        if not matches:
            print(f"  ❌ No server found with hostname '{args.pair}'")
            return
        peer = matches[0]
        from clavus.store import BlobStore
        from clavus.sync import save_remotes, Remote, load_remotes
        store = BlobStore()
        remotes = load_remotes(store)
        for r in remotes:
            if r.name == peer.name:
                print(f"  ⚠️  Remote '{peer.name}' already exists")
                return
        remotes.append(Remote(name=peer.name, url=peer.url))
        save_remotes(store, remotes)
        print(f"  ✅ Paired with '{peer.name}' → {peer.url}")
    print()

def cmd_push(args: argparse.Namespace) -> None:
    """Push cues and snapshots to remotes."""
    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        print(f"   Use 'clavus remote add <name> <url>' first.")
        return

    # If a remote name is specified, only push to that one
    if args.remote:
        remotes = [r for r in remotes if r.name == args.remote]
        if not remotes:
            print(f"❌ Remote '{args.remote}' not found.")
            return

    for remote in remotes:
        print(f"📤 Pushing to '{remote.name}' ({remote.url})...")
        result = push_to_remote(store, proj, remote)
        parts = []
        if result["error"]:
            parts = [f"❌ {result['error']}"]
        else:
            parts = [f"✅ {result['cues']} cues, {result['snapshots']} snapshots"]

            # Push snapshot content blobs + .als backups
            from clavus.sync import push_snapshot_blobs
            blob_count = push_snapshot_blobs(store, proj, remote)
            if blob_count:
                parts.append(f"{blob_count} blob{'s' if blob_count != 1 else ''}")

            # Push stems for current HEAD
            head = store.read_ref("HEAD")
            stem_store = StemStore(proj.name, store)
            if head and stem_store.get_manifest(head):
                from clavus.sync import push_stems_to_remote
                stem_count = push_stems_to_remote(store, proj, remote, stem_store, head)
                parts.append(f"{stem_count} stem{'s' if stem_count != 1 else ''}")
        print(f"  {' — '.join(parts)}")
        print()


def cmd_pull(args: argparse.Namespace) -> None:
    """Pull cues and snapshots from remotes."""
    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        print(f"   Add one with: clavus remote add <name> <url>")
        return

    # If a remote name is specified, only pull from that one
    if args.remote:
        remotes = [r for r in remotes if r.name == args.remote]
        if not remotes:
            print(f"❌ Remote '{args.remote}' not found.")
            return

    for remote in remotes:
        print(f"📥 Pulling from '{remote.name}' ({remote.url})...")
        result = pull_from_remote(store, proj, remote, output_dir=args.output)
        parts = []
        if result["error"]:
            parts = [f"❌ {result['error']}"]
        else:
            parts = [f"✅ {result['cues']} cues, {result['snapshots']} snapshots"]

            # Pull snapshot content blobs + .als backups
            from clavus.sync import pull_snapshot_blobs
            blob_count = pull_snapshot_blobs(store, proj, remote)
            if blob_count:
                parts.append(f"{blob_count} blob{'s' if blob_count != 1 else ''}")

            # Pull stems for current HEAD
            head = store.read_ref("HEAD")
            if head:
                from clavus.sync import pull_stems_from_remote
                stem_count = pull_stems_from_remote(store, proj, remote)
                if stem_count:
                    parts.append(f"{stem_count} stem{'s' if stem_count != 1 else ''}")
                    # Materialize after pull
                    stem_store = StemStore(proj.name, store)
                    manifest = stem_store.get_manifest(head)
                    if manifest and manifest.stems:
                        paths = stem_store.materialize_stems(head)
                        parts.append(f"materialized {len(paths)} file{'s' if len(paths) != 1 else ''}")
        print(f"  {' — '.join(parts)}")
        print()


def cmd_sync(args: argparse.Namespace) -> None:
    """Start the auto-sync daemon."""
    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        print(f"   Use 'clavus remote add <name> <url>' first.")
        print(f"   Then 'clavus push' to sync manually, or 'clavus sync' for auto.")
        return

    daemon = SyncDaemon(store, proj, interval=args.interval)
    daemon.start()

    print()
    print(f"  Press Ctrl+C to stop.")
    print()
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        daemon.stop()


def cmd_cue_render(args: argparse.Namespace) -> None:
    """Export unresolved cues as Ableton markers — either to file or injected into the .als."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    unresolved = cues.list_cues(CueFilter(status="pending"))
    if not unresolved:
        print("💬 No unresolved cues to render.")
        return

    if args.inject:
        # Inject directly into the project's .als file
        render_cues_as_markers(unresolved, "", inject_into_als=proj.root_als)
    else:
        output = args.output or f"{proj.name}_cues.xml"
        render_cues_as_markers(unresolved, output)
        print(f"📍 Rendered {len(unresolved)} cues to {output}")
        print(f"   Import into Ableton by merging <CuePoints> into your .als file.")


# ─── Snapshot Restore ─────────────────────────────────────────────────────

def cmd_restore(args: argparse.Namespace) -> None:
    """Restore a snapshot's .als file from the stored raw bytes."""
    store, proj = get_store_and_project()

    if args.hash:
        hash_str = resolve_snapshot(store, args.hash)
        if not hash_str:
            print(f"❌ Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = store.read_ref("HEAD")
        if not hash_str:
            print("❌ No snapshots to restore from.")
            sys.exit(1)

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"❌ Snapshot not found: {hash_str}")
        sys.exit(1)

    if not snap.als_hash:
        print(f"❌ Snapshot {snap.short_hash()} has no raw .als backup.")
        print("   Only snapshots created *after* the restore feature was built")
        print("   store raw .als data. Create a fresh snapshot first.")
        sys.exit(1)

    raw_als = store.get_object(snap.als_hash)
    if not raw_als:
        print(f"❌ Raw .als data missing for snapshot {snap.short_hash()}.")
        print(f"   Looked for blob: {snap.als_hash[:16]}...")
        sys.exit(1)

    als_path = Path(proj.root_als)
    if not als_path.exists():
        print(f"❌ Project .als file not found at {als_path}")
        sys.exit(1)

    # Confirmation
    snap_time = time.strftime("%Y-%m-%d %H:%M", time.localtime(snap.timestamp))
    print(f"📋 Restore snapshot: {snap.short_hash()}")
    print(f"   Message: {snap.message}")
    print(f"   Captured: {snap_time}")
    print(f"   Tracks: {snap.track_count}  BPM: {snap.bpm}")
    print(f"   Will overwrite: {als_path}")
    print()
    print(f"⚠️  This will overwrite your current .als file. A backup will be saved as:")
    print(f"   {als_path}.bak (if one doesn't already exist)")
    print()
    if not args.yes:
        confirm = input("  Continue? (y/N): ").strip().lower()
        if confirm != "y":
            print("❌ Restore cancelled.")
            return

    # Backup existing .als (only first time)
    bak_path = als_path.with_suffix(".als.bak")
    if not bak_path.exists():
        bak_path.write_bytes(als_path.read_bytes())
        print(f"   💾 Current .als backed up to: {bak_path}")

    # Write the restored .als
    als_path.write_bytes(raw_als)
    print(f"✅ Restored {als_path.name} to snapshot {snap.short_hash()}")
    print(f"   '{snap.message}' — from {snap_time}")

    # Update HEAD to point at this snapshot
    store.update_ref("HEAD", hash_str)
    proj.head = hash_str
    store.set_index(proj)
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
            print(f"❌ Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = proj.head
        if not hash_str:
            print("❌ No snapshots. Push from another machine or take a snapshot first.")
            sys.exit(1)

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"❌ Snapshot not found: {hash_str}")
        sys.exit(1)

    if not snap.als_hash:
        print(f"❌ Snapshot {snap.short_hash()} has no .als backup.")
        sys.exit(1)

    raw_als = store.get_object(snap.als_hash)
    if not raw_als:
        print(f"❌ .als blob missing. Try pulling again to fetch it.")
        sys.exit(1)

    # Determine output path — create a proper Ableton project folder
    project_name = proj.name.replace(" ", " ")
    if args.output:
        out_path = Path(args.output)
    else:
        project_dir = get_projects_dir() / project_name
        out_path = project_dir / f"{project_name}.als"

    out_path.parent.mkdir(parents=True, exist_ok=True)

    # Materialize audio samples into the project folder first (so they exist)
    sample_written = 0
    if snap.sample_hashes:
        base_dir = out_path.parent  # Project folder root
        for sh in snap.sample_hashes:
            fname = store.get_sample_filename(sh)
            relpath = store.get_sample_relpath(sh) or ""
            if fname and store.has_object(sh):
                try:
                    store.materialize_sample(sh, base_dir, fname, relpath)
                    sample_written += 1
                except Exception:
                    pass

    # Rewrite .als sample paths to point to local project folder (cross-OS fix)
    from clavus.parser import rewrite_als_sample_paths
    raw_als = rewrite_als_sample_paths(raw_als, out_path.parent)

    out_path.write_bytes(raw_als)
    print(f"✅ {project_name}.als → {out_path}")
    print(f"   Snapshot: {snap.short_hash()} — {snap.message or '(no message)'}")
    print(f"   {snap.track_count} tracks, {snap.bpm} BPM")
    if sample_written:
        print(f"   🎵 {sample_written} audio sample{'s' if sample_written != 1 else ''} → {base_dir}")

    # Launch Ableton
    system = platform.system()
    if system == "Darwin":
        ableton_path = None
        for candidate in Path("/Applications").glob("Ableton Live*.app"):
            ableton_path = str(candidate)
            break
        if ableton_path:
            sp.run(["open", "-a", ableton_path, str(out_path)])
            print(f"   🎹 Launched Ableton Live")
        else:
            sp.run(["open", str(out_path)])
            print(f"   🎹 Opened .als with default app")
    elif system == "Windows":
        sp.run(["start", "", str(out_path)], shell=True)
        print(f"   🎹 Opened .als with default app")
    else:
        print(f"   ℹ️  Open manually: {out_path}")

    # Helpful tip for first-open sample resolution
    if sample_written:
        print()
        print("   💡 If samples show as offline, point Ableton at any one —")
        print("      all others will resolve automatically.")


# ─── Backup / Restore Store ──────────────────────────────────────────────


def cmd_backup(args: argparse.Namespace) -> None:
    """Backup the entire Clavus store (cues, snapshots, refs, config)."""
    from clavus.store import BlobStore
    store = BlobStore()
    archive_path = store.backup_store()
    size_mb = archive_path.stat().st_size / (1024 * 1024)
    print(f"💾 Backup saved: {archive_path}")
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
    print(f"  📦 Available backups:")
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
            print("❌ No backups found. Run 'clavus backup' first.")
            return
        archive_path = backups[0]
        print(f"  Using latest backup: {archive_path.name}")
    else:
        archive_path = Path(args.archive)
    store.restore_store(archive_path)


# ─── Stem Commands ─────────────────────────────────────────────────────


def cmd_stem_import(args: argparse.Namespace) -> None:
    """Import a bounced stem file and register it with the current snapshot."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    head = store.read_ref("HEAD")

    if not head:
        print("❌ No snapshot yet. Run 'clavus snapshot' first.")
        return

    entry = stem_store.store_stem_file(args.file, args.track)
    manifest = stem_store.get_manifest(head) or StemManifest(snapshot_hash=head)

    # Check if this track already has a stem — replace it
    manifest.stems = [s for s in manifest.stems if s.track_name != args.track]
    manifest.stems.append(entry)
    manifest.created_at = time.time()
    stem_store.save_manifest(manifest)

    file_size_mb = entry.size / (1024 * 1024)
    print(f"📦 Imported stem: {entry.track_name} ({entry.file_name})")
    print(f"   Hash:   {entry.hash[:12]}")
    print(f"   Size:   {file_size_mb:.1f} MB")
    print(f"   Format: {entry.format} / {entry.sample_rate}Hz / {entry.bit_depth}bit / {entry.channels}ch")
    if entry.duration_seconds:
        print(f"   Length: {entry.duration_seconds:.1f}s")
    print(f"   Snapshot: {head[:12]}")


def cmd_stem_list(args: argparse.Namespace) -> None:
    """List stems for the current (or specified) snapshot."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    head = args.snapshot or store.read_ref("HEAD")

    if not head:
        print("❌ No snapshot to show stems for.")
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
        print("❌ No remotes configured. Use 'clavus remote add' first.")
        return

    head = store.read_ref("HEAD")
    if not head:
        print("❌ No snapshot yet.")
        return

    # Fetch stems from remotes via the sync extension
    from clavus.sync import pull_stems_from_remote
    total = 0
    for remote in remotes:
        print(f"  Pulling stems from '{remote.name}'...")
        count = pull_stems_from_remote(store, proj, remote)
        total += count
        if count:
            print(f"    Downloaded {count} stem(s)")
        else:
            print(f"    No new stems")

    # Materialize working tree
    manifest = stem_store.get_manifest(head)
    if manifest and manifest.stems:
        paths = stem_store.materialize_stems(head)
        print(f"\n  Materialized {len(paths)} stem(s) to ~/.clavus/stems/{proj.name}/{head[:12]}/")
    else:
        print(f"\n  No stem manifest for current snapshot")


def cmd_stem_push(args: argparse.Namespace) -> None:
    """Push stem files to all remotes."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    remotes = load_remotes(store)

    if not remotes:
        print("❌ No remotes configured.")
        return

    head = store.read_ref("HEAD")
    if not head:
        print("❌ No snapshot yet.")
        return

    manifest = stem_store.get_manifest(head)
    if not manifest or not manifest.stems:
        print("No stems to push for current snapshot.")
        return

    from clavus.sync import push_stems_to_remote
    for remote in remotes:
        print(f"  Pushing to '{remote.name}'...")
        count = push_stems_to_remote(store, proj, remote, stem_store, head)
        print(f"    Pushed {count} stem(s)")


# ─── Main Entry Point ──────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="Clavus — snapshot, sync, and collaborate on Ableton Live projects.",
        prog="clavus",
    )
    parser.add_argument("--clavus-dir", help="Override clavus storage directory")
    parser.add_argument("--version", action="store_true", help="Show version and exit")
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Init
    p_init = subparsers.add_parser("init", help="Initialize a new clavus project")
    p_init.add_argument("path", nargs="?", default=None, help="Path to .als file or project directory")

    # Projects (list)
    subparsers.add_parser("projects", help="List all tracked projects")

    # Project (switch active)
    p_project = subparsers.add_parser("project", help="Switch active project")
    p_project.add_argument("name", help="Project name to switch to")

    # Snapshot
    p_snap = subparsers.add_parser("snapshot", help="Create a new snapshot")
    p_snap.add_argument("message", nargs="?", default="",
                        help="Description of what changed (prompts if omitted)")
    p_snap.add_argument("--tag", "-t", default="", help="Comma-separated tags")
    p_snap.add_argument("--parent", "-p", default=None, help="Override parent snapshot hash")
    p_snap.add_argument("--verbose", "-v", action="store_true", help="Show detailed diff")

    # Log
    p_log = subparsers.add_parser("log", help="Show snapshot history")
    p_log.add_argument("--limit", "-n", type=int, default=20, help="Max snapshots to show")
    p_log.add_argument("--verbose", "-v", action="store_true", help="Show detailed info per snapshot")
    p_log.add_argument("--graph", action="store_true", help="Show branch topology")

    # Restore
    p_restore = subparsers.add_parser("restore", help="Restore .als from a snapshot")
    p_restore.add_argument("hash", nargs="?", default=None,
                          help="Snapshot hash or ref name (default: HEAD)")
    p_restore.add_argument("-y", "--yes", action="store_true",
                          help="Skip confirmation prompt")

    # Open
    p_open = subparsers.add_parser("open", help="Open latest .als in Ableton Live")
    p_open.add_argument("hash", nargs="?", default=None,
                       help="Snapshot hash (default: HEAD/latest)")
    p_open.add_argument("--output", "-o", default=None,
                       help="Output path (default: Desktop/<Project>.als)")

    # Store backup
    p_backup = subparsers.add_parser("backup", help="Backup the entire Clavus store")
    p_list_backups = subparsers.add_parser("backups", help="List available store backups")
    p_restore_store = subparsers.add_parser("restore-store", help="Restore Clavus store from a backup")
    p_restore_store.add_argument("archive", nargs="?", default=None,
                                help="Path to backup archive (default: latest)")

    # Diff
    p_diff = subparsers.add_parser("diff", help="Show changes in a snapshot")
    p_diff.add_argument("hash", nargs="?", default=None, help="Snapshot hash or ref name (default: HEAD)")
    p_diff.add_argument("--verbose", "-v", action="store_true", help="Show unchanged tracks")
    p_diff.add_argument("--visual", action="store_true", help="Show visual timeline diff")

    # Status
    subparsers.add_parser("status", help="Show current project status")

    # Cues
    p_cue = subparsers.add_parser("cue", help="Add a timeline-anchored comment")
    p_cue.add_argument("text", help="The comment text")
    p_cue.add_argument("position", nargs="?", default="0.0.0", help="Position (e.g. @1:23 or 4.1.1)")
    p_cue.add_argument("--track", "-t", default="", help="Which track this cue is about")
    p_cue.add_argument("--author", "-a", default="", help="Override author name")

    p_cue_reply = subparsers.add_parser("cue-reply", help="Reply to a cue thread")
    p_cue_reply.add_argument("cue_id", help="ID of the cue to reply to")
    p_cue_reply.add_argument("text", help="Reply text")

    p_cue_resolve = subparsers.add_parser("cue-resolve", help="Mark a cue as resolved")
    p_cue_resolve.add_argument("cue_id", help="ID of the cue to resolve")
    p_cue_resolve.add_argument("--note", "-n", default="", help="Optional resolution note")

    p_cue_skip = subparsers.add_parser("cue-skip", help="Skip a cue without resolving")
    p_cue_skip.add_argument("cue_id", help="ID of the cue to skip")
    p_cue_skip.add_argument("--reason", "-r", default="", help="Why it was skipped")

    p_cues = subparsers.add_parser("cues", help="List all cues")
    p_cues.add_argument("--pending", "-p", dest="pending_only", action="store_true",
                       help="Show only pending (unresolved) cues")
    p_cues.add_argument("--author", "-a", default="", help="Filter by author")
    p_cues.add_argument("--verbose", "-v", action="store_true", help="Show replies and resolved cues")

    p_cue_assign = subparsers.add_parser("cue-assign", help="Assign a cue to someone")
    p_cue_assign.add_argument("cue_id", help="ID of the cue to assign")
    p_cue_assign.add_argument("name", help="Name of the person to assign to")

    p_cue_unassign = subparsers.add_parser("cue-unassign", help="Remove assignee from a cue")
    p_cue_unassign.add_argument("cue_id", help="ID of the cue to unassign")

    p_cue_start = subparsers.add_parser("cue-start", help="Mark a cue as in-progress")
    p_cue_start.add_argument("cue_id", help="ID of the cue to start")

    p_cue_stop = subparsers.add_parser("cue-stop", help="Mark a cue as no longer in-progress")
    p_cue_stop.add_argument("cue_id", help="ID of the cue to stop")

    p_cue_delete = subparsers.add_parser("cue-delete", help="Permanently delete a cue")
    p_cue_delete.add_argument("cue_id", help="ID of the cue to delete")

    p_cue_archive = subparsers.add_parser("cue-archive", help="Archive a resolved/skipped cue")
    p_cue_archive.add_argument("cue_id", nargs="?", default="", help="ID of the cue to archive (omit to archive all resolved/skipped)")

    # Config
    p_config = subparsers.add_parser("config", help="View or edit configuration")
    p_config.add_argument("key", nargs="?", default="", help="Setting key to view or set")
    p_config.add_argument("value", nargs="?", default=None, help="Value to set (omit to view)")

    # ── Remote / Push / Pull / Sync ──
    p_remote = subparsers.add_parser("remote", help="Manage remote clavus servers")
    p_remote.add_argument("action", nargs="?", choices=["list", "add", "remove", "projects", "pull"], default="list",
                         help="Action: list, add, remove, projects, or pull")
    p_remote.add_argument("name", nargs="?", default="", help="Remote name (and project name for pull)")
    p_remote.add_argument("url", nargs="?", default="", help="Remote URL or project name (for pull)")
    p_remote.add_argument("--add", default="", help=argparse.SUPPRESS)
    p_remote.add_argument("--remove", default="", help=argparse.SUPPRESS)

    p_push = subparsers.add_parser("push", help="Push cues/snapshots to remotes")
    p_push.add_argument("remote", nargs="?", default=None,
                        help="Remote name (default: all)")

    p_pull = subparsers.add_parser("pull", help="Pull cues/snapshots from remotes")
    p_pull.add_argument("remote", nargs="?", default=None,
                        help="Remote name (default: all)")
    p_pull.add_argument("--output", "-o", type=str, default=None,
                        help="Output directory for project folder")

    p_sync = subparsers.add_parser("sync", help="Start auto-sync daemon")
    p_sync.add_argument("--interval", "-i", type=int, default=30,
                        help="Poll interval in seconds (default: 30)")

    # ── Branch / Checkout / Merge ──
    p_branch = subparsers.add_parser("branch", help="List or create branches")
    p_branch.add_argument("name", nargs="?", default=None, help="Branch name to create")
    p_branch.add_argument("--delete", "-d", default="", help="Delete a branch")
    p_branch.add_argument("--list", "-l", action="store_true", help="List all branches")

    p_checkout = subparsers.add_parser("checkout", help="Switch branches")
    p_checkout.add_argument("name", help="Branch to switch to")
    p_checkout.add_argument("-b", action="store_true", help="Create branch then switch")

    p_merge = subparsers.add_parser("merge", help="Merge another branch into current")
    p_merge.add_argument("branch", help="Branch name to merge from")
    p_merge.add_argument("--message", "-m", default="", help="Merge commit message")
    p_merge.add_argument("--no-ff", action="store_true", help="Create a merge commit even if fast-forward")

    p_cue_render = subparsers.add_parser("cue-render", help="Export cues as Ableton markers")
    p_cue_render.add_argument("--output", "-o", default="", help="Output file path")
    p_cue_render.add_argument("--inject", action="store_true",
                             help="Inject cues directly into the project's .als file (creates backup)")

    # Watch
    p_watch = subparsers.add_parser("watch", help="Auto-snapshot on file changes")
    p_watch.add_argument("--cooldown", "-c", type=int, default=30,
                        help="Seconds to wait after last change before snapshotting (default: 30)")
    p_watch.add_argument("--quiet", "-q", action="store_true",
                        help="Suppress detailed output")
    p_watch.add_argument("--once", action="store_true",
                        help="Take one snapshot and exit (useful for cron jobs)")

    # Relay (stripped-down always-on server)
    p_relay = subparsers.add_parser("relay", help="Start the Clavus relay server for collaboration")
    p_relay.add_argument("--host", default=None,
                        help="Host to bind to (default: from config or 0.0.0.0)")
    p_relay.add_argument("--port", "-p", type=int, default=None,
                        help="Port to listen on (default: from config or 7890)")

    # Share (relay + auto-discovery)
    p_share = subparsers.add_parser("share", help="Start a share session — relay + auto-discovery")
    p_share.add_argument("--host", default=None,
                        help="Host to bind to (default: from config or 0.0.0.0)")
    p_share.add_argument("--port", "-p", type=int, default=None,
                        help="Port to listen on (default: from config or 7890)")

    # Join (discover and connect)
    p_join = subparsers.add_parser("join", help="Discover and connect to a Clavus share session")
    p_join.add_argument("--code", "-c", default="",
                       help="Share code to connect to (optional — scans all if omitted)")
    p_join.add_argument("--timeout", "-t", type=int, default=5,
                       help="Seconds to scan for (default: 5)")
    p_join.add_argument("--lan", action="store_true",
                       help="Scan LAN only (default: LAN + Tailscale)")
    p_join.add_argument("--tailscale", action="store_true",
                       help="Scan Tailscale only (default: LAN + Tailscale)")

    # TUI (terminal dashboard)
    p_tui = subparsers.add_parser("tui", help="Launch the TUI dashboard")
    p_tui.add_argument("--connect", "-c", default="",
                       help="Clavus server URL (default: from config or http://localhost:7890)")

    # Find (LAN discovery)
    p_find = subparsers.add_parser("find", help="Find Clavus servers on your LAN or Tailscale tailnet")
    p_find.add_argument("--timeout", "-t", type=int, default=3,
                        help="Seconds to scan for (default: 3)")
    p_find.add_argument("--pair", "-p", default="",
                        help="Auto-pair with a discovered server by hostname")
    p_find.add_argument("--tailscale", action="store_true",
                        help="Scan your Tailscale tailnet instead of LAN")

    # ── Stem subcommands ──
    p_stem = subparsers.add_parser("stem", help="Manage stems (audio exports)")
    stem_sub = p_stem.add_subparsers(dest="stem_action", help="Stem commands")

    p_stem_import = stem_sub.add_parser("import", help="Import a stem file")
    p_stem_import.add_argument("file", help="Path to the stem audio file")
    p_stem_import.add_argument("--track", "-t", required=True,
                               help="Track name (e.g., 'Kick', 'Bass', 'Vocal')")

    p_stem_list = stem_sub.add_parser("list", help="List stems for a snapshot")
    p_stem_list.add_argument("--snapshot", "-s", default="",
                             help="Snapshot hash (default: current HEAD)")

    p_stem_push = stem_sub.add_parser("push", help="Push stem files to remotes")

    p_stem_pull = stem_sub.add_parser("pull", help="Pull stem files from remotes")

    # Repair (recover from corrupt/missing index)
    p_doctor = subparsers.add_parser("doctor", help="Diagnose Clavus store health (read-only)")
    p_doctor.add_argument("--verbose", "-v", action="store_true", help="Show detailed info")
    p_setup = subparsers.add_parser("setup", help="Guided first-run setup")
    p_repair = subparsers.add_parser("repair", help="Repair Clavus storage — recover projects from backup, cues, and refs")
    p_repair.add_argument("--force", "-f", action="store_true",
                          help="Force repair even if index.json exists")
    p_repair.add_argument("--set-als", type=str, default="",
                          help="Set .als path for recovered projects (name=/path format or 'all=/path')")

    args = parser.parse_args()

    if args.version:
        try:
            from importlib.metadata import version
            v = version("clavus")
        except ImportError:
            v = "0.7.0-beta"
        print(f"clavus {v}")
        return

    # Override clavus directory if specified
    if args.clavus_dir:
        DEFAULT_CLAVUS_DIR = Path(args.clavus_dir)

    # Dispatch
    commands = {
        "init": cmd_init,
        "projects": cmd_projects,
        "project": cmd_project,
        "snapshot": cmd_snapshot,
        "log": cmd_log,
        "diff": cmd_diff,
        "status": cmd_status,
        "watch": cmd_watch,
        "relay": cmd_relay,
        "share": cmd_share,
        "join": cmd_join,
        "tui": cmd_tui,
        "branch": cmd_branch,
        "checkout": cmd_checkout,
        "remote": cmd_remote,
        "doctor": cmd_doctor,
        "setup": cmd_setup,
        "repair": cmd_repair,
        "push": cmd_push,
        "pull": cmd_pull,
        "sync": cmd_sync,
        "merge": cmd_merge,
        "restore": cmd_restore,
        "open": cmd_open,
        "backup": cmd_backup,
        "backups": cmd_list_backups,
        "restore-store": cmd_restore_store,
        "cue": cmd_cue,
        "cue-reply": cmd_cue_reply,
        "cue-resolve": cmd_cue_resolve,
        "cue-skip": cmd_cue_skip,
        "cues": cmd_cues,
        "cue-render": cmd_cue_render,
        "cue-assign": cmd_cue_assign,
        "cue-unassign": cmd_cue_unassign,
        "cue-start": cmd_cue_start,
        "cue-stop": cmd_cue_stop,
        "cue-delete": cmd_cue_delete,
        "cue-archive": cmd_cue_archive,
        "config": cmd_config,
        "find": cmd_find,
    }

    if args.command in commands:
        commands[args.command](args)
    elif args.command == "stem":
        stem_actions = {
            "import": cmd_stem_import,
            "list": cmd_stem_list,
            "push": cmd_stem_push,
            "pull": cmd_stem_pull,
        }
        if args.stem_action in stem_actions:
            stem_actions[args.stem_action](args)
        else:
            print("Usage: clavus stem {import|list|push|pull}")
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
