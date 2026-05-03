"""
Clavus CLI — the primary user interface.

Commands:
  clavus init [path]          Initialize + git init
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
        print(f"❌ Snapshot not found: {hash_str}")
        return

    current_project = store.load_project(hash_str)
    if not current_project:
        print(f"❌ Could not load project data for {hash_str}")
        return

    if snap.parent:
        parent_project = store.load_project(snap.parent)
        if parent_project:
            diff = diff_projects(parent_project, current_project)
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
    run_web_server(host=args.host, port=args.port)


def cmd_tui(args: argparse.Namespace) -> None:
    """Launch the Textual TUI."""
    try:
        from clavus.tui import run_tui
    except ImportError:
        print("❌ TUI requires textual and httpx.")
        print("   Install with: pip install textual httpx")
        sys.exit(1)
    run_tui(url=args.connect)


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
            print(f"  ✅ {result['cues']} cues, {result['snapshots']} snapshots")
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
            print(f"  ✅ {result['cues']} cues, {result['snapshots']} snapshots")
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


# ─── Main Entry Point ──────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="Clavus — Git for Ableton Live projects.",
        prog="clavus",
    )
    parser.add_argument("--clavus-dir", help="Override clavus storage directory")
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Init
    p_init = subparsers.add_parser("init", help="Initialize a new clavus project")
    p_init.add_argument("path", nargs="?", default=None, help="Path to .als file or project directory")

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

    # Diff
    p_diff = subparsers.add_parser("diff", help="Show changes in a snapshot")
    p_diff.add_argument("hash", nargs="?", default=None, help="Snapshot hash or ref name (default: HEAD)")
    p_diff.add_argument("--verbose", "-v", action="store_true", help="Show unchanged tracks")

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
    p_serve.add_argument("--host", default="0.0.0.0",
                        help="Host to bind to (default: 0.0.0.0)")
    p_serve.add_argument("--port", "-p", type=int, default=7890,
                        help="Port to listen on (default: 7890)")

    # TUI (terminal dashboard)
    p_tui = subparsers.add_parser("tui", help="Launch the TUI dashboard")
    p_tui.add_argument("--connect", "-c", default="",
                       help="Clavus server URL (default: http://localhost:7890)")

    args = parser.parse_args()

    # Override clavus directory if specified
    if args.clavus_dir:
        DEFAULT_CLAVUS_DIR = Path(args.clavus_dir)

    # Dispatch
    commands = {
        "init": cmd_init,
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
        "cue": cmd_cue,
        "cue-reply": cmd_cue_reply,
        "cue-resolve": cmd_cue_resolve,
        "cue-skip": cmd_cue_skip,
        "cues": cmd_cues,
        "cue-render": cmd_cue_render,
    }

    if args.command in commands:
        commands[args.command](args)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
