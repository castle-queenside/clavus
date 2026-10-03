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
import hashlib
import json
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
from clavus.watch import watch
from clavus.store import (
    StemStore, StemEntry, StemManifest,
)
from clavus.sync import (
    load_remotes, save_remotes, Remote, push_to_remote, pull_from_remote, SyncDaemon,
)


# ─── Commands ──────────────────────────────────────────────────────────


from clavus.cli.commands.repair import cmd_repair
from clavus.cli.commands.doctor import cmd_doctor
from clavus.cli.commands.p2p import cmd_p2p
from clavus.cli.commands.help import cmd_help
from clavus.cli.commands.setup import cmd_setup
from clavus.cli.commands.init import cmd_init, init_project
from clavus.cli.commands.projects import cmd_projects, cmd_project
from clavus.cli.commands.snapshot import cmd_snapshot, create_snapshot, cmd_log, cmd_note
from clavus.cli.commands.diff import cmd_diff, cmd_status
from clavus.cli.commands.watch import cmd_watch
from clavus.cli.commands.relay import cmd_relay, cmd_share
from clavus.cli.commands.join import cmd_join
from clavus.cli.commands.tui import cmd_tui
from clavus.cli.commands.cues import (
    cmd_cue, cmd_cue_reply, cmd_cue_resolve, cmd_cue_skip, cmd_cues,
    cmd_cue_assign, cmd_cue_unassign, cmd_cue_start, cmd_cue_stop,
    cmd_cue_delete, cmd_cue_archive,
)
from clavus.cli.commands.branch import cmd_branch, cmd_checkout, cmd_merge
from clavus.cli.commands.remote import (
    cmd_remote, cmd_find, cmd_push, cmd_pull, cmd_pull_all, cmd_sync,
)
from clavus.cli.commands.render import cmd_cue_render
from clavus.cli.commands.restore import cmd_restore, cmd_open
from clavus.cli.commands.backup import cmd_backup, cmd_list_backups, cmd_restore_store
from clavus.cli.commands.stems import (
    cmd_stem_import, cmd_stem_import_folder, cmd_stem_list,
    cmd_stem_pull, cmd_stem_push,
)
from clavus.cli.commands.config import cmd_config

