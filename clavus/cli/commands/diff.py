from __future__ import annotations

"""Clavus CLI commands: diff, status."""
import argparse
from clavus.helpers import get_store_and_project

def cmd_diff(args: argparse.Namespace) -> None:
    """Show what changed in a snapshot vs its parent."""
    store, proj = get_store_and_project()

    if args.hash:
        hash_str = resolve_snapshot(store, args.hash)
        if not hash_str:
            print(f"-- Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = store.read_ref("HEAD")
        if not hash_str:
            print("[note] No snapshots yet.")
            return

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"-- Snapshot not found (resolved: '{hash_str}', meta: {store.objects_dir / hash_str[:2] / (hash_str + '.meta')})")
        return

    current_project = store.load_project(hash_str)
    if not current_project:
        print(f"-- Could not load project data for {hash_str}")
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
    from rich.console import Console
    from rich.style import Style
    c = Console()
    accent = Style(color="#1a9e9e", bold=True)
    dim    = Style(color="#6a9a9a")
    green  = Style(color="#40cc80")
    red    = Style(color="#ff4444")
    orange = Style(color="#d47030")

    store, proj = get_store_and_project()
    als_path = Path(proj.root_als)
    als_exists = als_path.exists()
    last_snap = store.load_snapshot(proj.head) if proj.head else None

    c.print()
    c.print("  {}".format(proj.name.upper()), style=accent)
    c.print("  " + "─"*50, style=dim)
    c.print()
    c.print("  Path    {}".format(proj.root_als), style=dim)

    if als_exists:
        c.print("  Status  ", end="")
        c.print("[ok] exists", style=green)
    else:
        c.print("  Status  ", end="")
        c.print("[X] missing", style=red)

    if last_snap:
        msg = last_snap.message or "(no message)"
        c.print("  HEAD    {} — '{}'".format(last_snap.short_hash(), msg))
        if als_exists:
            project = store.load_project(last_snap.hash)
            if project:
                diff = diff_projects(project, parse_als(str(als_path)))
                if diff.summary != "No changes":
                    c.print("  Warning ", end="")
                    c.print("[WARN] Unsaved changes — {}".format(diff.summary), style=orange)
                else:
                    c.print("  Sync    ", end="")
                    c.print("[ok] up to date", style=green)
    else:
        c.print("  HEAD    (no snapshots yet)")

    c.print()
    c.print("  Branch  {}".format(proj.branch))
    c.print("  Mode    {}".format("Shared" if proj.shared else "Private"))
    c.print()


