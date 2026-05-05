"""
Clavus CLI — the primary user interface.

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
import os
import sys
import time
from pathlib import Path
from typing import Optional

from clavus import parse_als, project_summary
from clavus.config import ClavusConfig
from clavus.store import (
    BlobStore, ClavusProject, diff_projects, format_diff,
    DEFAULT_CLAVUS_DIR,
)
from clavus.cues import (
    CueStore, CueFilter, format_cue, format_cue_list,
    render_cues_as_markers, add_cue_command,
)
from clavus.helpers import find_als_file, get_store_and_project, resolve_snapshot
from clavus.watch import watch as cmd_watch_daemon
from clavus.store import (
    StemStore, StemEntry, StemManifest,
)
from clavus.sync import (
    load_remotes, save_remotes, Remote, push_to_remote, pull_from_remote, SyncDaemon,
)


# Optional web server
try:
    from clavus.web import run_web_server
    _HAS_WEB = True
except ImportError:
    _HAS_WEB = False

# ─── Commands ──────────────────────────────────────────────────────────

def cmd_init(args: argparse.Namespace) -> None:
    """Initialize a new Clavus project."""
    target = args.path or os.getcwd()
    target = Path(target).resolve()

    als_path = find_als_file(target)
    if als_path is None:
        print(f"❌ No .als file found at {target}")
        print("   Specify the path to an .als file or a directory containing one.")
        sys.exit(1)

    store = BlobStore()
    store.init()

    # Check if already initialized
    existing = store.get_index(als_path.stem)
    if existing:
        print(f"⚠️  Project '{als_path.stem}' already tracked at {existing.root_als}")
        return

    # Parse the .als to get initial info
    project = parse_als(als_path)
    clavus_proj = ClavusProject(
        name=als_path.stem,
        root_als=str(als_path),
        created_at=time.time(),
    )

    # Create initial snapshot
    snap = store.save_snapshot(project, "Initial import", parent=None)
    clavus_proj.head = snap.hash
    store.update_ref("HEAD", snap.hash)
    store.update_ref(f"refs/tags/initial", snap.hash)

    # Save to index
    store.set_index(clavus_proj)

    print(f"📁 Initialized Clavus project '{clavus_proj.name}'")
    print(f"   .als: {als_path}")
    print(f"   Tracking {project.track_count} tracks @ {project.bpm}bpm")
    print(f"   Created snapshot: {snap.short_hash()}")
    print(f"")  # blank line for readability
    print(f"   Next: open the project, make changes, then run:")
    print(f"   clavus snapshot \"your message here\"")


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

    # Parse current state
    project = parse_als(als_path)

    # Create snapshot
    snap = store.save_snapshot(
        project,
        message=args.message,
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
    store, proj = _get_store_and_project()
    cmd_watch_daemon(
        store,
        proj,
        cooldown=args.cooldown,
        verbose=not args.quiet,
        once=args.once,
    )


def cmd_serve(args: argparse.Namespace) -> None:
    """Start the web companion server."""
    try:
        from clavus.web import run_web_server
    except ImportError:
        print("❌ Web companion requires fastapi and uvicorn.")
        print("   Install with: pip install fastapi uvicorn")
        sys.exit(1)
    cfg = ClavusConfig.load()
    host = args.host or cfg.host
    port = args.port or cfg.port
    run_web_server(host=host, port=port)


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
        store=store,
    )

    if cue:
        print(f"💬 Cue added at @{position}")
        print(f"   \"{cue.text}\"")
        print(f"   id: {cue.id}")
        if args.track:
            print(f"   Track: {args.track}")
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
    if args.key and args.value is not None:
        # Set a value
        valid_keys = {"author", "port", "host", "default_server", "default_project"}
        if args.key not in valid_keys:
            print(f"❌ Unknown setting '{args.key}'.")
            print(f"   Valid keys: {', '.join(sorted(valid_keys))}")
            return
        setattr(cfg, args.key, args.value)
        cfg.save()
        print(f"✅ {args.key} = {args.value}")
    elif args.key:
        # Show single value
        val = getattr(cfg, args.key, "")
        print(f"{args.key} = {val}")
    else:
        # Show all
        for k, v in cfg.to_dict().items():
            print(f"  {k} = {v}")


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
    """Archive a specific cue or all resolved/skipped cues."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    if args.cue_id:
        dst = cues.archive(args.cue_id)
        if dst:
            print(f"📦 Archived cue {args.cue_id}")
        else:
            print(f"❌ Cue '{args.cue_id}' not found or not resolved/skipped.")
    else:
        count = cues.archive_resolved()
        print(f"📦 Archived {count} resolved/skipped cue(s).")


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
        message=merge_message,
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
    store, proj = get_store_and_project()
    remotes = load_remotes(store)

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
                print("  are running 'clavus serve'.")
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
            print("  are running 'clavus serve'.")
        else:
            print("  No Clavus servers found.")
            print()
            print("  Make sure you or a friend is running 'clavus serve'.")
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
    """Push cues and snapshots to all remotes."""
    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        print(f"   Use 'clavus remote add <name> <url>' first.")
        return

    for remote in remotes:
        print(f"📤 Pushing to '{remote.name}' ({remote.url})...")
        result = push_to_remote(store, proj, remote)
        if result["error"]:
            print(f"  ❌ {result['error']}")
        else:
            parts = [f"✅ {result['cues']} cues, {result['snapshots']} snapshots"]

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
    """Pull cues and snapshots from all remotes."""
    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        return

    for remote in remotes:
        print(f"📥 Pulling from '{remote.name}' ({remote.url})...")
        result = pull_from_remote(store, proj, remote)
        if result["error"]:
            print(f"  ❌ {result['error']}")
        else:
            parts = [f"✅ {result['cues']} cues, {result['snapshots']} snapshots"]

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
        description="Clavus — Git for Ableton Live projects.",
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
    p_snap.add_argument("message", help="Description of what changed")
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
    p_remote.add_argument("action", nargs="?", choices=["list", "add", "remove"], default="list",
                         help="Action: list, add, or remove")
    p_remote.add_argument("name", nargs="?", default="", help="Remote name")
    p_remote.add_argument("url", nargs="?", default="", help="Remote URL")
    p_remote.add_argument("--add", default="", help=argparse.SUPPRESS)
    p_remote.add_argument("--remove", default="", help=argparse.SUPPRESS)

    p_push = subparsers.add_parser("push", help="Push to all remotes")

    p_pull = subparsers.add_parser("pull", help="Pull from all remotes")

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

    # Serve (web companion)
    p_serve = subparsers.add_parser("serve", help="Start the web companion UI")
    p_serve.add_argument("--host", default=None,
                        help="Host to bind to (default: from config or 0.0.0.0)")
    p_serve.add_argument("--port", "-p", type=int, default=None,
                        help="Port to listen on (default: from config or 7890)")

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

    args = parser.parse_args()

    if args.version:
        try:
            from importlib.metadata import version
            v = version("clavus")
        except ImportError:
            v = "0.5.0"
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
        "serve": cmd_serve,
        "tui": cmd_tui,
        "branch": cmd_branch,
        "checkout": cmd_checkout,
        "remote": cmd_remote,
        "push": cmd_push,
        "pull": cmd_pull,
        "sync": cmd_sync,
        "merge": cmd_merge,
        "restore": cmd_restore,
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
