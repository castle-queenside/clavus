"""
clavus TUI — Minimal, bulletproof terminal UI for cue management.

Layout:
  row 0: header
  row 1: cues list (left) | history (right)
  row 2: footer / inline input bar
"""

from __future__ import annotations

import asyncio
import hashlib
import io
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Container, Horizontal, Vertical, VerticalScroll
from textual.screen import Screen, ModalScreen
from textual.widgets import Static, Input, ListView, ListItem, Label, Button
from textual.css.query import NoMatches

# ─── Color Palette (CRUX dark) ──────────────────────────────────────────────

C = {
    "bg": "#0b1418", "surface": "#0f1a20", "surface2": "#162a34",
    "border": "#1a3040", "accent": "#1a9e9e", "fg": "#b8c8c8",
    "dim": "#6a8a8a", "muted": "#3a5a65",
    "yellow": "#d4a030", "green": "#44cc44", "red": "#ff4444",
}

# ─── Data Models ────────────────────────────────────────────────────────────

@dataclass
class Reply:
    id: str = ""
    author: str = ""
    text: str = ""
    timestamp: float = 0.0

@dataclass
class Cue:
    id: str = ""
    position: str = "1.1.1"
    text: str = ""
    author: str = ""
    status: str = "pending"
    timestamp: float = 0.0
    track_name: str = ""
    snapshot_hash: str = ""
    assignee: str = ""
    in_progress: bool = False
    conflict: bool = False
    replies: list = field(default_factory=list)

@dataclass
class Snap:
    hash: str = ""       # truncated (10 chars) — for display
    full_hash: str = ""  # full SHA256 — for file lookups
    message: str = ""
    timestamp: float = 0.0
    track_count: int = 0
    conflict_message: str | None = None  # Remote message in conflict with local

    @classmethod
    def from_dict(cls, d: dict) -> "Snap":
        return cls(
            hash=d.get("hash", d.get("full_hash", ""))[:10],
            full_hash=d.get("hash", d.get("full_hash", "")),
            message=d.get("message", ""),
            timestamp=d.get("timestamp", 0.0),
            track_count=d.get("track_count", 0),
            conflict_message=d.get("conflict_message", None),
        )

# ─── Help Screen ───────────────────────────────────────────────────────

class HelpScreen(Screen):
    """Full overlay showing all key bindings and commands."""

    CSS = f"""
    HelpScreen {{ background: {C['bg']}e0; align: center middle; }}
    #help-box {{ 
        width: 68; max-height: 95%;
        background: {C['surface']}; border: thick {C['accent']};
        padding: 0 1;
    }}
    #help-box Static {{ width: 100%; }}
    #help-box .help-title {{ color: {C['accent']}; text-style: bold; }}
    #help-box .help-key {{ color: {C['accent']}; }}
    #help-box .help-desc {{ color: {C['fg']}; }}
    #help-box .help-dim {{ color: {C['muted']}; }}
    #help-box .help-section {{ color: {C['yellow']}; text-style: bold; padding-top: 1; }}
    """

    BINDINGS = [
        Binding("escape", "dismiss", "Close"),
        Binding("q", "dismiss", "Close"),
        Binding("enter", "dismiss", "Close"),
        Binding("h", "dismiss", "Close"),
        Binding("j", "scroll_down", show=False),
        Binding("k", "scroll_up", show=False),
        Binding("down", "scroll_down", show=False),
        Binding("up", "scroll_up", show=False),
    ]

    def compose(self):
        with VerticalScroll(id="help-box"):
            yield Static("CLAVUS — KEY BINDINGS", classes="help-title")
            yield Static("CUES & COLLABORATION", classes="help-section")
            yield Static("  c    New cue        e    Edit         r    Reply")
            yield Static("  a    Assign         x    Archive      ▶    Start")
            yield Static("  R    Resolve        !    Conflict      d    Diff")
            yield Static("  T    Restore snap   e    Edit item     o    Open in Live")
            yield Static("SNAPSHOTS & SYNC", classes="help-section")
            yield Static("  p    Pull           P    Push          S    Snap + auto-push")
            yield Static("NAVIGATION", classes="help-section")
            yield Static("  j/↓  Down           k/↑  Up           Tab  Switch pane")
            yield Static("  Esc  Cancel/Dismiss ?/h  Help         :    Command mode")
            yield Static("COMMANDS (:)", classes="help-section")
            yield Static("  :snapshot <msg>  Create snapshot     :project <name>  Switch project")
            yield Static("  :open [path]     Open in Ableton     :pull / :push    Manual sync")
            yield Static("  :stem push/pull  Stem file sync      :init <path>     Init project")
            yield Static("  :remote rename <name>               :remote add <name> <url>")
            yield Static("[dim]Esc / q / Enter / h — close[/]", classes="help-dim")

    def action_dismiss(self):
        self.app.pop_screen()


# ─── App ────────────────────────────────────────────────────────────────────

