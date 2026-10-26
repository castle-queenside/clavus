from __future__ import annotations

"""Clavus CLI commands: projects, project."""
import argparse
import sys
from pathlib import Path
from clavus.store import BlobStore
from clavus.helpers import get_store_and_project

def cmd_projects(args: argparse.Namespace) -> None:
    """List all tracked projects."""
    from rich.console import Console
    from rich.style import Style
    c = Console()
    accent = Style(color="#1a9e9e", bold=True)
    dim    = Style(color="#6a9a9a")
    green  = Style(color="#40cc80")
    red    = Style(color="#ff4444")

    store = BlobStore()
    projects = store.list_projects()

    if not projects:
        print("\n  No Clavus projects found. Run 'clavus init <path>' to add one.\n")
        return

    c.print()
    c.print("  CLAVUS PROJECTS ({})".format(len(projects)), style=accent)
    c.print("  " + "─"*50, style=dim)
    c.print()

    for p in sorted(projects, key=lambda x: x.name.lower()):
        icon = "pk" if not p.shared else "net"
        status = "[ok]" if Path(p.root_als).exists() else "[X]"
        c.print("  {} {}".format(icon, p.name), style=dim)
        c.print("     {}  {}".format(status, p.root_als))

    c.print()
    last = store.read_ref("_last_project") or "none"
    try:
        _, active = get_store_and_project()
        c.print("  Current: {}    Active: {}".format(last, active.name), style=dim)
    except SystemExit:
        c.print("  Current: {}".format(last), style=dim)
    c.print()


def cmd_project(args: argparse.Namespace) -> None:
    """Switch the active project, or toggle sharing."""
    store = BlobStore()
    proj = store.get_index(args.name)
    if not proj:
        print(f"-- Project '{args.name}' not found.")
        print("   Run 'clavus projects' to see available projects.")
        sys.exit(1)

    # Toggle share/private
    if args.share:
        if proj.shared:
            print(f"Project '{args.name}' is already shared.")
        else:
            proj.shared = True
            store.set_index(proj)
            print(f"Project '{args.name}' is now shared — visible to collaborators.")
        return
    if args.private:
        if not proj.shared:
            print(f"Project '{args.name}' is already private.")
        else:
            proj.shared = False
            store.set_index(proj)
            print(f"Project '{args.name}' is now private — hidden from collaborators.")
        return

    store.set_index(proj)
    print(f"[ok] Switched to project '{args.name}'")
    print(f"   Path: {proj.root_als}")
    if proj.head:
        print(f"   HEAD: {proj.head[:8]}")
    else:
        print(f"   (no snapshots yet)")
    print(f"   Branch: {proj.branch}")
    print(f"   {'Shared' if proj.shared else 'Private'}")


