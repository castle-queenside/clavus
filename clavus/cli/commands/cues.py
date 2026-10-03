from __future__ import annotations

"""Clavus CLI commands: cue, cue-reply, cue-resolve, cue-skip, cues, etc."""
import argparse
from clavus.cues import CueStore
from clavus.helpers import get_store_and_project

def cmd_cue(args: argparse.Namespace) -> None:
    """Add a timeline-anchored comment."""
    store, proj = get_store_and_project()
    head = store.read_ref("HEAD")

    position = args.position or "0.0.0"
    # Strip @ prefix if present
    if position.startswith("@"):
        position = position[1:]
    # Validate position format — must be bars.beats.sixteenths or bars:beats
    def _pos_is_ok(p: str) -> bool:
        try:
            if ":" in p:
                parts = p.split(":")
                int(parts[0]); int(parts[1]) if len(parts) > 1 else 0
                return True
            parts = p.split(".")
            for part in parts:
                int(part)
            return True
        except (ValueError, TypeError):
            return False
    if position != "0.0.0" and not _pos_is_ok(position):
        print(f"  [WARN] Invalid position '{position}' — using 0.0.0 instead")
        position = "0.0.0"

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
        print(f"   [note] Reply: clavus cue reply {cue.id} \"your response\"")


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
        print(f"-- Cue '{args.cue_id}' not found.")


def cmd_cue_resolve(args: argparse.Namespace) -> None:
    """Resolve a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    cue = cues.resolve(args.cue_id, note=args.note or "")
    if cue:
        print(f"[ok] Cue {args.cue_id} resolved")
        if args.note:
            print(f"   \"{args.note}\"")
    else:
        print(f"-- Cue '{args.cue_id}' not found.")


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
        print(f"-- Cue '{args.cue_id}' not found.")


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
            print(f"-- Unknown setting '{args.key}'.")
            print(f"   Valid keys: {', '.join(sorted(valid_keys))}")
            return
        setattr(cfg, args.key, args.value)
        cfg.save()
        print(f"[ok] {args.key} = {args.value}")
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
        print(f"-- Cue '{args.cue_id}' not found.")


def cmd_cue_unassign(args: argparse.Namespace) -> None:
    """Remove assignee from a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.unassign(args.cue_id)
    if result:
        print(f"👤 Unassigned cue {args.cue_id}")
    else:
        print(f"-- Cue '{args.cue_id}' not found.")


def cmd_cue_start(args: argparse.Namespace) -> None:
    """Mark a cue as in-progress."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.start(args.cue_id)
    if result:
        print(f"▶ Cue {args.cue_id} marked as in-progress")
    else:
        print(f"-- Cue '{args.cue_id}' not found.")


def cmd_cue_stop(args: argparse.Namespace) -> None:
    """Mark a cue as no longer in-progress."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.stop(args.cue_id)
    if result:
        print(f"⏸ Cue {args.cue_id} no longer in-progress")
    else:
        print(f"-- Cue '{args.cue_id}' not found.")


def cmd_cue_delete(args: argparse.Namespace) -> None:
    """Permanently delete a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    if cues.delete(args.cue_id):
        print(f"🗑 Deleted cue {args.cue_id}")
    else:
        print(f"-- Cue '{args.cue_id}' not found.")


def cmd_cue_archive(args: argparse.Namespace) -> None:
    """Archive a specific cue, or all cues."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    if args.cue_id:
        ok = cues.archive(args.cue_id)
        if ok:
            print(f"Archived cue {args.cue_id}")
        else:
            print(f"-- Cue '{args.cue_id}' not found.")
    else:
        count = cues.archive_resolved()
        print(f"Archived {count} cue(s).")


