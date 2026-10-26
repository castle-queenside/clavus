from __future__ import annotations

"""Clavus CLI commands: branch, checkout, merge."""
import argparse
from pathlib import Path
from clavus.parser import parse_als
from clavus.helpers import get_store_and_project

def cmd_branch(args: argparse.Namespace) -> None:
    """List or create branches."""
    store, proj = get_store_and_project()

    if args.delete:
        # Delete a branch
        branch_ref = f"heads/{args.delete}"
        existing = store.read_ref(branch_ref)
        if not existing:
            print(f"-- Branch '{args.delete}' not found.")
            return
        if args.delete == proj.branch:
            print(f"-- Cannot delete the current branch '{args.delete}'.")
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
        print(f"-- No snapshots yet. Create one first with 'clavus snapshot'.")
        return

    # Check if branch already exists
    existing = store.read_ref(branch_ref)
    if existing:
        print(f"[WARN]  Branch '{args.name}' already exists (at {existing[:8]})")
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
            print(f"-- No snapshots yet.")
            return
        store.update_ref(f"heads/{args.name}", head)

    branch_ref = f"heads/{args.name}"
    branch_head = store.read_ref(branch_ref)
    if not branch_head:
        print(f"-- Branch '{args.name}' not found.")
        print(f"   Available: {[f.stem for f in store.refs_dir.glob('heads/*')]}")
        return

    # Update project to point to new branch
    proj.branch = args.name
    store.update_ref("HEAD", branch_head)
    store.set_project_head(proj, branch_head, source="branch")

    snap = store.load_snapshot(branch_head)
    msg = f" — '{snap.message}'" if snap else ""
    print(f"[ok] Switched to branch '{args.name}' at {branch_head[:8]}{msg}")


def cmd_merge(args: argparse.Namespace) -> None:
    """Merge another branch into the current branch."""
    store, proj = get_store_and_project()

    # Get current HEAD
    current_head = store.read_ref("HEAD")
    if not current_head:
        print(f"-- No snapshots on current branch.")
        return

    # Get the branch to merge
    merge_ref = f"heads/{args.branch}"
    merge_head = store.read_ref(merge_ref)
    if not merge_head:
        print(f"-- Branch '{args.branch}' not found.")
        return

    if merge_head == current_head:
        print(f"[ok] Already up to date — branches are at the same snapshot.")
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
        print(f"[WARN]  No common ancestor found. Branches have diverged completely.")
        return

    if merge_base == merge_head:
        # Fast-forward: merge_branch is ancestor of current
        print(f"[ok] Already up to date — '{args.branch}' is behind current branch.")
        return

    if merge_base == current_head:
        # Fast-forward: current is ancestor of merge_branch
        if not args.no_ff:
            # Fast-forward: just move HEAD forward
            store.update_ref("HEAD", merge_head)
            store.set_project_head(proj, merge_head, source="merge-ff")
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
        print(f"-- Could not parse .als file: {proj.root_als}")
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
    store.update_ref("HEAD", snap.hash)
    store.update_ref(f"heads/{proj.branch}", snap.hash)
    store.set_project_head(proj, snap.hash, source="merge")

    base_msg = f" ({merge_base[:8]})" if merge_base else ""
    print(f"🔀 Merged '{args.branch}' into '{proj.branch}'")
    print(f"   Merge commit: {snap.short_hash()}")
    print(f"   Base{base_msg} | Parents: {current_head[:8]} + {merge_head[:8]}")