def main():
    import sys
    # --no-banner support: check before building argparser to avoid side-effects
    if "--no-banner" in sys.argv:
        sys.argv.remove("--no-banner")
    parser = argparse.ArgumentParser(
        description="Clavus — snapshot, sync, and collaborate on Ableton Live projects.",
        prog="clavus",
        add_help=False,  # manual help so we can banner before it
    )
    parser.add_argument("--clavus-dir", help="Override clavus storage directory")
    parser.add_argument("--version", action="store_true", help="Show version and exit")
    parser.add_argument("--no-banner", action="store_true", help=argparse.SUPPRESS)  # hidden, handled above
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Init
    p_init = subparsers.add_parser("init", help="Initialize a new clavus project")
    p_init.add_argument("path", nargs="?", default=None, help="Path to .als file or project directory")

    # Projects (list)
    subparsers.add_parser("projects", help="List all tracked projects")

    # Project (switch active)
    p_project = subparsers.add_parser("project", help="Switch active project, or toggle sharing")
    p_project.add_argument("name", help="Project name to switch to")
    p_project.add_argument("--share", action="store_true", help="Make project visible to collaborators")
    p_project.add_argument("--private", action="store_true", help="Hide project from collaborators")

    # Snapshot
    p_snap = subparsers.add_parser("snapshot", help="Create a new snapshot")
    p_snap.add_argument("message", nargs="?", default="",
                        help="Description of what changed (prompts if omitted)")
    p_snap.add_argument("--message", "-m", dest="message_flag", default=None,
                        help="Description of what changed (alternative to positional arg)")
    p_snap.add_argument("--tag", "-t", default="", help="Comma-separated tags")
    p_snap.add_argument("--notes", help="Longer-form session notes (markdown supported)")
    p_snap.add_argument("--parent", "-p", default=None, help="Override parent snapshot hash")
    p_snap.add_argument("--verbose", "-v", action="store_true", help="Show detailed diff")
    p_snap.add_argument("--allow-frozen", action="store_true", help="Skip frozen track warning (for TUI/non-interactive use)")

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

    # Note — read/write session notes on a snapshot
    p_note = subparsers.add_parser("note", help="Read or write session notes for a snapshot")
    p_note.add_argument("action", nargs="?", choices=["read", "write", "append"],
                        help="'read' shows notes, 'write' replaces, 'append' adds (default: read)")
    p_note.add_argument("text", nargs="*", default=None, help="Note text (for write/append)")
    p_note.add_argument("--hash", "-n", default=None, help="Snapshot hash (default: HEAD)")
    p_note.add_argument("--file", "-f", default=None, help="Read note text from file")

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
    p_remote.add_argument("action", nargs="?", choices=["list", "add", "remove", "rename", "projects", "pull"], default="list",
                         help="Action: list, add, remove, rename, projects, or pull")
    p_remote.add_argument("name", nargs="?", default="", help="Remote name (and project name for pull)")
    p_remote.add_argument("url", nargs="?", default="", help="Remote URL or project name (for pull)")
    p_remote.add_argument("--add", default="", help=argparse.SUPPRESS)
    p_remote.add_argument("--remove", default="", help=argparse.SUPPRESS)

    p_push = subparsers.add_parser("push", help="Push cues/snapshots to remotes")
    p_push.add_argument("remote", nargs="?", default=None,
                        help="Remote name (default: all)")
    p_push.add_argument("--force", "-f", action="store_true",
                        help="Force push — skip conflict check, overwrite relay state")

    p_pull = subparsers.add_parser("pull", help="Pull cues/snapshots from remotes")
    p_pull.add_argument("remote", nargs="?", default=None,
                        help="Remote name (default: all)")
    p_pull.add_argument("--output", "-o", type=str, default=None,
                        help="Output directory for project folder")

    p_pull_all = subparsers.add_parser("pull-all", help="Pull ALL projects from active remote")
    p_pull_all.add_argument("remote", nargs="?", default=None,
                            help="Remote name (default: active remote)")

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
    p_watch.add_argument("subcommand", nargs="?", default=None,
                        choices=["install", "start", "stop", "restart", "status"],
                        help="install (set up launchd/systemd service), start, stop, restart, status")
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
    p_relay.add_argument("--project", type=str, default=None,
                        help="Only serve this project (hide others from collaborators)")

    # Share (relay + auto-discovery)
    p_share = subparsers.add_parser("share", help="Start a share session — relay + auto-discovery")
    p_share.add_argument("--host", default=None,
                        help="Host to bind to (default: from config or 0.0.0.0)")
    p_share.add_argument("--port", "-p", type=int, default=None,
                        help="Port to listen on (default: from config or 7890)")

    p_share.add_argument("--background", "-b", action="store_true",
                        help="Run relay in background, return immediately")
    p_share.add_argument("--bg", action="store_true",
                        help="Alias for --background")
    p_share.add_argument("--kill", action="store_true",
                        help="Stop the background relay")
    p_share.add_argument("--project", type=str, default=None,
                        help="Only serve this project (hide others from collaborators)")
    # Join (discover and connect)
    p_join = subparsers.add_parser("join", help="Discover and connect to a Clavus share session")
    p_join.add_argument("code", nargs="?", default="",
                       help="Share code or relay URL to connect to (e.g. BRIGHT-DUCK-7 or http://100.127.1.109:7890)")
    p_join.add_argument("--timeout", "-t", type=int, default=5,
                       help="Seconds to scan for (default: 5)")
    p_join.add_argument("--lan", action="store_true",
                       help="Scan LAN only (default: LAN + Tailscale)")
    p_join.add_argument("--tailscale", action="store_true",
                       help="Scan Tailscale only (default: LAN + Tailscale)")

    # Inject (alias for cue-render --inject)
    subparsers.add_parser("inject", help="Inject unresolved cues as Ableton markers")

    # TUI (terminal dashboard)
    p_tui = subparsers.add_parser("tui", help="Launch the TUI dashboard")
    p_tui.add_argument("--connect", "-c", default="",
                       help="Clavus server URL (default: from config or http://localhost:7890)")
    p_tui.add_argument("--debug", "-d", action="store_true",
                       help="Enable diagnostic logging to ~/.clavus/debug.log")

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

    p_stem_import_folder = stem_sub.add_parser("import-folder", help="Import all WAV files from a folder as stems")
    p_stem_import_folder.add_argument("folder", help="Path to folder containing .wav, .aif, or .aiff files (Ableton exports AIFF by default)")
    p_stem_import_folder.add_argument("--prefix", "-p", default="",
                                     help="Optional prefix for track names (e.g. 'Drums -')")

    p_stem_list = stem_sub.add_parser("list", help="List stems for a snapshot")
    p_stem_list.add_argument("--snapshot", "-s", default="",
                             help="Snapshot hash (default: current HEAD)")

    p_stem_push = stem_sub.add_parser("push", help="Push stem files to remotes")

    p_stem_pull = stem_sub.add_parser("pull", help="Pull stem files from remotes")

    # Repair (recover from corrupt/missing index)
    p_doctor = subparsers.add_parser("doctor", help="Diagnose Clavus store health (read-only)")
    p_doctor.add_argument("--verbose", "-v", action="store_true", help="Show detailed info")

    # P2P (peer-to-peer sync over Tailscale)
    p_p2p = subparsers.add_parser("p2p", help="Discover and sync with peers on the tailnet")
    p_p2p.add_argument("--host", action="store_true", help="Start listening for incoming connections")
    p_p2p.add_argument("--connect", type=str, default="",
                       help="Connect to a peer by their Tailscale DNS name")

    p_setup = subparsers.add_parser("setup", help="Guided first-run setup")
    p_repair = subparsers.add_parser("repair", help="Repair Clavus storage — recover projects from backup, cues, and refs")
    p_repair.add_argument("--force", "-f", action="store_true",
                          help="Force repair even if index.json exists")
    p_repair.add_argument("--set-als", type=str, default="",
                          help="Set .als path for recovered projects (name=/path format or 'all=/path')")

    # Help
    p_help = subparsers.add_parser("help", help="Show this help message")
    p_help.add_argument("topic", nargs="?", default="", help="Topic to get help on")

    args = parser.parse_args()

    if args.version:
        try:
            from importlib.metadata import version
            v = version("clavus")
        except ImportError:
            v = "0.1.0-beta"
        # Banner already printed by main() entry point
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
        "note": cmd_note,
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
        "p2p": cmd_p2p,
        "setup": cmd_setup,
        "help": cmd_help,
        "repair": cmd_repair,
        "push": cmd_push,
        "pull": cmd_pull,
        "pull-all": cmd_pull_all,
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
    elif args.command == "inject":
        args.inject = True
        cmd_cue_render(args)
    elif args.command == "stem":
        stem_actions = {
            "import": cmd_stem_import,
            "import-folder": cmd_stem_import_folder,
            "list": cmd_stem_list,
            "push": cmd_stem_push,
            "pull": cmd_stem_pull,
        }
        if args.stem_action in stem_actions:
            stem_actions[args.stem_action](args)
        else:
            print("Usage: clavus stem {import|list|push|pull}")
    else:
        # No command given — friendly welcome
        from rich.console import Console
        from rich.style import Style
        c = Console()
        accent = Style(color="#1a9e9e", bold=True)
        dim = Style(color="#6a9a9a")
        print()
        c.print("  👋 Welcome to [bold #1a9e9e]Clavus[/]", style=accent)
        c.print("  snapshot · sync · collaborate on Ableton Live", style=dim)
        print()
        c.print("  Get started:", style=accent)
        print("    clavus doctor     — check your setup")
        print("    clavus tui        — launch the dashboard")
        print("    clavus projects   — list your projects")
        print()
        c.print("  Run 'clavus --help' for all commands.", style=dim)


if __name__ == "__main__":
    main()