class ClavusApp(App):
    CSS = f"""
    ClavusApp {{ background: {C['bg']}; }}
    Screen {{ background: {C['bg']}; }}

    #main {{ layout: grid; grid-size: 1 3; grid-rows: auto 1fr auto; height: 100%; }}

    #header-title {{ color: {C['accent']}; text-style: bold; background: {C['surface']}; padding: 0 1; }}

    #content {{ layout: grid; grid-size: 2 1; grid-columns: 5fr 2fr; height: 100%; }}

    #welcome {{ display: none; column-span: 2; content-align: center middle; height: 100%; color: {C['dim']}; text-style: bold; }}

    #cues-list {{ height: 100%; min-height: 5; border: solid {C['border']}; background: transparent; }}
    #cues-list:focus-within {{ border: solid {C['accent']}; background: rgba(26,158,158,0.03); }}
    #cues-list ListView {{ height: 100%; border: none; background: transparent; }}
    #cues-list ListItem {{ background: transparent; padding: 0 2; min-height: 1; max-height: 10; }}
    #cues-list ListItem:hover {{ background: {C['surface']}; }}
    #clv > ListItem.-highlight {{ background: {C['surface']}80; text-style: bold; border-left: solid transparent; padding-left: 1; }}
    #clv:focus > ListItem.-highlight {{ background: {C['surface2']}; text-style: bold; border-left: solid #d4a030; padding-left: 1; }}
    #hlv > ListItem.-highlight {{ background: {C['surface']}80; text-style: bold; border-left: solid transparent; padding-left: 1; }}
    #hlv:focus > ListItem.-highlight {{ background: {C['surface2']}; text-style: bold; border-left: solid #d4a030; padding-left: 1; }}

    #history {{ height: 100%; background: {C['surface']}; padding: 0 1; border: solid transparent; }}
    #history:focus-within {{ border: solid {C['accent']}; background: rgba(26,158,158,0.03); }}
    #history-list {{ height: 100%; }}
    #history-list ListView {{ height: 100%; border: none; background: transparent; }}
    #history-list ListItem {{ background: transparent; padding: 0 1; min-height: 1; }}

    #footer {{ height: 1; background: {C['surface']}; padding: 0 1; }}
    #footer.input-mode {{ height: 3; padding: 0; }}
    #footer-keys {{ display: none; }}
    #footer-status {{ color: {C['fg']}; }}
    #footer-hint {{ color: {C['muted']}; text-align: right; }}
    #footer-stats {{ display: none; }}
    #footer-input {{ display: none; width: 100%; height: 3; background: {C['bg']}; border: solid {C['accent']}; color: {C['fg']}; padding: 0 1; }}
    #footer.input-mode #footer-input {{ display: block; }}
    #footer.input-mode #footer-status {{ display: none; }}
    #footer.input-mode #footer-hint {{ display: none; }}

    Scrollbar {{ scrollbar-color: #1a9e9e #0f1a20; }}
    Scrollbar > .scrollbar--grabber {{ background: #1a9e9e; }}
    Scrollbar.vertical > .scrollbar--grabber {{ min-height: 3; }}
    """

    BINDINGS = [
        Binding("q", "quit", "Quit"),
        Binding("r", "reply", "Reply"),
        Binding("c", "cue_new", "New cue"),
        Binding("S", "snapshot", "Snapshot"),
        Binding("R", "resolve", "Resolve"),
        Binding("T", "restore_snapshot", "Restore"),
        Binding("i", "inject_cues", "Inject"),
        Binding("a", "assign", "Assign"),
        Binding("x", "archive", "Archive"),
        Binding("!", "resolve_conflict", "Conflict"),
        Binding("d", "diff", "Diff"),
        Binding("p", "pull", "Pull"),
        Binding("P", "push", "Push"),
        Binding("o", "open_selected_or_head", "Open"),
        Binding("e", "edit_item", "Edit"),
        Binding("tab", "focus_next_pane", "Pane"),
        Binding("j", "cursor_down", "Down", show=False),
        Binding("k", "cursor_up", "Up", show=False),
        Binding("down", "cursor_down", "Down", show=False),
        Binding("up", "cursor_up", "Up", show=False),
        Binding(":", "command", ":cmd", show=False),
        Binding("?", "help", "Help", show=False),
        Binding("h", "help", "Help", show=False),
        Binding("escape", "cancel_input", "Cancel input", show=False),
    ]

    def __init__(self, url: str = ""):
        super().__init__()
        from clavus.store import BlobStore
        from clavus.config import ClavusConfig
        self.store = BlobStore()
        self.server_url = url or "local"
        self.project: str = ""
        self.connected: bool = True  # Always connected — working from disk
        self.ws_connected: bool = False
        self.cues: list[Cue] = []
        self.snaps: list[Snap] = []
        self.idx: int = 0
        self._input_mode: str = ""
        self._input_debounce: float = 0.0  # unix ts after last _hide_input
        self._pending_cue_text: str = ""
        self._relay_proc = None
        self._busy: bool = False
        self._last_sync: str = ""     # "⬆ ✓ 12:34" or "⬇ ✓ 12:34" — last completed sync
        self._last_snap_time: float = 0.0  # unix timestamp of last auto-snapshot
        self._sync_status: str = ""    # Live sync progress: "⬆ pushing...", "⬇ pulling..."
        self._spinner_idx: int = 0     # Braille spinner frame
        self._spinner_timer = None     # Timer handle for spinner animation
        self._peer_name: str = ""     # remote name (e.g. "mac")
        self._peer_reachable: bool = False
        self._archived_count: int = 0  # cues with status="archived" (hidden from list)
        _cfg = ClavusConfig.load()
        self.author = _cfg.author
        self._clavus_cfg = _cfg
        self._cue_store = None  # Lazy init per project
        self._header_title: Optional[Static] = None
        self._footer_stats: Optional[Static] = None

    def _load_config(self) -> str:
        from clavus.config import ClavusConfig
        return ClavusConfig.load().author

    def _save_config(self):
        from clavus.config import ClavusConfig
        self._clavus_cfg.author = self.author
        self._clavus_cfg.save()

    def compose(self):
        with Container(id="main"):
            yield Static("⧩ clavus", id="header-title")
            yield Container(
                Container(ListView(id="clv"), id="cues-list"),
                Container(
                    Static(" History", id="history-label"),
                    Container(ListView(id="hlv"), id="history-list"),
                    id="history",
                ),
                Static("", id="welcome"),
                id="content",
            )
            yield Horizontal(
                Static("", id="footer-status"),
                Input(placeholder="type here...", id="footer-input"),
                Static("", id="footer-keys"),
                Static("? help", id="footer-hint"),
                id="footer",
            )

    def on_mount(self):
        self._header_title = self.query_one("#header-title", Static)
        self._footer_stats = self.query_one("#footer-status", Static)
        self._update_header()
        self._update_footer()
        self._update_footer_hint()
        self._update_welcome()
        self._connect()
        # Periodic health probe — re-check relay reachability every 15s
        self.set_interval(15.0, self._probe_reachability)

    # ─── Input bar ──────────────────────────────────────────────────────

    def _show_input(self, mode: str, prompt: str, prefill: str = ""):
        if self._input_mode:
            self._log_event(f"input blocked: already in {self._input_mode} mode")
            return  # already showing input
        if time.time() - self._input_debounce < 0.15:
            self._log_event(f"input blocked: debounce ({mode})")
            return  # within 150ms of dismiss, ignore double-tap
        # Remember which pane had focus so we can restore it after submit
        self._pre_input_focus = self._focused_list_view()
        self._input_mode = mode
        footer = self.query_one("#footer")
        inp = self.query_one("#footer-input", Input)
        inp.value = prefill
        self.query_one("#footer-status", Static).update(f"[{C['accent']}]{prompt}[/]")
        footer.add_class("input-mode")
        inp.focus()

    def _hide_input(self):
        self._input_mode = ""
        self._input_debounce = time.time()  # block double-tap after dismiss
        self.query_one("#footer").remove_class("input-mode")
        self.call_after_refresh(self._update_footer)
        # Restore focus to whichever pane had it before the input appeared
        prev = getattr(self, '_pre_input_focus', None)
        if prev is not None:
            try:
                prev.focus()
            except Exception:
                self._focus_cues()
            self._pre_input_focus = None
        else:
            self._focus_cues()

    def on_input_submitted(self, event: Input.Submitted):
        event.stop()
        text = event.value.strip()
        mode = self._input_mode
        self._hide_input()
        if not text:
            return
        if mode == "reply":
            self._do_reply(text)
        elif mode == "edit":
            self._do_edit(text)
        elif mode == "cue":
            self._pending_cue_text = text
            self._input_mode = "cue_pos"
            footer = self.query_one("#footer")
            inp = self.query_one("#footer-input", Input)
            inp.value = "1.1.1"
            self.query_one("#footer-status", Static).update(
                f"[{C['accent']}]position @ (or blank for 1.1.1):[/]")
            footer.add_class("input-mode")
            inp.focus()
            return
        elif mode == "cue_pos":
            self._do_new_cue(self._pending_cue_text, text)
        elif mode == "switch_proj":
            self._run_switch_project(text)
        elif mode == "assign":
            self._do_assign(text)
        elif mode == "browse":
            self._run_browse(text)
        elif mode == "confirm_delete":
            if text.lower() in ("y", "yes"):
                self._do_delete_cue()
            else:
                self._focus_cues()
        elif mode == "confirm_archive":
            if text.lower() in ("y", "yes"):
                self._do_archive_cue()
            else:
                self._focus_cues()
        elif mode == "edit_snapshot":
            self._do_edit_snapshot(text)
        elif mode == "command":
            self._do_command(text)

    def action_cancel_input(self):
        if self._input_mode:
            self._hide_input()
            self._focus_cues()

    # ─── Command mode ──────────────────────────────────────────────────

    def action_command(self):
        self._show_input("command", ":", prefill="")

    def _do_command(self, text: str):
        parts = text.strip().split(maxsplit=1)
        if not parts:
            return
        cmd = parts[0].lower()
        arg = parts[1] if len(parts) > 1 else ""
        # Strip surrounding quotes users often type around paths with spaces
        if arg and len(arg) >= 2 and arg[0] in ('"', "'") and arg[0] == arg[-1]:
            arg = arg[1:-1]

        if cmd == "project" and arg:
            self._run_switch_project(arg)
        elif cmd == "projects":
            self._run_list_projects()
        elif cmd == "name" and arg:
            self.author = arg
            self._save_config()
            self._status(f"name set to: {self.author}")
        elif cmd == "init" and arg:
            self._run_init_project(arg)
        elif cmd == "browse":
            self._show_input("browse", "browse: ", prefill=arg or "~")
        elif cmd == "inject":
            self._run_inject()
        elif cmd == "restore":
            self._run_restore(arg)
        elif cmd == "open":
            self._run_open(arg)
        elif cmd == "setup":
            self._run_setup()
        elif cmd in ("status", "info"):
            self._run_status()
        elif cmd == "doctor":
            self._run_doctor()
        elif cmd == "log":
            self._run_log()
        elif cmd == "config":
            self._run_config()
        elif cmd == "remote":
            self._run_remote(arg)
        elif cmd in ("pull", "push"):
            # :pull or :pull <project> — mirrors CLI behavior
            if arg:
                import subprocess, sys
                subprocess.run([sys.executable, "-m", "clavus", cmd, arg])
                down = "\u2b07"; up = "\u2b06"
                self._last_sync = f"{down if cmd == 'pull' else up} {time.strftime('%H:%M')}"
                self._connect()  # reload
            else:
                self.action_pull() if cmd == "pull" else self.action_push()
        elif cmd == "branch":
            self._run_branch(arg)
        elif cmd == "backup":
            self._run_backup()
        elif cmd == "backups":
            self._run_list_backups()
        elif cmd == "restore-store":
            self._run_restore_store(arg)
        elif cmd == "snapshot":
            if arg:
                self._run_snapshot(arg)
            else:
                self._show_input("command", "snapshot <message>: ", prefill="")
        elif cmd == "stem":
            if arg == "push":
                self.action_stem_push()
            elif arg == "pull":
                self.action_pull()
            else:
                self._status("stem push  |  stem pull")
        elif cmd == "archive":
            self.action_archive()
        elif cmd in ("delete", "del"):
            self.action_delete_cue()
        elif cmd == "share":
            self._run_share()
        elif cmd == "join":
            if arg and (arg.startswith("http://") or arg.startswith("https://")):
                self._run_join_url(arg)
            else:
                self._status("use :join http://IP:PORT — get URL from 'clavus share' on host")
        elif cmd in ("help", "h", "?"):
            self.push_screen(HelpScreen())
        else:
            self._status(f"unknown: {cmd}")

    @work(exclusive=False)
    async def _run_switch_project(self, name: str):
        self._status(f"switching to {name}...")
        proj = self.store.get_index(name)
        if not proj:
            self._status(f"project '{name}' not found")
            return
        # Save _last_project for next launch
        if self.store.index_path.exists():
            index = json.loads(self.store.index_path.read_text())
            index["_last_project"] = name
            self.store.index_path.write_text(json.dumps(index, indent=2, default=str))
        self.project = name
        self._log_event(f"switched to project '{name}'")
        self._load_cues_from_disk()
        self._load_snapshots_from_disk()
        self.idx = 0
        self._update_header()
        self._render()
        self._update_footer()
        # Show result as a temporary label at top of cue list
        msg = f"  switched to project [bold]{name}[/]  —  {len(self.cues)} cues, {len(self.snaps)} snapshots"
        try:
            lv = self.query_one("#clv", ListView)
            lv.mount(Label(msg, classes="project-list"), before=0)
            self.set_timer(3.0, lambda: self._clear_project_list())
        except NoMatches:
            pass

    @work(exclusive=False)
    async def _run_list_projects(self):
        projects = self.store.list_projects()
        if not projects:
            self._status("no projects found  —  run :init <path> to add one")
            return
        lines = []
        for p in projects:
            name = p.name
            head = p.head or ""
            branch = p.branch or "main"
            active = " ◀" if name == self.project else ""
            lines.append(f"  {name}  @ {head[:12] if head else '(no snaps)':12s}  [{branch}]{active}")
        msg = "\n".join(lines)
        # Show as a temporary log entry so it's visible above the footer
        try:
            lv = self.query_one("#clv", ListView)
            lv.mount(Label(msg, classes="project-list"), before=0)
            self._status("")
            self.set_timer(3.0, lambda: self._clear_project_list())
        except NoMatches:
            self._status(msg)
        self._show_input("switch_proj", "project name to switch:")

    def _clear_project_list(self):
        try:
            lv = self.query_one("#clv", ListView)
            for c in list(lv.children):
                if hasattr(c, "classes") and "project-list" in c.classes:
                    c.remove()
        except NoMatches:
            pass

    @work(exclusive=False)
    async def _run_init_project(self, path: str):
        """Import a project from a filesystem path — background worker so UI stays alive."""
        # Defensive: strip quotes that may have leaked through
        if path and len(path) >= 2 and path[0] in ('"', "'") and path[0] == path[-1]:
            path = path[1:-1]
        self._sync_status = "importing project..."
        self._update_header()
        self._status(f"importing {path}...")
        try:
            from clavus.cli import init_project
            name, logs = init_project(path)
            for line in logs:
                self._log_event(line)
            if name is None:
                self._sync_status = ""
                self._update_header()
                self._status("init failed — see log")
                return
            self._sync_status = ""
            self._update_header()
            self._footer_toast(f"[{C['green']}]✓ imported: {name}[/] — loading...", 5.0)
            self._log_event(f"✓ project '{name}' ready")
            # Reload from disk
            self._connect()
        except Exception as e:
            self._sync_status = ""
            self._update_header()
            self._log_event(f"init error: {e}")

    @work(exclusive=False)
    async def _run_browse(self, directory: str):
        """Browse a directory for .als files locally (no server needed)."""
        from pathlib import Path
        # Normalize relative paths
        if directory and "/" not in directory and "\\" not in directory and directory not in ("..", ".", ""):
            if hasattr(self, "_last_browse_dir") and self._last_browse_dir:
                directory = os.path.join(self._last_browse_dir, directory)
        self._last_browse_dir = directory
        d = Path(directory or os.path.expanduser("~")).expanduser().resolve()
        if not d.exists():
            self._status(f"directory not found: {d}")
            return
        subdirs = sorted([x.name for x in d.iterdir() if x.is_dir() and not x.name.startswith(".")])
        als_files = sorted([x.name for x in d.glob("*.als")])
        lines = [f"📁 {d}"]
        if subdirs:
            lines.append(f"  dirs: {' '.join(subdirs[:8])}{'...' if len(subdirs)>8 else ''}")
        if als_files:
            lines.append(f"  🔵 als: {' '.join(als_files)}")
        self._status(" | ".join(lines))
        if als_files:
            hint = f" — :init {d}/<name> to import"
        else:
            hint = ""
        self._show_input("browse", "browse (enter subdir, .. up, or :init): ", prefill="")

    @work(exclusive=False)
    async def _run_inject(self):
        """Inject unresolved cues as Ableton markers via CLI subprocess."""
        if not self.project:
            self._status("no project selected")
            return
        import asyncio
        self._status("injecting cues into .als...")
        try:
            proc = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "clavus", "cue-render", "--inject",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=30)
            out = stdout.decode().strip()
            err = stderr.decode().strip()
            if out:
                for line in out.split("\n"):
                    if line.strip():
                        self._log_event(line.strip())
            if proc.returncode != 0 and err:
                self._log_event(f"error: {err}")
            self._status("inject complete" if proc.returncode == 0 else "inject failed")
            # Auto-snapshot so injected markers survive future :open
            if proc.returncode == 0 and out and "no changes" not in out:
                self._log_event("auto-snapshot to save injected markers...")
                asyncio.create_task(self._run_snapshot_for_inject())
        except Exception as e:
            self._status(f"inject error: {e}")

    @work(exclusive=False)
    async def _run_restore(self, hash_str: str = ""):
        """Restore the .als from a snapshot backup via CLI subprocess."""
        if not self.project:
            self._status("no project selected")
            return
        import asyncio
        self._status(f"restoring from {'HEAD' if not hash_str else hash_str}...")
        try:
            cmd = [sys.executable, "-m", "clavus", "restore"]
            if hash_str:
                cmd.append(hash_str)
            proc = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=30)
            out = stdout.decode().strip()
            if out:
                for line in out.split("\n"):
                    if line.strip():
                        self._log_event(line.strip())
            self._status("restore complete" if proc.returncode == 0 else "restore failed")
            await self._do_pull()
        except Exception as e:
            self._status(f"restore error: {e}")

    @work(exclusive=False)
    async def _run_join_url(self, url: str):
        """Add a remote by URL and pull immediately."""
        from clavus.sync import Remote, save_remotes, load_remotes, pull_from_remote
        self._status(f"joining {url}...")
        name = url.replace("http://", "").replace("https://", "").split(":")[0].replace(".", "-")
        try:
            store = self.store
            remotes = load_remotes(store)
            # Replace existing remote with same name
            remotes = [r for r in remotes if r.name != name]
            remotes.append(Remote(name=name, url=url))
            save_remotes(store, remotes)
            self._status(f"added remote '{name}' — pulling...")
            await self._do_pull()
        except Exception as e:
            self._status(f"join failed: {e}")

    @work(exclusive=False)
    async def _run_open(self, hash_str: str = ""):
        """Restore the HEAD snapshot .als to the project folder and open it."""
        if not self.project:
            self._status("no project selected")
            return
        proj = self.store.get_index(self.project)
        if not proj or not proj.root_als:
            self._status("project not found in store")
            return

        h = hash_str or proj.head
        if not h:
            self._status("no snapshots to open")
            return

        from clavus.helpers import resolve_snapshot
        resolved = resolve_snapshot(self.store, h)
        if not resolved:
            self._status(f"could not resolve hash: {h}")
            return

        snap = self.store.load_snapshot(resolved)
        if not snap or not snap.als_hash:
            self._status("snapshot has no .als backup")
            return

        raw = self.store.get_object(snap.als_hash)
        if not raw:
            self._status("raw .als blob missing — try pulling")
            return

        # Write to Ableton project folder convention:
        # Ableton expects "Song.als" inside "Song Project/" subfolder.
        # Always compute from projects_dir — NEVER derive from root_als
        # (prevents infinite nesting when root_als was set by a prior open).
        from clavus.helpers import get_projects_dir
        proj_dir = get_projects_dir() / self.project
        als_dir = proj_dir / f"{self.project} Project"
        out = als_dir / f"{self.project}.als"
        out.parent.mkdir(parents=True, exist_ok=True)
        # Create Ableton project folder scaffolding so Ableton recognizes
        # this as a valid project folder and doesn't auto-create nested copies.
        # Ableton checks for: * Project/ folder name + Ableton Project Info/
        (als_dir / "Samples").mkdir(exist_ok=True)
        (als_dir / "Backup").mkdir(exist_ok=True)
        proj_info = als_dir / "Ableton Project Info"
        proj_info.mkdir(exist_ok=True)
        out.write_bytes(raw)

        # Update root_als so future snapshots find the right file
        proj.root_als = str(out)
        self.store.set_index(proj)

        msg = f"restored {self.project}.als ← snapshot {resolved[:10]}"
        self._log_event(msg)

        # Launch in Ableton
        import platform
        if platform.system() == "Darwin":
            import subprocess as sp
            for v in ["12", "11", "10"]:
                able = Path(f"/Applications/Ableton Live {v}/Ableton Live {v}.app")
                if able.exists():
                    sp.Popen(["open", "-a", str(able), str(out)])
                    break
            else:
                sp.Popen(["open", str(out)])
        elif platform.system() == "Windows":
            import os as _os
            _os.startfile(str(out))
    @work(exclusive=False)
    async def _run_setup(self):
        """Run the interactive setup wizard (same as 'clavus setup')."""
        import asyncio
        self._status("running setup...")
        self._log_event("launching setup wizard")
        try:
            proc = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "clavus", "setup",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            if proc.stdout:
                async for line in proc.stdout:
                    text = line.decode().strip()
                    if text:
                        self._log_event(text)
                        self._status(text)
            await proc.wait()
            self._status("setup complete")
        except Exception as e:
            self._status(f"setup failed: {e}")

    def _run_status(self):
        """Show detailed connection status in the footer."""
        relay_status = "running" if (self._relay_proc and self._relay_proc.poll() is None) else "none"
        parts = [
            f"server: {self.server_url}",
            f"connected: {self.connected}",
            f"relay: {relay_status}",
            f"project: {self.project or '(none)'}",
            f"cues: {len(self.cues)}",
            f"snaps: {len(self.snaps)}",
            f"author: {self.author}",
        ]
        self._status("  |  ".join(parts))

    def _run_doctor(self):
        """Run clavus doctor in background and show result."""
        import subprocess, sys
        try:
            proc = subprocess.run(
                [sys.executable, "-m", "clavus", "doctor"],
                capture_output=True, text=True, timeout=10,
            )
            lines = [l.strip() for l in proc.stdout.split("\n") if l.strip()][:8]
            self._status(" | ".join(lines[:3]) if lines else "doctor ran")
        except Exception as e:
            self._status(f"doctor failed: {e}")

    def _run_log(self):
        """Show snapshot history."""
        import subprocess, sys
        try:
            proc = subprocess.run(
                [sys.executable, "-m", "clavus", "log"],
                capture_output=True, text=True, timeout=10,
            )
            lines = [l.strip() for l in proc.stdout.split("\n") if l.strip()][:12]
            for line in lines:
                self._log_event(line)
            self._status(f"showing {len(lines)} snapshots")
        except Exception as e:
            self._status(f"log failed: {e}")

    def _run_config(self):
        """Show current config."""
        from clavus.config import ClavusConfig
        cfg = ClavusConfig.load()
        lines = [
            f"author: {cfg.author}",
            f"port: {cfg.port}",
            f"host: {cfg.host}",
            f"project: {self.project or '(none)'}",
            f"cues: {len(self.cues)}  snaps: {len(self.snaps)}",
        ]
        self._status("  |  ".join(lines))

    def _run_remote(self, action: str = ""):
        """Manage remotes: list, add, remove, rename.
        
        Smart rename:
          :remote rename Relay       → renames connected remote to "Relay"
          :remote rename mac Studio  → renames remote "mac" to "mac Studio"
          :remote rename "mac studio"→ renames connected remote to "mac studio"
        """
        import subprocess, sys
        try:
            cmd = [sys.executable, "-m", "clavus", "remote"]
            if action:
                parts = action.split()
                if parts[0] == "rename" and len(parts) >= 2:
                    if len(parts) == 2:
                        # :remote rename Relay → rename connected remote
                        if self._peer_name:
                            cmd.extend(["rename", self._peer_name, parts[1]])
                            self._log_event(f"renaming '{self._peer_name}' → '{parts[1]}'")
                        else:
                            self._status("no connected remote — use :remote rename <old> <new>")
                            return
                    else:
                        # :remote rename mac Studio → is "mac" an existing remote?
                        from clavus.sync import load_remotes
                        remotes = load_remotes(self.store)
                        first = parts[1]
                        match = next((r for r in remotes if r.name.lower() == first.lower()), None)
                        new_name = " ".join(parts[2:])
                        if match:
                            cmd.extend(["rename", match.name, new_name])
                            self._log_event(f"renaming '{match.name}' → '{new_name}'")
                        elif self._peer_name:
                            # First word isn't a remote → treat all as new name
                            full = " ".join(parts[1:])
                            cmd.extend(["rename", self._peer_name, full])
                            self._log_event(f"renaming '{self._peer_name}' → '{full}'")
                        else:
                            self._status(f"remote '{first}' not found — use :remote rename <old> <new>")
                            return
                else:
                    cmd.extend(parts)
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
            out = proc.stdout.strip()
            err = proc.stderr.strip()
            if out:
                for line in out.split("\n")[:5]:
                    self._log_event(line)
            if err:
                for line in err.split("\n")[:3]:
                    self._log_event(f"remote: {line}")
            if not out and not err:
                self._status("remote list" if not action else f"remote {action}")
            elif proc.returncode != 0:
                self._status(f"remote failed (exit {proc.returncode})")
            else:
                # On success, reload remote name so header updates immediately
                if parts[0] in ("rename", "add", "remove") and proc.returncode == 0:
                    from clavus.sync import load_remotes
                    remotes = load_remotes(self.store)
                    self._peer_name = remotes[0].name if remotes else ""
                    self._update_header()
                self._status("remote list" if not action else f"remote {action}")
        except Exception as e:
            self._status(f"remote failed: {e}")

    def _run_branch(self, action: str = ""):
        """List or switch branches."""
        import subprocess, sys
        try:
            cmd = [sys.executable, "-m", "clavus", "branch"]
            if action:
                cmd.append(action)
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
            out = proc.stdout.strip()
            if out:
                for line in out.split("\n")[:5]:
                    self._log_event(line)
            self._status("branches" if not action else f"branch {action}")
        except Exception as e:
            self._status(f"branch failed: {e}")

    async def _run_snapshot_for_inject(self):
        """Take a snapshot after inject to save markers to blob store."""
        import asyncio, sys
        try:
            proc = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "clavus", "snapshot",
                "injected markers",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=30)
            out = stdout.decode().strip()
            if proc.returncode == 0:
                self._log_event("● markers saved to snapshot")
                self._load_snapshots_from_disk()
                self._render()
            else:
                self._log_event(f"snapshot after inject failed: {out[:80]}")
        except Exception as e:
            self._log_event(f"snapshot after inject error: {e}")

    def _run_backup(self):
        """Backup the entire Clavus store."""
        import subprocess, sys
        try:
            proc = subprocess.run(
                [sys.executable, "-m", "clavus", "backup"],
                capture_output=True, text=True, timeout=30,
            )
            out = proc.stdout.strip().split("\n")
            self._status(out[0] if out else "backup complete")
        except Exception as e:
            self._status(f"backup failed: {e}")

    def _run_list_backups(self):
        """List available store backups."""
        import subprocess, sys
        try:
            proc = subprocess.run(
                [sys.executable, "-m", "clavus", "backups"],
                capture_output=True, text=True, timeout=10,
            )
            out = proc.stdout.strip()
            lines = [l.strip() for l in out.split("\n") if l.strip()][:5]
            self._status(" | ".join(lines) if lines else out[:60])
        except Exception as e:
            self._status(f"backups failed: {e}")

    def _run_restore_store(self, archive_path: str = ""):
        """Restore Clavus store from a backup archive."""
        import subprocess, sys
        cmd = [sys.executable, "-m", "clavus", "restore-store"]
        if archive_path:
            cmd.append(archive_path)
        try:
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            out = (proc.stdout + proc.stderr).strip().split("\n")[-1][:60]
            self._status(out or "restore complete")
        except Exception as e:
            self._status(f"restore failed: {e}")

    @work(exclusive=False)
    async def _run_snapshot(self, message: str = ""):
        """Create a new snapshot — in-process, no subprocess."""
        if not self.project:
            self._status("no project selected")
            return
        if not message:
            self._status("usage: :snapshot <message>")
            return
        self._sync_status = "creating snapshot..."
        self._update_header()
        await asyncio.sleep(0)
        snap_hash = None
        try:
            from clavus.cli import create_snapshot
            snap_hash, logs = create_snapshot(message, allow_frozen=True)
            for line in logs:
                self._log_event(line)
            if snap_hash:
                self._sync_status = "● snap ✓"
                status_text = "● snap ✓"
                asyncio.create_task(self._delayed_clear_snapshot_status(status_text))
                self._log_event(f"● {snap_hash[:10]} — '{message}'")
            else:
                # Surface the actual reason — show in header AND footer with longer timeout
                reason = "no changes or error"
                for line in logs:
                    if "No changes" in line:
                        reason = "no changes — save project first"
                        break
                    elif "frozen" in line:
                        reason = "frozen tracks — unfreeze first"
                        break
                    elif ".als file not found" in line:
                        reason = ".als missing — open & save in Ableton first"
                        break
                self._sync_status = f"● skipped: {reason}"
                self._log_event(f"● skipped: {reason}")
                asyncio.create_task(self._delayed_clear_snapshot_status(self._sync_status))
        except Exception as e:
            self._sync_status = "● error"
            self._log_event(f"snapshot error: {e}")
            asyncio.create_task(self._delayed_clear_snapshot_status("● error"))
        # Auto-push snapshot to relay if connected (debounced to avoid flooding)
        if snap_hash and self._peer_reachable:
            now = time.time()
            if now - getattr(self, '_last_auto_push', 0) > 5:
                self._last_auto_push = now
                self._status(f"● {snap_hash[:10]} — 'auto-pushing to relay...'")
                self._log_event(f"auto-push: {snap_hash[:8]}")
                asyncio.create_task(self._do_push())
        # Reload snapshots from disk and refresh UI
        self._load_snapshots_from_disk()
        self._update_header()
        self._render()

    # ─── Cue operations ────────────────────────────────────────────────

    def _do_reply(self, text: str):
        cue = self._get_cue()
        if not cue:
            return
        cue.replies.append(Reply(
            id=hashlib.sha256(f"{time.time()}{text}".encode()).hexdigest()[:10],
            author=self.author, text=text, timestamp=time.time(),
        ))
        self._render()
        self._status("reply added")
        self._log_event(f"replied to @{cue.position}")
        self._save()

    def _do_new_cue(self, text: str, pos: str = "1.1.1"):
        if not text:
            return
        pos = pos.strip() or "1.1.1"
        self.cues.append(Cue(
            id=str(int(time.time() * 1000)),
            position=pos, text=text, author=self.author,
            status="pending", timestamp=time.time(),
        ))
        self.idx = len(self.cues) - 1
        self._render()
        self._status(f"cue added @ {pos}")
        self._log_event(f"cue: {text[:30]} @ {pos}")
        self._save()

    def _do_edit(self, text: str):
        cue = self._get_cue()
        if not cue:
            return
        cue.text = text
        self._render()
        self._status("edited")
        self._log_event(f"edited cue: {text[:30]}")
        self._save()

    def _do_assign(self, name: str):
        cue = self._get_cue()
        if not cue:
            return
        cue.assignee = name.strip()
        cue.in_progress = False
        self._render()
        self._status(f"assigned to {cue.assignee}")
        self._log_event(f"assigned to {cue.assignee}")
        self._save()

    def _do_edit_snapshot(self, text: str):
        """Update the selected snapshot's message."""
        if not self.snaps:
            self._status("no snapshots to edit")
            return
        idx = self._get_history_idx()
        snap = self.snaps[idx]
        old_msg = snap.message
        full = snap.full_hash or snap.hash  # full_hash is the real SHA256
        try:
            meta_dir = self.store.objects_dir / full[:2]
            meta_path = meta_dir / f"{full}.meta"
            if meta_path.exists():
                meta = json.loads(meta_path.read_text())
            else:
                meta = {}
            meta["message"] = text
            meta_dir.mkdir(parents=True, exist_ok=True)
            meta_path.write_text(json.dumps(meta, indent=2, default=str))
            self._load_snapshots_from_disk()
            self._render()
            self._status(f"snapshot renamed: '{text[:40]}'")
            self._log_event(f"renamed snapshot {full[:12]}: {old_msg[:30]} → {text[:30]}")
        except Exception as e:
            self._status(f"edit failed: {e}")
            self._log_event(f"snapshot edit failed: {e}")

    def action_reply(self):
        cue = self._get_cue()
        if not cue:
            self._status("select a cue first")
            return
        self._show_input("reply", f"Reply to @{cue.position}:")

    def action_cue_new(self):
        self._show_input("cue", "New cue text:")

    def action_edit(self):
        cue = self._get_cue()
        if not cue:
            self._status("select a cue first")
            return
        self._show_input("edit", "Edit:", prefill=cue.text)

    def action_resolve(self):
        cue = self._get_cue()
        if not cue:
            self._status("select a cue first")
            return
        cue.status = "resolved" if cue.status == "pending" else "pending"
        self._render()
        self._status("resolved" if cue.status == "resolved" else "unresolved")
        self._save()

    def action_resolve_conflict(self):
        """Resolve a sync conflict — cues if cue pane focused, snapshots if history pane."""
        target = self._focused_list_view()

        # Snapshot message conflict (history pane)
        if target and target.id == "hlv" and self.snaps:
            idx = self._get_history_idx()
            snap = self.snaps[idx]
            if not snap.conflict_message:
                self._status("no conflict to resolve")
                return
            self._resolve_snapshot_conflict(snap)
            return

        # Cue conflict (cue pane)
        cue = self._get_cue()
        if not cue or not cue.conflict:
            self._status("no conflict to resolve")
            return

        from textual.screen import Screen
        from textual.widgets import Static, Button, Footer
        from textual.binding import Binding as ScrBinding
        from textual.containers import Horizontal, Vertical

        local_ts = time.strftime("%m/%d %H:%M", time.localtime(cue.timestamp)) if cue.timestamp else ""
        remote_ts = time.strftime("%m/%d %H:%M", time.localtime(cue.conflict["timestamp"])) if cue.conflict.get("timestamp") else ""

        class ConflictScreen(Screen):
            BINDINGS = [
                ScrBinding("escape", "dismiss", "Close"),
                ScrBinding("q", "dismiss", "Close"),
            ]

            def __init__(self_, parent, cue_):
                super().__init__()
                self_._parent = parent
                self_._cue = cue_

            def compose(self_):
                c = self_._cue.conflict
                yield Static(
                    f"[bold {C['yellow']}]⚠ Sync Conflict[/]\n"
                    f"  {self_._cue.id[:12]} @{self_._cue.position}\n\n"
                    f"[{C['green']}]Yours (local)[/] [{C['muted']}]{local_ts}[/]\n"
                    f"  [{C['fg']}]{self_._cue.text[:80]}[/]\n"
                    f"  status: [{C['yellow']}]{self_._cue.status}[/]\n\n"
                    f"[{C['accent']}]Theirs (remote)[/] [{C['muted']}]{remote_ts}[/]\n"
                    f"  [{C['fg']}]{c['text'][:80]}[/]\n"
                    f"  status: [{C['yellow']}]{c['status']}[/]  author: {c.get('author', '?')}\n",
                    id="conflict-info"
                )
                with Horizontal(classes="conflict-actions"):
                    yield Button("Keep Mine", id="keep-mine", variant="primary")
                    yield Button("Keep Theirs", id="keep-theirs", variant="warning")
                yield Footer()

            def on_button_pressed(self_, event: Button.Pressed):
                if event.button.id == "keep-mine":
                    self_._cue.conflict = None
                    self_._cue.timestamp = time.time()  # bump so other side accepts as winner
                    self_._parent._save()
                    self_._parent._load_cues_from_disk()
                    self_._parent._render()
                    self_._parent._status("kept local version")
                elif event.button.id == "keep-theirs":
                    c = self_._cue.conflict
                    self_._cue.text = c["text"]
                    self_._cue.status = c["status"]
                    self_._cue.position = c["position"]
                    self_._cue.assignee = c.get("assignee", "")
                    self_._cue.timestamp = time.time()  # bump so other side accepts as winner
                    self_._cue.conflict = None
                    self_._parent._save()
                    self_._parent._load_cues_from_disk()
                    self_._parent._render()
                    self_._parent._status("kept remote version")
                self_.dismiss()

            def action_dismiss(self_):
                self_.dismiss()

        self.push_screen(ConflictScreen(self, cue))

    def _resolve_snapshot_conflict(self, snap: Snap):
        """Open a conflict resolution modal for snapshot message conflicts."""
        from textual.screen import Screen
        from textual.widgets import Static, Button, Footer
        from textual.binding import Binding as ScrBinding
        from textual.containers import Horizontal, Vertical

        class SnapConflictScreen(Screen):
            BINDINGS = [
                ScrBinding("escape", "dismiss", "Close"),
                ScrBinding("q", "dismiss", "Close"),
            ]

            def __init__(self_, parent, snap_):
                super().__init__()
                self_._parent = parent
                self_._snap = snap_

            def compose(self_):
                yield Static(
                    f"[bold {C['yellow']}]⚠ Snapshot Message Conflict[/]\n"
                    f"  Snapshot: {self_._snap.hash[:12]}\n\n"
                    f"[{C['green']}]Yours (local):[/]\n"
                    f"  [{C['fg']}]{self_._snap.message[:80]}[/]\n\n"
                    f"[{C['accent']}]Theirs (remote):[/]\n"
                    f"  [{C['fg']}]{self_._snap.conflict_message[:80]}[/]\n",
                    id="conflict-info"
                )
                with Horizontal(classes="conflict-actions"):
                    yield Button("Keep Mine", id="keep-mine", variant="primary")
                    yield Button("Keep Theirs", id="keep-theirs", variant="warning")
                yield Footer()

            def on_button_pressed(self_, event: Button.Pressed):
                if event.button.id == "keep-mine":
                    self_._parent._clear_snap_conflict(self_._snap, keep_mine=True)
                    self_._parent._status("kept local message")
                elif event.button.id == "keep-theirs":
                    self_._parent._clear_snap_conflict(self_._snap, keep_mine=False)
                    self_._parent._status("kept remote message")
                self_.dismiss()

            def action_dismiss(self_):
                self_.dismiss()

        self.push_screen(SnapConflictScreen(self, snap))

    def _clear_snap_conflict(self, snap: Snap, keep_mine: bool):
        """Persist conflict resolution for a snapshot message."""
        try:
            full = snap.full_hash or snap.hash
            meta_dir = self.store.objects_dir / full[:2]
            meta_path = meta_dir / f"{full}.meta"
            if meta_path.exists():
                meta = json.loads(meta_path.read_text())
            else:
                meta = {}
            if not keep_mine:
                meta["message"] = snap.conflict_message
            meta["conflict_message"] = None
            meta_path.write_text(json.dumps(meta, indent=2, default=str))
            self._load_snapshots_from_disk()
            self._render()
        except Exception as e:
            self._status(f"resolve failed: {e}")
            self._log_event(f"snap conflict resolve failed: {e}")

    def action_inject_cues(self):
        """Inject unresolved cues as Ableton markers."""
        if not self.project:
            self._status("no project selected")
            return
        if not self.cues:
            self._status("no cues to inject")
            return
        self._status("injecting cues...")
        self._run_inject()

    def action_restore_snapshot(self):
        """Restore the most recently selected snapshot from the history pane."""
        if not self.snaps:
            self._status("no snapshots to restore from")
            return
        idx = self._get_history_idx()
        snap = self.snaps[idx]
        self._status(f"restoring to {snap.hash} ('{snap.message[:40]}')...")
        self._log_event(f"restoring to {snap.hash[:12]}...")
        self._run_restore(snap.hash)

    def action_open_selected_or_head(self):
        """Open in Ableton: selected snapshot if history focused, HEAD otherwise."""
        target = self._focused_list_view()
        if target and target.id == "hlv" and self.snaps:
            # History pane focused — open the selected snapshot
            idx = self._get_history_idx()
            snap = self.snaps[idx]
            self._status(f"opening snapshot {snap.hash[:12]} ('{snap.message[:40]}')...")
            self._log_event(f"opening snapshot {snap.hash[:12]}")
            self._run_open(snap.hash)
        else:
            # Open HEAD
            self._run_open("")

    def action_edit_item(self):
        """Edit the selected item: cues if cue pane focused, snapshots if history pane."""
        target = self._focused_list_view()
        self._log_event(f"DEBUG action_edit_item: target={target.id if target else None} snaps={len(self.snaps)}")
        if target and target.id == "hlv" and self.snaps:
            # History pane — edit snapshot message via footer input
            idx = self._get_history_idx()
            snap = self.snaps[idx]
            self._show_input("edit_snapshot", f"Edit message ({snap.hash[:12]}):", prefill=snap.message)
        elif target and target.id == "clv":
            # Cue pane — edit selected cue
            self.action_edit()
        else:
            self._status("select an item to edit")

    def action_diff(self):
        """Show what changed in the selected snapshot vs its parent."""
        if not self.snaps:
            self._status("no snapshots to diff")
            return
        idx = self._get_history_idx()
        snap = self.snaps[idx]

        # Load the snapshot and its parent
        current_snap = self.store.load_snapshot(snap.full_hash or snap.hash)
        if not current_snap or not current_snap.parent:
            self._status("no parent snapshot to diff against")
            return

        parent_project = self.store.load_project(current_snap.parent)
        current_project = self.store.load_project(snap.full_hash or snap.hash)
        if not parent_project or not current_project:
            self._status("could not load project data")
            return

        from clavus.store import diff_projects
        diff = diff_projects(parent_project, current_project)

        buf = io.StringIO()
        buf.write(f"  {current_snap.short_hash()} — '{current_snap.message}'\n\n")

        # Changed tracks
        changed = [t for t in diff.tracks if t.status != "unchanged"]
        if changed:
            for td in changed:
                icon = {"added": "+", "removed": "-", "modified": "~"}.get(td.status, " ")
                buf.write(f"  {icon} {td.name}\n")
                details = []
                if td.devices_added:
                    details.append(f"+{', '.join(d[:12] for d in td.devices_added[:2])}")
                if td.devices_removed:
                    details.append(f"-{', '.join(d[:12] for d in td.devices_removed[:2])}")
                if td.clips_changed:
                    delta = td.clips_after - td.clips_before
                    sign = "+" if delta > 0 else ""
                    details.append(f"{sign}{delta} clips")
                if details:
                    buf.write(f"    {', '.join(details)}\n")
        else:
            buf.write("  (no structural track changes)\n")

        # Summary
        added = [t for t in changed if t.status == "added"]
        removed = [t for t in changed if t.status == "removed"]
        modified = [t for t in changed if t.status == "modified"]
        parts = []
        if added:
            parts.append(f"+{len(added)} tracks")
        if removed:
            parts.append(f"-{len(removed)} tracks")
        if modified:
            parts.append(f"~{len(modified)} modified")
        if parts:
            buf.write(f"\n  {' | '.join(parts)}\n")

        from textual.screen import Screen
        from textual.widgets import Static, Footer
        from textual.binding import Binding as ScrBinding

        class DiffScreen(Screen):
            BINDINGS = [
                ScrBinding("escape", "dismiss_popup", "Close"),
                ScrBinding("q", "dismiss_popup", "Close"),
                ScrBinding("d", "dismiss_popup", "Close"),
            ]
            def compose(self):
                yield Static(buf.getvalue(), id="diff-output")
                yield Footer()
            def action_dismiss_popup(self):
                self.app.pop_screen()

        self.push_screen(DiffScreen())

    def action_snapshot(self):
        """Quick-snapshot — instant capture with auto-timestamp. No prompt."""
        if self._input_mode:
            return  # don't snapshot while typing an input
        ts = time.strftime("%H:%M")
        self._run_snapshot(f"snap {ts}")

    def action_help(self):
        """Show full key bindings and commands overlay."""
        self.push_screen(HelpScreen())

    def action_assign(self):
        if self._input_mode or time.time() - self._input_debounce < 0.3:
            return  # ignore while input active or within 300ms of dismiss
        cue = self._get_cue()
        if not cue:
            return
        if cue.assignee:
            cue.assignee = ""
            cue.in_progress = False
            self._status("unassigned")
            self._render()
            self._save()
            return
        # Toggle: if already assigned to someone, unassign. Otherwise prompt for name.
        self._show_input("assign", "assign to:")

    def action_start(self):
        """Toggle cue in_progress (start/stop playback marker)."""
        cue = self._get_cue()
        if not cue:
            return
        if self._input_mode:
            # Don't toggle while input prompt is active
            return
        if cue.in_progress:
            cue.in_progress = False
            self._status("paused")
        else:
            cue.in_progress = True
            self._status("in progress")
        self._render()
        self._save()

    @work(exclusive=True)
    async def action_archive(self):
        """Archive the selected cue — ask for confirmation first."""
        cue = self._get_cue()
        if not cue:
            self._status("select a cue first")
            return
        if not self.project:
            self._status("no project selected")
            return
        self._show_input("confirm_archive",
                         f"archive @{cue.position} '{cue.text[:30]}'? (y/N) ▼",
                         prefill="n")

    def _do_archive_cue(self):
        """Actually archive the cue (after confirmation)."""
        cue = self._get_cue()
        if not cue:
            return
        if not self._cue_store:
            self._status("no cue store")
            return
        self._status(f"archiving {cue.id[:8]}...")
        try:
            cue.status = "archived"
            self._cue_store._save_cue(cue)
            self._status("archived")
            self._log_event(f"archived @{cue.position}")
            self._load_cues_from_disk()
            self._render()
        except Exception as e:
            self._status(f"archive failed: {e}")

    @work(exclusive=True)
    async def action_delete_cue(self):
        """Delete the selected cue permanently — ask for confirmation first."""
        cue = self._get_cue()
        if not cue:
            self._status("select a cue first")
            return
        if not self.project:
            self._status("no project selected")
            return
        self._show_input("confirm_delete",
                         f"delete @{cue.position} '{cue.text[:30]}' — PERMANENT? (y/N) ▼",
                         prefill="n")

    def _do_delete_cue(self):
        """Actually delete the cue (after confirmation)."""
        cue = self._get_cue()
        if not cue:
            return
        if not self._cue_store:
            self._status("no cue store")
            return
        self._status(f"deleting {cue.id[:8]}...")
        try:
            self._cue_store.delete(cue.id)
            self._status("deleted")
            self._log_event(f"deleted @{cue.position}")
            self._load_cues_from_disk()
            self._render()
        except Exception as e:
            self._status(f"delete failed: {e}")

    def _run_share(self):
        """Start a relay and show connection URLs."""
        from clavus.config import ClavusConfig
        import subprocess, os

        cfg = ClavusConfig.load()
        lan_ip = self._lan_ip()
        tailscale_ip = self._tailscale_ip()
        port = cfg.port

        # Spawn relay server
        proc = subprocess.Popen(
            [sys.executable, "-m", "clavus", "relay", "--port", str(port)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        def stop_relay():
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()

        self.push_screen(ShareModal(lan_ip, tailscale_ip, port, stop_relay))

    @staticmethod
    def _lan_ip() -> str:
        """Get LAN IP for displaying in share banner."""
        try:
            s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            s.connect(("8.8.8.8", 80))
            ip = s.getsockname()[0]
            s.close()
            return ip
        except Exception:
            return "your-lan-ip"

    @staticmethod
    def _tailscale_ip() -> str:
        """Get Tailscale IP if available, empty string otherwise."""
        try:
            import subprocess
            r = subprocess.run(["tailscale", "ip", "-4"], capture_output=True, text=True, timeout=5)
            return r.stdout.strip() if r.returncode == 0 else ""
        except Exception:
            return ""

    def _start_relay(self) -> subprocess.Popen | None:
        """Start a relay server in the background."""
        port = self._clavus_cfg.port
        try:
            proc = subprocess.Popen(
                [sys.executable, "-m", "clavus", "relay", "--port", str(port)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            return proc
        except Exception as e:
            self._status(f"failed to start relay: {e}")
            return None

    def on_exit(self):
        """Cleanup: kill auto-started relay."""
        if self._relay_proc and self._relay_proc.poll() is None:
            self._relay_proc.terminate()
            try:
                self._relay_proc.wait(timeout=3)
            except Exception:
                self._relay_proc.kill()
            self._relay_proc = None

    @work
    async def action_pull(self):
        self._busy = True
        self._status("\u23f3 pulling...")
        try:
            await self._do_pull()
        finally:
            self._busy = False
            self._update_header()
            self.refresh()

    @work
    async def action_push(self):
        self._busy = True
        self._status("\u23f3 pushing...")
        try:
            await self._do_push()
        finally:
            self._busy = False
            self._update_header()
            self.refresh()

    @work(exclusive=True)
    async def action_stem_push(self):
        self._status("pushing stems...")
        import asyncio
        proc = await asyncio.create_subprocess_shell(
            "clavus stem push",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=300)
        out = stdout.decode().strip()
        err = stderr.decode().strip()
        msg = out.split("\n")[-1][:60] if out else (err.split("\n")[-1][:60] if err else "done")
        self._status(f"stems: {msg}")
        self._log_event(f"stems: {msg}")

    # ─── Persistence ────────────────────────────────────────────────────

    def _save(self):
        """Save cues to disk directly."""
        if not self.project or not self._cue_store:
            return
        try:
            for cue in self.cues:
                self._cue_store._save_cue(cue)
            self._status("saved")
        except Exception as e:
            self._status(f"save failed: {e}")
            self._log_event(f"save failed: {e}")

    def _probe_reachability(self):
        """Periodic health check — updates dot color based on relay reachability.
        Also detects new data pushed from collaborators and auto-refreshes the UI."""
        if not self._peer_name:
            return
        try:
            from clavus.sync import load_remotes
            remotes = load_remotes(self.store)
            if not remotes:
                return
            import urllib.request
            url = remotes[0].url.rstrip("/") + "/api/ping"
            req = urllib.request.Request(url)
            r = urllib.request.urlopen(req, timeout=2)
            was_reachable = self._peer_reachable
            self._peer_reachable = (r.status == 200)
            if was_reachable != self._peer_reachable:
                self._update_header()
            # Auto-refresh: check if new data arrived from a collaborator push
            if self._peer_reachable and self.project:
                self._auto_refresh_if_changed(remotes[0])
        except Exception:
            if self._peer_reachable:
                self._peer_reachable = False
                self._update_header()

    def _auto_refresh_if_changed(self, remote=None):
        """Check local store for new cues/snapshots pushed from a collaborator.
        If anything changed, reload and re-render. When HEAD moved (new
        snapshot arrived), also download missing blobs and reconstruct
        the .als file so the user can open/restore it.
        (The relay shares ~/.clavus/ with this TUI process.)"""
        try:
            # Check if cues changed
            from clavus.cues import CueStore, CueFilter
            cue_store = CueStore(self.project, store=self.store)
            current_cues = cue_store.list_cues(CueFilter())
            active_count = sum(1 for c in current_cues if c.status != "archived")
            
            # Check if HEAD moved (new snapshot pushed)
            proj = self.store.get_index(self.project)
            current_head = proj.head if proj else None
            
            cue_changed = active_count != len(self.cues)
            head_changed = current_head != getattr(self, '_last_auto_head', None)
            
            if cue_changed or head_changed:
                prev_head = getattr(self, '_last_auto_head', None)
                self._last_auto_head = current_head
                self._log_event(f"auto-refresh: cues {len(self.cues)}→{active_count}, head {str(prev_head or '?')[:8]}→{str(current_head or '?')[:8]}")
                
                # If HEAD moved, pull missing blobs and reconstruct .als on disk
                if head_changed and current_head and remote:
                    try:
                        from clavus.sync import pull_snapshot_blobs
                        pull_snapshot_blobs(self.store, proj, remote)
                    except Exception:
                        pass  # best-effort — don't break UI refresh on blob failure
                
                self._load_cues_from_disk()
                self._load_snapshots_from_disk()
                self._last_snap_time = time.time()
                # Only re-render if enough time passed since last render
                # (prevents rapid clear-rebuild flicker on Windows)
                if time.time() - getattr(self, '_last_auto_render', 0) > 2:
                    self._last_auto_render = time.time()
                    self._render_cues()
                    self._render_history()
                    self._update_header()
                    self._update_footer()
        except Exception:
            pass  # auto-refresh is best-effort

    # ─── Connection ─────────────────────────────────────────────────────

    def _connect(self):
        """Load project data directly from the local store (no web server needed)."""
        self._status("loading from disk...")
        projects = self.store.list_projects()
        if not projects:
            # Check if remotes are configured — if so, guide to pull
            try:
                from clavus.sync import load_remotes
                remotes = load_remotes(self.store)
                if remotes:
                    self._peer_name = remotes[0].name
                    self._peer_reachable = False  # yellow until proven
                    self._status("connected — press p to pull projects")
                    self._log_event("remotes configured — press p to pull")
                else:
                    self._status("no project — use :join http://IP:PORT or :init <path>")
                    self._log_event("no remotes — use :join http://IP:PORT")
            except Exception:
                self._status("no project — use :init <path> to add one")
            self._update_header()
            self._update_footer()
            # Show helpful empty-state message in the cue list area
            try:
                lv = self.query_one("#clv", ListView)
                lv.clear()
                msg = f"  [{C['dim']}]no project — type [{C['accent']}]:join http://IP:PORT[/] or [{C['accent']}]:init /path/to/project.als[/]"
                lv.append(ListItem(Label(msg)))
            except NoMatches:
                pass
            return

        # Use _last_project from index, fall back to first available
        target = ""
        if self.store.index_path.exists():
            index = json.loads(self.store.index_path.read_text())
            target = index.get("_last_project", "")

        match = None
        if target:
            match = self.store.get_index(target)
        if not match:
            match = projects[0]

        self.project = match.name
        self._status(f"loaded: {self.project}")
        # Detect peer name from remotes
        from clavus.sync import load_remotes
        remotes = load_remotes(self.store)
        self._peer_name = remotes[0].name if remotes else ""
        self._peer_reachable = False
        # Quick health probe — use urllib (fast, no httpx overhead)
        if self._peer_name and remotes:
            try:
                import urllib.request
                url = remotes[0].url.rstrip("/") + "/api/ping"
                req = urllib.request.Request(url)
                r = urllib.request.urlopen(req, timeout=2)
                if r.status == 200:
                    self._peer_reachable = True
                    self._log_event(f"\u25cf {self._peer_name} reachable")
            except Exception:
                pass
        self._update_footer()

        # Load cues from disk
        self._load_cues_from_disk()
        # Load snapshots from store
        self._load_snapshots_from_disk()
        # Auto-create initial snapshot if this project has none
        self._ensure_initial_snapshot()
        # Peer dot: only green after confirmed sync. Don't auto-green just because
        # we have local data — that tells the user lies about reachability.
        self._peer_reachable = False
        self._update_header()
        self._status(f"{len(self.cues)} cues, {len(self.snaps)} snapshots")
        self._render()

    def _load_cues_from_disk(self):
        """Load cues for the current project from CueStore."""
        if not self.project:
            self._log_event("_load_cues: no project set")
            return
        from clavus.cues import CueStore, CueFilter
        cue_store = CueStore(self.project, store=self.store)
        self._cue_store = cue_store
        all_cues = cue_store.list_cues(CueFilter())
        # Split archived from active — archived are hidden from the cue list
        self._archived_count = sum(1 for c in all_cues if c.status == "archived")
        active_cues = [c for c in all_cues if c.status != "archived"]
        self._log_event(f"_load_cues: {len(active_cues)} active cue(s) + {self._archived_count} archived from {cue_store.cues_dir}")
        self.cues = self._sort_cues(active_cues)
        self.idx = min(self.idx, len(self.cues) - 1) if self.cues else 0
        self._cue_fingerprint = None  # invalidate render cache

    def _load_snapshots_from_disk(self):
        """Load snapshot history for the current project from BlobStore."""
        if not self.project:
            return
        proj = self.store.get_index(self.project)
        if not proj or not proj.head:
            self.snaps = []
            return
        history = []
        current = proj.head
        seen: set = set()
        while current:
            if current in seen:
                break
            seen.add(current)
            snap = self.store.load_snapshot(current)
            if not snap:
                break
            history.append(snap)
            if snap.parent == current:
                break
            current = snap.parent
        self.snaps = [
            Snap(
                hash=s.hash[:10],
                full_hash=s.hash,
                message=s.message,
                timestamp=s.timestamp,
                track_count=s.track_count,
                conflict_message=getattr(s, 'conflict_message', None),
            )
            for s in history
        ]
        if self.snaps:
            self._last_snap_time = self.snaps[0].timestamp
        self._snap_fingerprint = None  # invalidate render cache
        self._render_history()

    def _ensure_initial_snapshot(self):
        """Auto-create a snapshot if this project has none — baseline to restore from."""
        if not self.project or self.snaps:
            return
        try:
            proj = self.store.get_index(self.project)
            if not proj or not proj.root_als:
                return
            from pathlib import Path
            als_path = Path(proj.root_als)
            if not als_path.exists():
                return
            from clavus.cli import create_snapshot
            snap_hash, logs = create_snapshot("initial", allow_frozen=True)
            if snap_hash:
                self._log_event(f"● initial snapshot {snap_hash[:10]} — baseline saved")
                self._load_snapshots_from_disk()
            else:
                # Not an error — project might have no changes yet
                pass
        except Exception:
            pass  # don't block project load on snapshot failure

    def _sort_cues(self, cues: list[Cue]) -> list[Cue]:
        """Sort cues by timeline position, then by creation timestamp.

        Handles both position formats:
          bars.beats.sixteenths (e.g. "5.1.1")
          bars:beats           (e.g. "3:45")
        """
        def sort_key(c: Cue) -> tuple:
            pos = c.position or ""
            if ":" in pos:
                # bars:beats format
                parts = pos.split(":")
                bars = int(parts[0]) if parts[0] else 0
                beats = int(parts[1]) if len(parts) > 1 and parts[1] else 0
                return (bars, beats, 0, c.timestamp)
            # bars.beats.sixteenths format
            parts = pos.split(".")
            bars = int(parts[0]) if len(parts) > 0 and parts[0] else 0
            beats = int(parts[1]) if len(parts) > 1 and parts[1] else 0
            sixteenths = int(parts[2]) if len(parts) > 2 and parts[2] else 0
            return (bars, beats, sixteenths, c.timestamp)
        return sorted(cues, key=sort_key)

    async def _do_pull(self):
        """Pull cues + snapshots + blobs from remotes — auto-discovers projects if none local."""
        import asyncio
        import time
        from pathlib import Path
        from clavus.sync import load_remotes, pull_from_remote, pull_snapshot_blobs, SyncClient
        from clavus.store import ClavusProject
        try:
            proj_index = self.store.get_index(self.project) if self.project else None
            remotes = load_remotes(self.store)
            if not remotes:
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                self._status("\u274c no remotes — use :join http://...")
                return
            self._sync_status = f"\u2b07 {time.strftime('%H:%M')} pulling..."
            self._update_header()
            await asyncio.sleep(0)
            self._status("\u2b07 pulling...")

            # If no local project, auto-discover from remotes
            if not proj_index:
                self._sync_status = f"\u2b07 {time.strftime('%H:%M')} probing {len(remotes)} remote(s)..."
                self._update_header()
                await asyncio.sleep(0)
                # Try external remotes first (localhost is slow/unreachable on Windows)
                remotes_sorted = sorted(remotes, key=lambda r: 0 if "localhost" in r.url else -1)
                pulled_any = False
                for remote in remotes_sorted:
                    self._sync_status = f"\u2b07 {time.strftime('%H:%M')} {remote.name}..."
                    self._update_header()
                    await asyncio.sleep(0)
                    client = SyncClient(remote.url)
                    try:
                        r, err = client.request_with_retry("GET", "/api/projects", timeout=10)
                        if r is None or r.status_code != 200:
                            continue
                        projects = r.json().get("projects", [])
                        if not projects:
                            continue
                        for pdata in projects:
                            pname = pdata["name"]
                            if self.store.get_index(pname):
                                proj_index = self.store.get_index(pname)
                            else:
                                self._sync_status = f"\u2b07 {time.strftime('%H:%M')} {pname}..."
                                self._update_header()
                                await asyncio.sleep(0)
                                r2, _ = client.request_with_retry(
                                    "GET", "/api/sync/pull",
                                    params={"name": pname}, timeout=30)
                                if r2 is None or r2.status_code != 200:
                                    continue
                                info = r2.json().get("project", {})
                                new_proj = ClavusProject(
                                    name=pname,
                                    root_als=info.get("root_als", f"~/{pname}/{pname}.als"),
                                    created_at=time.time(),
                                )
                                self.store.set_index(new_proj)
                                proj_index = new_proj
                            # Pull data
                            remote_ref = remote
                            result = pull_from_remote(self.store, proj_index, remote_ref)
                            blob_count = pull_snapshot_blobs(self.store, proj_index, remote_ref)
                            parts = []
                            if result.get("cues"): parts.append(f"{result['cues']}c")
                            if result.get("snapshots"): parts.append(f"{result['snapshots']}s")
                            if blob_count: parts.append(f"{blob_count}b")
                            self._sync_status = f"\u2b07 {time.strftime('%H:%M')} {pname}  {' '.join(parts)}" if parts else f"\u2b07 {time.strftime('%H:%M')} {pname}  up to date"
                            self._update_header()
                            await asyncio.sleep(0)
                            pulled_any = True
                        if pulled_any:
                            break
                    except Exception as e:
                        self._sync_status = f"\u2b07 {time.strftime('%H:%M')} {remote.name}: err"
                        self._update_header()
                        await asyncio.sleep(0)
                        continue
                    finally:
                        client.close()

                if not pulled_any:
                    self._sync_status = ""
                    self._update_header()
                    await asyncio.sleep(0)
                    self._status("\u274c no projects found on any remote")
                    return

                # Switch to the pulled project
                if proj_index:
                    self.project = proj_index.name
                    self._peer_name = remotes[0].name if remotes else ""
                    self._peer_reachable = True
                    self._last_sync = f"\u2b07 {time.strftime('%H:%M')}"
                    self._load_cues_from_disk()
                    self._load_snapshots_from_disk()
                    self._update_header()
                    asyncio.create_task(self._delayed_clear_sync())
                    await asyncio.sleep(0)
                    self._render()
                    self._status(f"\u2705 pulled {self.project}: {len(self.cues)} cues, {len(self.snaps)} snaps")
                return

            # ── Normal pull for existing project ──
            # Auto-snapshot local work before overwriting with remote changes.
            # This ensures the user can always go back to what they had.
            try:
                als_path = Path(proj_index.root_als)
                if als_path.exists() and proj_index.head:
                    raw_als = als_path.read_bytes()
                    current_hash = hashlib.sha256(raw_als).hexdigest()
                    if current_hash != proj_index.head:
                        from clavus import parse_als
                        project = parse_als(als_path)
                        if project:
                            snap = self.store.save_snapshot(
                                project,
                                message="auto-snapshot before sync",
                                parent=proj_index.head,
                            )
                            if snap.hash != proj_index.head:
                                self.store.update_ref("HEAD", snap.hash)
                                proj_index.head = snap.hash
                                self.store.set_index(proj_index)
                                self._log_event(f"● auto-snapshot {snap.hash[:8]} (local changes saved)")
            except Exception:
                pass  # best-effort — don't block pull on snapshot failure

            any_ok = False
            last_error = ""
            for remote in remotes:
                self._sync_status = f"\u2b07 {time.strftime('%H:%M')} {remote.name}..."
                self._update_header()
                await asyncio.sleep(0)
                result = pull_from_remote(self.store, proj_index, remote)
                if result.get("error"):
                    self._peer_reachable = False
                    self._last_sync = f"\u2b07 \u2717 {time.strftime('%H:%M')}"
                    self._sync_status = ""
                    self._update_header()
                    await asyncio.sleep(0)
                    self._status(f"● {self._peer_name} unreachable")
                    self._log_event(f"● {self._peer_name} unreachable — pull failed")
                    last_error = result["error"]
                    continue  # try next remote, don't bail
                any_ok = True
                cues_n = result.get("cues", 0)
                snaps_n = result.get("snapshots", 0)
                conflicts_n = result.get("conflicts", 0)
                blobs = pull_snapshot_blobs(self.store, proj_index, remote)
                self._sync_status = f"\u2b07 {time.strftime('%H:%M')} {remote.name}  {cues_n}c {snaps_n}s" + (f" {blobs}b" if blobs else "")
                if conflicts_n:
                    self._sync_status += f"  \u26a0{conflicts_n}"
                self._update_header()
                await asyncio.sleep(0)
                self._peer_reachable = True
            if not any_ok:
                self._last_sync = f"\u2b07 \u2717 {time.strftime('%H:%M')}"
                self._sync_status = ""
                self._update_header()
                self._status(f"❌ pull failed: {last_error[:60]}")
                self._log_event(f"pull failed: {last_error} — check relay")
                return
            self._last_sync = f"\u2b07 {time.strftime('%H:%M')}"
            self._update_header()
            asyncio.create_task(self._delayed_clear_sync())
            await asyncio.sleep(0)
            self._log_event(f"\u2b07 pulled: {len(self.cues)} cues, {len(self.snaps)} snapshots")
            # Refresh from disk
            if self.project:
                self._load_cues_from_disk()
                self._load_snapshots_from_disk()
                self._update_header()
                await asyncio.sleep(0)
                self._render()
            self.set_timer(0.05, self._update_header)
        except Exception as e:
            self._log_event(f"\u274c pull error: {e}")
            self._status(f"\u274c pull error: {e}")

    async def _do_push(self):
        """Push cues + snapshots + blobs to remotes — direct function calls, no subprocess."""
        import asyncio
        import time
        from clavus.sync import load_remotes, push_to_remote, push_snapshot_blobs
        try:
            proj_index = self.store.get_index(self.project)
            if not proj_index:
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                self._status("\u274c no project")
                return
            remotes = load_remotes(self.store)
            if not remotes:
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                self._status("\u274c no remotes configured")
                return
            self._sync_status = f"\u2b06 {time.strftime('%H:%M')} pushing..."
            self._update_header()
            await asyncio.sleep(0)
            self._status("\u2b06 pushing...")
            for remote in remotes:
                self._sync_status = f"\u2b06 {time.strftime('%H:%M')} {remote.name}..."
                self._update_header()
                await asyncio.sleep(0)
                result = push_to_remote(self.store, proj_index, remote)
                if result.get("error"):
                    self._peer_reachable = False
                    self._last_sync = f"\u2b06 \u2717 {time.strftime('%H:%M')}"
                    self._sync_status = ""
                    self._update_header()
                    await asyncio.sleep(0)
                    err = result['error']
                    if 'pull first' in err.lower() or 'conflict' in err.lower():
                        self._status(f"⚠️ conflict — press p to pull, then P to push")
                        self._log_event(f"⚠️ {err} — press p to pull, then P to push")
                    else:
                        self._status(f"❌ push failed: {err[:60]}")
                        self._log_event(f"push error: {err}")
                    return
                cues_n = result.get("cues", 0)
                snaps_n = result.get("snapshots", 0)
                blobs = push_snapshot_blobs(self.store, proj_index, remote)
                self._sync_status = f"\u2b06 {time.strftime('%H:%M')} {remote.name}  {cues_n}c {snaps_n}s" + (f" {blobs}b" if blobs else "")
                self._update_header()
                await asyncio.sleep(0)
                self._peer_reachable = True
            self._last_sync = f"\u2b06 {time.strftime('%H:%M')}"
            self._update_header()
            asyncio.create_task(self._delayed_clear_sync())
            await asyncio.sleep(0)
            self._status(f"⬆ pushed: {len(self.cues)} cues, {len(self.snaps)} snaps")
            self._log_event(f"⬆ pushed: {len(self.cues)} cues, {len(self.snaps)} snapshots")
            self.set_timer(0.05, self._update_header)
        except Exception as e:
            self._sync_status = ""
            self._update_header()
            await asyncio.sleep(0)
            self._status(f"\u274c push error: {e}")

    def _get_cue(self) -> Optional[Cue]:
        if 0 <= self.idx < len(self.cues):
            return self.cues[self.idx]
        return None

    def _get_history_idx(self) -> int:
        """Get the selected index from the history list view, or default to 0."""
        try:
            hlv = self.query_one("#hlv", ListView)
            idx = hlv.index
        except (NoMatches, AttributeError):
            idx = 0
        if idx is None or idx >= len(self.snaps):
            idx = 0
        return idx

    # ─── Navigation ─────────────────────────────────────────────────────

    def action_cursor_down(self):
        target = self._focused_list_view()
        if target:
            target.action_cursor_down()

    def action_cursor_up(self):
        target = self._focused_list_view()
        if target:
            target.action_cursor_up()

    def _focused_list_view(self) -> Optional[ListView]:
        """Return whichever ListView has focus: #clv (cues) or #hlv (history)."""
        try:
            hlv = self.query_one("#hlv", ListView)
            if hlv.has_focus:
                return hlv
        except NoMatches:
            pass
        try:
            clv = self.query_one("#clv", ListView)
            if clv.has_focus:
                return clv
        except NoMatches:
            pass
        return None

    def action_focus_next_pane(self):
        """Tab between cues list and history pane."""
        try:
            hlv = self.query_one("#hlv", ListView)
            clv = self.query_one("#clv", ListView)
            if hlv.has_focus:
                clv.focus()
            elif self.query_one("#cues-list").has_focus or clv.has_focus:
                hlv.focus()
            else:
                clv.focus()
            self._update_footer_hint()
        except NoMatches:
            pass

    def _update_footer_hint(self):
        """Context-aware hint: shows relevant keys for the focused pane."""
        hint = "? help"
        try:
            hlv = self.query_one("#hlv", ListView)
            if hlv.has_focus:
                hint = "S snap  T restore  e edit  o open  d diff  ? help"
            else:
                clv = self.query_one("#clv", ListView)
                if clv.has_focus:
                    hint = "c cue  r reply  e edit  a assign  o open  S snap  p pull  T history  ? help"
        except NoMatches:
            pass
        try:
            self.query_one("#footer-hint", Static).update(hint)
        except NoMatches:
            pass

    def on_list_view_highlighted(self, event: ListView.Highlighted):
        if event.list_view.id == "clv":
            self.idx = event.list_view.index
            self._update_footer()

    # ─── Status / Header / Footer ───────────────────────────────────────

    def _footer_toast(self, msg: str, duration: float = 4.0):
        """Write to footer, auto-restore project info after duration seconds."""
        w = self._footer_stats
        if w is not None:
            w.update(msg)
            w.refresh()
        # Cancel any pending restore, then schedule new one
        if hasattr(self, "_toast_timer") and self._toast_timer is not None:
            self._toast_timer.stop()
        self._toast_timer = self.set_timer(duration, lambda: self._update_footer())

    def _status(self, msg: str):
        """Short footer toast — auto-clears after 3s."""
        safe = msg.replace("[", "\\[").replace("]", "\\]")
        self._footer_toast(f"[{C['dim']}]{safe}[/]", 3.0)

    def _log_event(self, event: str):
        """Timestamped footer toast — auto-clears after 8s."""
        ts = time.strftime("%H:%M:%S")
        safe = event.replace("[", "\\[").replace("]", "\\]")
        self._footer_toast(f"[{C['dim']}]{ts}[/] [{C['accent']}⟩[/] {safe}", 8.0)

    def _clear_log_events(self):
        self._update_footer()

    async def _delayed_clear_sync(self):
        """Keep live sync status visible for 1.5s after sync completes."""
        await asyncio.sleep(1.5)
        self._sync_status = ""
        self._update_header()

    async def _delayed_clear_snapshot_status(self, expected: str = ""):
        """Keep snapshot result visible in header for 4s."""
        await asyncio.sleep(4)
        # Only clear if the header still shows this snapshot's result
        # (prevents clearing a newer snapshot's status)
        if self._sync_status and (not expected or self._sync_status == expected):
            self._sync_status = ""
            self._update_header()

    def _update_header(self):
        """Header: clavus logo, project, connection dot + remote, sync activity."""
        try:
            # Project name
            proj = f"  [white]{self.project}[/]" if self.project else ""
            # Connection dot — green = reachable, yellow = offline
            if self._peer_name and self._peer_reachable:
                peer = f"  [bold {C['green']}]\u25cf[/]"
            elif self._peer_name:
                peer = f"  [{C['yellow']}]\u25cb[/]"
            else:
                peer = ""
            # Sync activity — spinner during, timestamp after
            sync = ""
            if self._sync_status:
                s = self.BRAILLE[self._spinner_idx % len(self.BRAILLE)]
                sync = f"  [{C['yellow']}]{s} {self._sync_status}[/]"
            elif self._last_sync:
                sync = f"  [{C['green']}]{self._last_sync}[/]"
            widget = self.query_one("#header-title", Static)
            widget.update(f"[bold {C['accent']}]⧩ clavus[/]{proj}{peer}{sync}")
            widget.refresh()
            # Manage spinner based on sync activity (header-only, no footer cascade)
            if self._sync_status:
                self._start_spinner()
            else:
                self._stop_spinner()
        except NoMatches:
            pass

    def _update_history_label(self):
        try:
            label = self.query_one('#history-label', Static)
            n = len(self.snaps)
            text = f' History ({n})' if n else ' History'
            if self._last_snap_time:
                elapsed = time.time() - self._last_snap_time
                if elapsed < 120:
                    text += f'  [{C["dim"]}]● {int(elapsed)}s[/]'
                elif elapsed < 3600:
                    text += f'  [{C["dim"]}]● {int(elapsed // 60)}m[/]'
                elif elapsed < 86400:
                    text += f'  [{C["dim"]}]● {int(elapsed // 3600)}h[/]'
                else:
                    text += f'  [{C["dim"]}]● {int(elapsed // 86400)}d[/]'
            label.update(text)
        except NoMatches:
            pass

    def _update_footer(self):
        """Footer: project state — cues, snapshots. Sync activity lives in header."""
        try:
            status = self.query_one("#footer-status", Static)
            if not self.project:
                status.update(f"[{C['dim']}]welcome — :init <path> to open a project[/]")
                self._update_welcome()
                return

            parts = [f"[bold]{self.project}[/]"]
            self._update_welcome()  # hide welcome, show cues + history

            # Cues — always show, even 0
            n = len(self.cues)
            parts.append(f"{n} cue{'s' if n != 1 else ''}")

            # Snapshot — most recent message
            if self.snaps:
                snap = self.snaps[0]
                msg = snap.message[:30] if snap.message else ""
                parts.append(f"● '{msg}'" if msg else "●")

            status.update("  ".join(parts))
        except NoMatches:
            pass

    def _update_welcome(self):
        """Show/hide centered welcome message based on project state."""
        try:
            welcome = self.query_one("#welcome", Static)
            cues_list = self.query_one("#cues-list")
            history = self.query_one("#history")
        except NoMatches:
            return
        if not self.project:
            welcome.update(
                f"\n\n"
                f"    [{C['accent']}]clavus[/]\n\n"
                f"    [{C['dim']}]cue management for Ableton[/]\n\n"
                f"    [{C['fg']}]:init <path>[/]  open a project\n"
                f"    [{C['fg']}]:join <URL>[/]   connect to a relay\n\n"
                f"    [{C['dim']}]S snapshot  ? help  :cmd[/]\n"
            )
            welcome.styles.display = "block"
            cues_list.styles.display = "none"
            history.styles.display = "none"
        else:
            welcome.styles.display = "none"
            cues_list.styles.display = "block"
            history.styles.display = "block"


    BRAILLE = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

    def _start_spinner(self):
        """Start the braille spinner animation (updates footer every 100ms)."""
        if self._spinner_timer is not None:
            return
        self._spinner_timer = self.set_interval(0.1, self._tick_spinner)

    def _stop_spinner(self):
        """Stop the braille spinner."""
        if self._spinner_timer is not None:
            self._spinner_timer.stop()
            self._spinner_timer = None

    def _tick_spinner(self):
        """Advance spinner and refresh footer."""
        self._spinner_idx = (self._spinner_idx + 1) % len(self.BRAILLE)
        if not self._sync_status:
            self._stop_spinner()
            self._update_footer()
            return
        try:
            self._update_header()
        except Exception:
            pass

    def _focus_cues(self):
        try:
            self.query_one("#clv", ListView).focus()
        except NoMatches:
            pass

    # ─── Rendering ──────────────────────────────────────────────────────

    def _time_ago(self, ts: float) -> str:
        """Relative time string — 'just now', '2m ago', '3h ago', '2d ago'."""
        if not ts:
            return ""
        delta = time.time() - ts
        if delta < 60:
            return "just now"
        mins = int(delta // 60)
        if mins < 60:
            return f"{mins}m ago"
        hours = int(delta // 3600)
        if hours < 24:
            return f"{hours}h ago"
        days = int(delta // 86400)
        return f"{days}d ago"

    def _render(self):
        try:
            self._render_cues()
            self._render_history()
            self._update_footer()
        except Exception as e:
            self._status(f"render error: {e}")

    def _render_cues(self):
        lv = self.query_one("#clv", ListView)
        # Fingerprint: skip entire rebuild if content hasn't changed
        # (remove_children causes reflow jitter on Windows even when identical)
        fp = (len(self.cues), tuple((c.id, c.status, c.text[:60], len(c.replies), c.assignee, c.in_progress) for c in self.cues))
        if fp == getattr(self, '_cue_fingerprint', None):
            return
        self._cue_fingerprint = fp
        lv.remove_children()

        if not self.cues:
            lv.append(ListItem(Label(f"  [{C['dim']}]no cues yet — c to place one at the playhead[/]")))
            return

        for i, c in enumerate(self.cues):
            color = C["yellow"] if c.status == "pending" else (
                C["green"] if c.status == "resolved" else C["muted"])
            dot = "✓" if c.status == "resolved" else "●"
            rc = f" [{C['dim']}]{len(c.replies)}r[/]" if c.replies else ""
            assignee_part = f"  👤 {c.assignee}" if c.assignee else ""
            in_prog = f" [{C['yellow']}]▶[/]" if c.in_progress else ""
            safe_text = c.text[:60].replace("[", "\\[").replace("]", "\\]")
            conflict_mark = f" [{C['yellow']}]⚠[/]" if getattr(c, 'conflict', False) else ""
            ago = self._time_ago(c.timestamp)
            cue_line = (
                f"  [{color}]{dot}[/] [{color}]@{c.position}[/] "
                f"[{C['fg']}]{safe_text}[/]{rc}{conflict_mark}"
                + (f" [{C['dim']}]{ago}[/]" if ago else "")
            )
            lines = [cue_line]
            if assignee_part:
                lines.append(f"  [{C['dim']}]├──[/]{assignee_part}{in_prog}")
            elif in_prog:
                lines.append(f"  [{C['dim']}]├──[/]  [{C['yellow']}]▶[/]")
            if c.replies:
                for j, r in enumerate(c.replies):
                    tag = r.author or "anon"
                    r_ago = self._time_ago(r.timestamp)
                    conn = "╰─" if j == len(c.replies) - 1 else "├─"
                    safe_reply = r.text[:55].replace("[", "\\[").replace("]", "\\]")
                    lines.append(
                        f"  [{C['dim']}]{conn} {tag} [{C['muted']}]{r_ago}[/]"
                        f"  [{C['dim']}]{safe_reply}[/]"
                    )
            lv.append(ListItem(Label("\n".join(lines))))

        if self.idx < len(lv.children):
            lv.index = self.idx

    def _render_history(self):
        lv = self.query_one("#hlv", ListView)
        # Fingerprint: skip rebuild if snapshots haven't changed
        fp = tuple((s.hash, s.message, s.conflict_message) for s in self.snaps[:10])
        if fp == getattr(self, '_snap_fingerprint', None):
            return
        self._snap_fingerprint = fp
        lv.remove_children()
        if not self.snaps:
            lv.append(ListItem(Label(f"  [{C['dim']}]no snapshots yet — S to capture[/]")))
            lv.refresh()
            return
        for s in self.snaps[:10]:
            ago = self._time_ago(s.timestamp)
            safe_msg = s.message[:50].replace("[", "\\[").replace("]", "\\]")
            conflict_mark = f" [{C['yellow']}]⚠[/]" if s.conflict_message else ""
            lv.append(ListItem(Label(
                f"[{C['fg']}]{safe_msg}[/]{conflict_mark}  [{C['dim']}]{s.hash[:8]}  {ago}[/]"
            )))
        lv.refresh()


# ─── Modals ──────────────────────────────────────────────────────────────────

class ShareModal(ModalScreen[None]):
    """Modal showing relay connection URLs."""

    CSS = f"""
    ShareModal {{
        align: center middle;
    }}
    #share-box {{
        width: 52;
        max-height: 12;
        background: {C['surface']};
        border: thick {C['accent']};
        padding: 1 2;
    }}
    #share-title {{
        text-style: bold;
        color: {C['accent']};
        padding-bottom: 1;
    }}
    #share-url {{
        text-style: bold;
        color: {C['fg']};
        padding: 1 2;
        background: {C['surface2']};
    }}
    #share-hint {{
        color: {C['dim']};
        padding-top: 1;
    }}
    #share-footer {{
        color: {C['muted']};
        padding-top: 1;
        text-style: italic;
    }}
    .share-actions {{
        padding-top: 1;
        align: center middle;
    }}
    #stop-share {{
        width: 16;
    }}
    """

    BINDINGS = [
        Binding("escape", "dismiss", "Close"),
    ]

    def __init__(self, lan_ip: str, tailscale_ip: str, port: int, stop_cb) -> None:
        super().__init__()
        self.lan_ip = lan_ip
        self.tailscale_ip = tailscale_ip
        self.port = port
        self.stop_cb = stop_cb

    def compose(self) -> ComposeResult:
        join_url = f"http://{self.tailscale_ip or self.lan_ip}:{self.port}"
        with Vertical(id="share-box"):
            yield Static("🎹  Clavus Share — relay running", id="share-title")
            yield Static(f"  {join_url}  ", id="share-url")
            yield Static(
                "Collaborator runs:  clavus join " + join_url,
                id="share-hint",
            )
            if self.tailscale_ip:
                yield Static(
                    f"LAN:  http://{self.lan_ip}:{self.port}",
                    id="share-lan",
                )
            with Horizontal(classes="share-actions"):
                yield Button("Stop Share", id="stop-share", variant="error")
            yield Static("Esc to close (relay keeps running)", id="share-footer")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "stop-share":
            self.action_stop()

    def action_dismiss(self) -> None:
        self.stop_cb()
        self.dismiss()

    def action_stop(self) -> None:
        self.stop_cb()
        self.dismiss()


class JoinModal(ModalScreen[None]):
    """Modal for joining a share session — scans then shows results."""

    CSS = f"""
    JoinModal {{
        align: center middle;
    }}
    #join-box {{
        width: 56;
        max-height: 16;
        background: {C['surface']};
        border: thick {C['accent']};
        padding: 1 2;
    }}
    #join-title {{
        text-style: bold;
        color: {C['yellow']};
    }}
    #join-result {{
        color: {C['fg']};
        padding-top: 1;
    }}
    #join-footer {{
        color: {C['muted']};
        padding-top: 1;
        text-style: italic;
    }}
    """

    BINDINGS = [
        Binding("escape", "dismiss", "Close"),
    ]

    def __init__(self, code: str = "") -> None:
        super().__init__()
        self.code = code

    def compose(self) -> ComposeResult:
        with Vertical(id="join-box"):
            yield Static("scanning LAN + Tailscale...", id="join-title")
            yield Static("", id="join-result")
            yield Static("Esc to close", id="join-footer")

    def on_mount(self) -> None:
        self.run_worker(self._do_scan(), exclusive=True)

    async def _do_scan(self) -> None:
        # Run the scan in a thread
        def _scan(code: str) -> str:
            try:
                from clavus.discovery import scan_for_share_codes
                from clavus.sync import SyncClient
                from clavus.store import BlobStore
                from clavus.sync import Remote, save_remotes, load_remotes, pull_from_remote
                import concurrent.futures
            except ImportError as e:
                return f"join failed: {e}"

            try:
                peers = scan_for_share_codes(timeout=3)
            except Exception as e:
                return f"scan failed: {e}"

            if not peers:
                return "no Clavus share sessions found"

            def _get_info(peer):
                try:
                    client = SyncClient(f"http://{peer.host}:{peer.port}")
                    r = client.client.get(f"http://{peer.host}:{peer.port}/api/share", timeout=10)
                    if r.status_code == 200:
                        return r.json()
                except Exception:
                    pass
                return None

            relay_info = []
            with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
                fut_map = {pool.submit(_get_info, p): p for p in peers}
                for fut in concurrent.futures.as_completed(fut_map, timeout=10):
                    try:
                        info = fut.result()
                        if info and info.get("share_code"):
                            relay_info.append((fut_map[fut], info))
                    except Exception:
                        continue

            if not relay_info:
                return f"found {len(peers)} servers, none in share mode"

            if code:
                code_upper = code.upper()
                relay_info = [(p, i) for p, i in relay_info if i.get("share_code", "").upper() == code_upper]
                if not relay_info:
                    return f"no relay found with code '{code}'"

            if len(relay_info) == 1 or code:
                peer, info = relay_info[0]
                sc = info.get("share_code", "???")
                author = info.get("author", "?")
                host, port = peer.host, peer.port
                name = info.get("hostname", author).lower().replace(" ", "-")
                try:
                    store = BlobStore()
                    remotes = load_remotes(store)
                    remotes = [r for r in remotes if r.name != name]
                    remotes.append(Remote(name=name, url=f"http://{host}:{port}"))
                    save_remotes(store, remotes)
                except Exception as e:
                    return f"failed to save remote: {e}"
                try:
                    projects = store.list_projects()
                    proj_name = info.get("project", {}).get("name", "")
                    matched = next((p for p in projects if p.name == proj_name), None)
                    if matched:
                        result = pull_from_remote(store, matched, Remote(name=name, url=f"http://{host}:{port}"))
                        cues = result.get("cues", 0)
                        snaps = result.get("snapshots", 0)
                        return f"paired with {author}\\n  {sc} — {cues} cues, {snaps} snapshots"
                    else:
                        return f"paired with {author}\\n  {sc} — run :pull to sync"
                except Exception as e:
                    return f"paired with {author}\\n  {sc} — pull failed: {e}"

            lines = [f"  {i.get('share_code','?')} — {i.get('author','?')}" for _, i in relay_info[:3]]
            return "multiple sessions found:\\n" + "\\n".join(lines) + "\\n\\nuse :join <code> to pick"

        loop = asyncio.get_event_loop()
        try:
            result = await loop.run_in_executor(None, _scan, self.code)
        except Exception as e:
            result = f"error: {e}"

        try:
            title = self.query_one("#join-title", Static)
            title.update("Join Result")
            title.styles.color = C['accent']
            self.query_one("#join-result", Static).update(result)
        except NoMatches:
            pass

    def action_dismiss(self) -> None:
        self.dismiss()


# ─── Entry Point ────────────────────────────────────────────────────────────

def run_tui(url: str = "") -> None:
    ClavusApp(url=url).run()


if __name__ == "__main__":
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument("--connect", "-c", default="")
    a = p.parse_args()
    run_tui(a.connect)
