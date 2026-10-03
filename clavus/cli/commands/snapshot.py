from __future__ import annotations

"""Clavus CLI commands: snapshot, create_snapshot, log, note."""
import argparse
from clavus.helpers import get_store_and_project
from clavus.store import BlobStore

def create_snapshot(message: str, allow_frozen: bool = True) -> tuple[Optional[str], list[str]]:
    """Core snapshot logic (non-interactive). Returns (snap_hash, log_lines). 
    
    If allow_frozen=True, skips frozen track prompt. Used by TUI.
    Returns (None, [...]) on failure or no-change.
    """
    logs: list[str] = []
    store, proj = get_store_and_project()
    als_path = Path(proj.root_als) if proj.root_als else None
    if not als_path or not als_path.is_file():
        from clavus.helpers import get_projects_dir
        candidate = get_projects_dir() / proj.name / f"{proj.name}.als"
        if candidate.is_file():
            als_path = candidate
        else:
            logs.append(f"-- .als file not found: {proj.root_als or 'projects/' + proj.name}")
            # Also try the Project subfolder convention
            candidate2 = get_projects_dir() / proj.name / f"{proj.name} Project" / f"{proj.name}.als"
            if candidate2.is_file():
                als_path = candidate2
                logs.append(f"   Found in project subfolder: {candidate2}")
            else:
                return None, logs

    project = parse_als(als_path)
    frozen_count = sum(1 for t in project.tracks if t.is_frozen) if project else 0
    if frozen_count:
        if not allow_frozen:
            logs.append(f"  [WARN]  {frozen_count} frozen track(s) — pass allow_frozen=True to skip")
            return None, logs
        else:
            logs.append(f"  [WARN]  {frozen_count} frozen track(s) — snapshots may not restore on other platforms")

    # Guard: if a snapshot already exists for this .als content, skip.
    # Prevents recycling an old root snapshot and destroying the chain.
    if proj.head:
        raw_als = als_path.read_bytes()
        current_als_hash = hashlib.sha256(raw_als).hexdigest()
        prev = store.load_snapshot(proj.head)
        if prev and prev.als_hash == current_als_hash:
            logs.append(f"[WARN]  No changes — {proj.name}.als is identical to last snapshot")
            return None, logs
        existing_meta = store.objects_dir / current_als_hash[:2] / f"{current_als_hash}.meta"
        if existing_meta.exists():
            existing = store.load_snapshot(current_als_hash)
            if existing:
                logs.append(f"[WARN]  Snapshot {current_als_hash[:10]} already exists for this .als state")
                return existing.hash, logs

    snap = store.save_snapshot(
        project, message=message, parent=proj.head, tags=[],
    )

    # Warn if snapshot has no .als backup — it cannot restore
    if not snap.als_hash:
        logs.append(f"  [WARN]  Snapshot saved but .als file has no backup — restore will not be possible")

    store.update_ref("HEAD", snap.hash)
    store.set_project_head(proj, snap.hash, source="create-snapshot")

    if prev:
        prev_project = store.load_project(prev.hash)
        if prev_project:
            diff = diff_projects(prev_project, project)
            logs.append(f"📸 {snap.short_hash()} — '{snap.message}'")
            logs.append(f"   {diff.summary}")
        else:
            logs.append(f"📸 {snap.short_hash()} — '{snap.message}'")
    else:
        logs.append(f"📸 {snap.short_hash()} — '{snap.message}' ({project.track_count} tracks @ {project.bpm}bpm)")

    return snap.hash, logs


def cmd_snapshot(args: argparse.Namespace) -> None:
    """Create a new snapshot of the current project state."""
    store, proj = get_store_and_project()
    als_path = Path(proj.root_als) if proj.root_als else None
    # Fallback: if root_als is empty or missing, look in projects dir
    if not als_path or not als_path.is_file():
        from clavus.helpers import get_projects_dir
        candidate = get_projects_dir() / proj.name / f"{proj.name}.als"
        if candidate.is_file():
            als_path = candidate
        else:
            print(f"-- .als file not found: {proj.root_als or 'projects/' + proj.name}")
            sys.exit(1)

    # Prompt for message if not provided
    # Support both positional `clavus snapshot "msg"` and `--message`/`-m` flag
    message = args.message_flag if getattr(args, 'message_flag', None) else args.message
    if not message:
        try:
            message = input("  Snapshot message (or blank to cancel): ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\n-- Snapshot cancelled.")
            return
        if not message:
            print("-- Snapshot cancelled.")
            return

    # Parse current state
    project = parse_als(als_path)

    # Check for frozen tracks (cross-platform crash risk)
    frozen_count = sum(1 for t in project.tracks if t.is_frozen) if project else 0
    if frozen_count:
        if args.allow_frozen:
            print(f"  [WARN]  {frozen_count} frozen track(s) — proceeding anyway (--allow-frozen)")
        else:
            print(f"  [WARN]  {frozen_count} frozen track(s) detected — will crash on other platforms.")
            print(f"  Unfreeze tracks in Ableton first for cross-platform compatibility.")
            try:
                choice = input("  Continue anyway? [y/N]: ").strip().lower()
            except (EOFError, KeyboardInterrupt):
                print("\n-- Snapshot cancelled.")
                return
            if choice != 'y':
                print("-- Snapshot cancelled.")
                return

    # Guard: if a snapshot already exists for this .als content, skip.
    # Prevents recycling an old root snapshot and destroying the chain.
    if proj.head:
        raw_als = als_path.read_bytes()
        current_als_hash = hashlib.sha256(raw_als).hexdigest()
        prev = store.load_snapshot(proj.head)
        if prev and prev.als_hash == current_als_hash:
            print(f"[WARN]  No changes detected — project state is identical to last snapshot.")
            print(f"   Current HEAD: {prev.short_hash()} — '{prev.message}'")
            return
        existing_meta = store.objects_dir / current_als_hash[:2] / f"{current_als_hash}.meta"
        if existing_meta.exists():
            existing = store.load_snapshot(current_als_hash)
            if existing:
                print(f"[WARN]  Snapshot {current_als_hash[:10]} already exists for this .als state")
                print(f"   '{existing.message}'")
                return

    # Create snapshot
    notes_arg = getattr(args, 'notes', None) or ""
    snap = store.save_snapshot(
        project,
        message=message,
        parent=proj.head,
        tags=args.tag.split(",") if args.tag else [],
        notes=notes_arg,
    )

    # Update references
    store.update_ref("HEAD", snap.hash)
    store.set_project_head(proj, snap.hash, source="snapshot-cli")

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

    if snap.has_notes():
        notes_preview = snap.notes[:120] + ("..." if len(snap.notes) > 120 else "")
        print(f"   📝 Notes: {notes_preview}")

    print(f"   [note] Run 'clavus log' to see history.")


def cmd_log(args: argparse.Namespace) -> None:
    """Show snapshot history."""
    store, proj = get_store_and_project()
    current_hash = proj.head
    count = 0

    if not current_hash:
        print("[note] No snapshots yet. Run 'clavus snapshot' to create one.")
        return

    # Collect all branch refs for graph display
    branch_refs: dict[str, str] = {"HEAD": current_hash}
    if store.refs_dir.exists():
        for ref_file in store.refs_dir.glob("heads/*"):
            branch_refs[ref_file.name] = ref_file.read_text().strip()

    print(f"[note] Snapshot history for '{proj.branch}'")
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
            if snap.has_notes():
                note_preview = snap.notes[:60].replace("\n", " ")
                print(f"  |    📝 {note_preview}...")

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



def cmd_note(args: argparse.Namespace) -> None:
    """Read, write, or append session notes on a snapshot."""
    store, proj = get_store_and_project()

    # Resolve target snapshot
    target_hash = args.hash or proj.head
    if not target_hash:
        print("-- No snapshot specified and HEAD is empty. Run 'clavus snapshot' first.")
        sys.exit(1)

    snap = store.load_snapshot(target_hash)
    if not snap:
        print(f"-- Snapshot not found: {target_hash[:8]}")
        sys.exit(1)

    action = args.action or "read"

    if action == "read":
        if snap.notes:
            print(f"─── Session Notes ─ {snap.short_hash()} '{snap.message}' ───")
            print(snap.notes)
            print("─" * 50)
        else:
            print(f"📝 No notes for snapshot {snap.short_hash()} — '{snap.message}'")
            print("   Write notes with: clavus note write <text>")
            print("   Or append:        clavus note append <text>")

    elif action in ("write", "append"):
        # Gather note text
        if args.file:
            try:
                note_text = Path(args.file).read_text()
            except Exception as e:
                print(f"-- Could not read file '{args.file}': {e}")
                sys.exit(1)
        elif args.text:
            note_text = " ".join(args.text)
        else:
            # Interactive
            print("Type your note (Ctrl+D / Ctrl+C to finish):")
            try:
                note_text = sys.stdin.read().strip()
            except (EOFError, KeyboardInterrupt):
                print("\n-- Cancelled.")
                return
            if not note_text:
                print("-- Empty note — cancelled.")
                return

        # Apply
        if action == "append" and snap.notes:
            new_notes = snap.notes + "\n\n" + note_text
        else:
            new_notes = note_text

        # Update snapshot metadata in-place
        meta_path = store.objects_dir / snap.hash[:2] / f"{snap.hash}.meta"
        data = json.loads(meta_path.read_text())
        data["notes"] = new_notes
        meta_path.write_text(json.dumps(data, indent=2, default=str))

        # Also update in-memory copy
        snap.notes = new_notes

        preview = new_notes[:100].replace("\n", " ")
        print(f"[ok] Note {'appended to' if action == 'append' else 'written for'} snapshot {snap.short_hash()}")
        print(f"   Preview: {preview}{'...' if len(new_notes) > 100 else ''}")


