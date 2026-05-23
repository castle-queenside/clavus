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

# ─── Color Palette (CRUX dark — shared across TUI) ──────────────────────────

C = {
    # Core
    "bg": "#0b1418",       # near-black blue
    "surface": "#0f1a20", # card background
    "surface2": "#162a34", # elevated surface
    "border": "#1a3040",   # subtle border
    # Text
    "fg": "#c8d8d8",       # primary text
    "dim": "#6a9a9a",     # secondary/muted text
    "muted": "#3a5a65",    # very subtle
    # Accent — teal spectrum
    "accent": "#1a9e9e",   # primary teal
    "accent2": "#2ac8c8",  # bright teal (highlights)
    "accent_dim": "#0e7070", # dim teal
    # Status
    "green": "#40cc80",    # success / reachable
    "yellow": "#d4a030",   # warning / offline
    "orange": "#d47030",   # error / conflict
    "red": "#ff4444",      # danger
    # Semantic extras
    "purple": "#8878d0",   # branch / special
    "cyan": "#50c8c8",     # info / sync
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
        background: {C['surface']}; border: solid {C['accent']};
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
        # Box-drawing header with double-line border
        title = f"[bold {C['accent']}]╔{'═' * 20} CLAVUS {'═' * 20}╗[/]"
        with VerticalScroll(id="help-box"):
            yield Static(title, classes="help-title")
            yield Static("")
            yield Static("CUES & COLLABORATION", classes="help-section")
            yield Static(f"  [{C['accent']}]c[/{C['accent']}]    New cue        [{C['accent']}]e[/{C['accent']}]    Edit         [{C['accent']}]r[/{C['accent']}]    Reply")
            yield Static(f"  [{C['accent']}]a[/{C['accent']}]    Assign         [{C['accent']}]x[/{C['accent']}]    Archive      [{C['accent']}]i[/{C['accent']}]    Inject markers")
            yield Static(f"  [{C['accent']}]R[/{C['accent']}]    Resolve        [{C['orange']}]![/{C['orange']}]    Conflict      [{C['accent']}]d[/{C['accent']}]    Diff")
            yield Static(f"  [{C['accent']}]T[/{C['accent']}]    Restore snap   [{C['accent']}]o[/{C['accent']}]    Open in Live")
            yield Static("")
            yield Static("SNAPSHOTS & SYNC", classes="help-section")
            yield Static(f"  [{C['accent']}]p[/{C['accent']}]    Pull           [{C['accent']}]P[/{C['accent']}]    Push          [{C['accent']}]S[/{C['accent']}]    Snapshot")
            yield Static("")
            yield Static("NAVIGATION", classes="help-section")
            yield Static(f"  [{C['dim']}]j/↓[/{C['dim']}]  Down           [{C['dim']}]k/↑[/{C['dim']}]  Up           [{C['accent']}]Tab[/{C['accent']}]  Switch pane")
            yield Static(f"  [{C['dim']}]Esc[/{C['dim']}]  Cancel/Dismiss [{C['accent']}]?/h[/{C['accent']}]  Help         [{C['accent']}]s[/{C['accent']}]    Settings")
            yield Static("")
            yield Static("COMMANDS ([dim]:[/])", classes="help-section")
            yield Static(f"  :snapshot <msg>  Create snapshot     :project <name>  Switch project")
            yield Static(f"  :open [path]     Open in Ableton     :pull / :push    Manual sync")
            yield Static(f"  :push!           Force push (bypass lock — use when :push blocks)")
            yield Static(f"  :stem push/pull  Stem file sync      :init <path>     Init project")
            yield Static(f"  :stem import <file>  Import WAV stem  :stem import-folder <dir>  Import all WAVs")
            yield Static(f"  :p2p-host        Start P2P host      :p2p-connect <dns>  P2P sync")
            yield Static(f"  :peers           Show connected peers :find            Discover relays")
            yield Static(f"  :remote rename <name>               :remote add <name> <url>")
            yield Static("")
            yield Static(f"[dim]╚{'═' * 50}╝[/]", classes="help-dim")

    def action_dismiss(self):
        self.app.pop_screen()


# ─── Settings Screen ─────────────────────────────────────────────────

class SettingsScreen(ModalScreen):
    """Settings overlay — uses shared C palette."""

    CSS = f"""
    SettingsScreen {{ background: {C['bg']}; align: center middle; }}
    #settings-box {{
        width: 70; max-height: 95%;
        background: {C['surface']}; border: solid {C['accent']};
        padding: 0 1;
    }}
    #settings-box Static {{ width: 100%; }}
    #settings-box .s-title {{ color: {C['accent']}; text-style: bold; }}
    #settings-box .s-section {{ color: {C['yellow']}; text-style: bold; padding-top: 1; }}
    #settings-box .s-label {{ color: {C['dim']}; }}
    #settings-box Input {{
        background: {C['bg']}; color: {C['fg']};
        border: solid {C['border']}; height: 3; min-width: 40;
    }}
    #settings-box Input:focus {{ border: solid {C['accent']}; }}
    #settings-box Button {{
        background: {C['surface']}; color: {C['fg']};
        border: solid {C['border']}; height: 3; min-width: 12; padding: 0 2;
    }}
    #settings-box Button:hover {{ border: solid {C['accent']}; }}
    #settings-box Button.primary {{ background: {C['accent']}; color: {C['bg']}; text-style: bold; }}
    #settings-box #s-actions {{ height: 5; margin-top: 1; }}
    #settings-box #s-result {{ color: {C['dim']}; height: 1; }}
    """

    BINDINGS = [
        Binding("escape", "cancel", "Cancel"),
        Binding("q", "cancel", "Cancel"),
    ]

    def action_cancel(self):
        self.app.pop_screen()

    def __init__(self):
        super().__init__()
        from clavus.config import ClavusConfig, CONFIG_PATH
        self._cfg = ClavusConfig.load()
        self._cfg_path = CONFIG_PATH

    def compose(self):
        with VerticalScroll(id="settings-box"):
            yield Static("CLAVUS — SETTINGS", classes="s-title")

            # Author
            yield Static("AUTHOR", classes="s-section")
            yield Input(value=self._cfg.author or "", id="s-author", placeholder="your name")

            # Paths
            yield Static("PATHS", classes="s-section")
            yield Static("Projects directory", classes="s-label")
            yield Input(value=self._cfg.projects_dir or "", id="s-projects-dir", placeholder="~/.clavus/projects")
            yield Static("Server host", classes="s-label")
            yield Input(value=self._cfg.host or "", id="s-host", placeholder="localhost")
            yield Static("Server port", classes="s-label")
            yield Input(value=str(self._cfg.port or ""), id="s-port", placeholder="7890")
            yield Static("Server URL", classes="s-label")
            yield Input(value=self._cfg.server_url or "", id="s-url", placeholder="http://localhost:7890")

            # Actions
            yield Static("", id="s-result")
            yield Horizontal(
                Button("Save", id="s-save", variant="primary"),
                Button("Cancel", id="s-cancel"),
                id="s-actions",
            )

    def on_button_pressed(self, event: Button.Pressed):
        btn_id = event.button.id
        if btn_id == "s-save":
            self._save()
        elif btn_id == "s-cancel":
            self.app.pop_screen()

    def _save(self):
        import json
        try:
            data = json.loads(self._cfg_path.read_text())
        except Exception:
            data = {}

        author = self.query_one("#s-author", Input).value.strip()
        projects_dir = self.query_one("#s-projects-dir", Input).value.strip()
        host = self.query_one("#s-host", Input).value.strip()
        port_str = self.query_one("#s-port", Input).value.strip()
        server_url = self.query_one("#s-url", Input).value.strip()

        data["author"] = author or "Chris"
        data["projects_dir"] = projects_dir
        if projects_dir:
            Path(projects_dir).mkdir(parents=True, exist_ok=True)
        data["host"] = host or "0.0.0.0"
        try:
            data["port"] = int(port_str) if port_str else 7890
        except ValueError:
            data["port"] = 7890
        data["server_url"] = server_url

        self._cfg_path.write_text(json.dumps(data, indent=2) + "\n")
        result = self.query_one("#s-result", Static)
        result.update("ok saved")
        self.app.pop_screen()


# ─── App ────────────────────────────────────────────────────────────────────

class ClavusApp(App):
    CSS = f"""
    ClavusApp {{ background: {C['bg']}; }}
    Screen {{ background: {C['bg']}; }}

    #main {{ layout: grid; grid-size: 1 3; grid-rows: auto 1fr auto; height: 100%; }}

    #header-title {{
        color: {C['fg']};
        background: {C['surface']};
        border-bottom: solid {C['accent']};
        padding: 0 1 0 2;
        text-style: bold;
    }}

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
    #footer.input-mode {{ height: 3; }}
    #footer-keys {{ display: none; }}
    #footer-status {{ color: {C['dim']}; width: auto; margin-right: 2; }}
    #footer-hint {{ color: {C['muted']}; text-align: right; min-width: 40; }}
    #footer-stats {{ display: none; }}
    #footer-input {{ display: none; width: 100%; height: 3; background: {C['bg']}; border: solid {C['accent']}; color: {C['fg']}; padding: 0 1; }}
    #footer.input-mode #footer-input {{ display: block; }}
    #footer.input-mode #footer-status {{ display: none; }}
    #footer.input-mode #footer-hint {{ display: none; }}
    /* Worker errors use self.notify() (OS toast) so they are always visible,
       even when the footer is hidden during command input (input-mode). */

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
        Binding("s", "settings", "Settings", priority=True),
        Binding("ctrl+s", "settings", "Settings", priority=True),
    ]

    def __init__(self, url: str = "", debug: bool = False):
        super().__init__()
        from clavus.store import BlobStore
        from clavus.config import ClavusConfig
        self.store = BlobStore()
        self.server_url = url or "local"
        self._debug = debug
        self.project: str = ""
        self.connected: bool = True  # Always connected — working from disk
        self.ws_connected: bool = False
        self.cues: list[Cue] = []
        self.snaps: list[Snap] = []
        self.idx: int = 0
        self._input_mode: str = ""
        self._input_debounce: float = 0.0  # unix ts after last _hide_input
        self._pending_cue_text: str = ""
        self._project_picker_active: bool = False
        self._project_list: list = []  # project objects for picker
        self._remote_picker_active: bool = False
        self._remote_list: list = []  # Remote objects for picker
        self._relay_proc = None
        self._busy: bool = False
        self._last_sync: str = ""     # "⬆ [ok] 12:34" or "⬇ [ok] 12:34" — last completed sync
        self._last_snap_time: float = 0.0  # unix timestamp of last auto-snapshot
        self._sync_status: str = ""    # Live sync progress: "⬆ pushing...", "⬇ pulling..."
        self._sync_progress: str = ""  # Per-category: "c:3/10 a:1/5 s:5/20"
        self._sync_start_time: float = 0.0  # For ETA calculation
        self._spinner_idx: int = 0     # Braille spinner frame
        self._spinner_timer = None     # Timer handle for spinner animation
        self._peer_name: str = ""     # remote name (e.g. "mac")
        self._peer_reachable: bool = False
        self._allow_frozen: bool = True  # :freeze toggle
        self._archived_count: int = 0  # cues with status="archived" (hidden from list)
        _cfg = ClavusConfig.load()
        self.author = _cfg.author
        self._clavus_cfg = _cfg
        # Ensure projects directory exists
        from clavus.helpers import get_projects_dir
        get_projects_dir()
        self._cue_store = None  # Lazy init per project
        self._header_title: Optional[Static] = None
        self._footer_stats: Optional[Static] = None
        self._sticky_error: str = ""  # persistent error shown in footer
        self._sample_counts: tuple[int, int] | None = None  # (total, materialized) from HEAD snapshot

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
        self._connect()  # load project FIRST — welcome depends on project state
        self._update_footer()
        self._update_footer_hint()
        self._update_welcome()  # safety: hide welcome if project auto-loaded
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
        elif self._project_picker_active:
            self._cancel_project_picker()
        elif self._remote_picker_active:
            self._cancel_remote_picker()

    def on_list_view_selected(self, event: ListView.Selected) -> None:
        """Enter key in ListView — switch to project/remote if picker active."""
        if self._project_picker_active:
            event.stop()
            if event.index is not None and event.index < len(self._project_list):
                proj = self._project_list[event.index]
                self._cancel_project_picker()
                self._run_switch_project(proj.name)
        elif self._remote_picker_active:
            event.stop()
            idx = event.index
            if idx == 1:
                # Local-only mode selected (index 1 is always the local-only entry)
                self._cancel_remote_picker()
                self._peer_name = ""
                self._peer_reachable = False
                proj_index = self.store.get_index(self.project)
                if proj_index:
                    proj_index.active_remote = ""
                    self.store.set_index(proj_index)
                self._update_header()
                self._render()
            elif idx is not None and idx - 2 < len(self._remote_list):
                remote = self._remote_list[idx - 2]
                self._cancel_remote_picker()
                self._peer_name = remote.name
                # Save to project so it persists across sessions
                proj_index = self.store.get_index(self.project)
                if proj_index:
                    proj_index.active_remote = remote.name
                    self.store.set_index(proj_index)
                self._probe_reachability()
                self._update_header()
                self._render()

    def _cancel_project_picker(self):
        """Exit project picker, restore normal cues list."""
        self._project_picker_active = False
        self._project_list = []
        try:
            lv = self.query_one("#clv", ListView)
            lv.remove_children()
            lv.index = 0
        except NoMatches:
            pass
        self._cue_fingerprint = None  # force full rebuild
        self._update_footer()
        self._render()  # rebuild cues list

    # ─── Remote picker ─────────────────────────────────────────────────

    @work(exclusive=True)
    async def _run_list_remotes(self):
        from clavus.sync import load_remotes
        remotes = load_remotes(self.store)
        if not remotes:
            self._status("no remotes configured  --  use :join http://... or :remote add")
            return
        self._remote_list = remotes
        self._remote_picker_active = True
        lv = self.query_one("#clv", ListView)
        lv.clear()
        # Show active project context
        pname = f"  [{C['dim']}]project: {self.project}[/]" if self.project else ""
        lv.append(ListItem(Label(pname, classes="project-picker-item")))
        # Index 1 is always "local only" — selected when _peer_name is ""
        is_active = self._peer_name == ""
        active_mark = " ◀" if is_active else ""
        reachable_mark = "●" if is_active else "○"
        lv.append(ListItem(Label(f"  {reachable_mark} local only{active_mark}", classes="project-picker-item")))
        for r in remotes:
            active = " ◀" if r.name == self._peer_name else ""
            reachable = "●" if self._peer_reachable and r.name == self._peer_name else "○"
            line = f"  {reachable} {r.name}  @ {r.url}{active}"
            lv.append(ListItem(Label(line, classes="project-picker-item")))
        self._footer_toast(f"[{C['accent']}]pick a remote -> enter   [{C['dim']}]esc to cancel[/]", 10.0)
        self._update_footer_hint()

    def _cancel_remote_picker(self):
        """Exit remote picker, restore normal cues list."""
        self._remote_picker_active = False
        self._remote_list = []
        try:
            lv = self.query_one("#clv", ListView)
            lv.remove_children()
            lv.index = 0
        except NoMatches:
            pass
        self._cue_fingerprint = None
        self._update_footer()
        self._render()

    async def _run_peers(self):
        """Query the relay for recently connected peers."""
        from clavus.sync import load_remotes
        import urllib.request, json as j
        remotes = load_remotes(self.store)
        relay_url = None
        if self._peer_name:
            match = next((r for r in remotes if r.name == self._peer_name), None)
            if match:
                relay_url = match.url.rstrip("/")
        if not relay_url:
            self._status("no relay connected -- use :remotes to pick a remote first")
            return
        try:
            r = urllib.request.urlopen(f"{relay_url}/api/peers", timeout=5)
            data = j.loads(r.read())
            peers = data.get("peers", [])
            if not peers:
                self._status("no other peers connected to the relay")
                return
            lines = []
            for p in peers:
                ip = p["ip"]
                proj = ", ".join(p.get("projects", [])) or "?"
                age = p.get("age", 0)
                age_str = f"{age}s ago" if age < 60 else f"{age/60:.0f}m ago"
                lines.append(f"  {ip}  ({proj})  {age_str}")
            self._log_event(f"peers ({data['count']}):\n" + "\n".join(lines))
            for p in peers:
                ip = p["ip"]
                if not any(r.url.endswith(ip) or ip in r.url for r in remotes):
                    self._status(f"peer {ip} not in remotes -- use :remote add <name> http://{ip}:7890")
                    return
            self._status(f"all {data['count']} peer(s) already in remotes")
        except Exception as e:
            self._status(f"peers failed: {e}")

    # ─── Command mode ──────────────────────────────────────────────────

    def action_command(self):
        self._show_input("command", ":", prefill="")

    def _do_command(self, text: str):
        # Imports needed by any branch — prevents UnboundLocalError
        import subprocess, sys, asyncio, time
        
        self._debug_log(f"command: {text}")
        
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
            asyncio.create_task(self._run_init_project(arg))
        elif cmd == "inject":
            asyncio.create_task(self._run_inject())
        elif cmd == "restore":
            asyncio.create_task(self._run_restore(arg))
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
        elif cmd == "settings":
            self.action_settings()
        elif cmd == "remote":
            self._run_remote(arg)
        elif cmd == "remotes":
            self._run_list_remotes()
        elif cmd in ("pull", "push") and arg != "all":
            # :pull or :pull <project> — mirrors CLI behavior
            if arg:
                subprocess.run([sys.executable, "-m", "clavus", cmd, arg])
                down = "\u2b07"; up = "\u2b06"
                self._last_sync = f"{down if cmd == 'pull' else up} {time.strftime('%H:%M')}"
                self._connect()  # reload
            else:
                self.action_pull() if cmd == "pull" else self.action_push()
        elif cmd == "push!":
            self.action_force_push()
        elif cmd == "freeze":
            self._toggle_freeze()
        elif cmd == "pull-all" or (cmd == "pull" and arg == "all"):
            self._run_pull_all()
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
            elif arg == "import":
                # No arg yet — prompt for path
                self._show_input("command", ":stem import ", prefill="")
            elif arg == "import-folder":
                self._show_input("command", ":stem import-folder ", prefill="")
            elif arg and arg.startswith("import "):
                path = arg[7:].strip()
                asyncio.create_task(self.action_stem_import(path))
            elif arg and arg.startswith("import-folder "):
                path = arg[14:].strip()
                asyncio.create_task(self.action_stem_import_folder(path))
            else:
                self._status("stem push  |  stem pull  |  stem import <file>  |  stem import-folder <dir>")
        elif cmd == "archive":
            self.action_archive()
        elif cmd in ("delete", "del"):
            self.action_delete_cue()
        elif cmd == "share":
            self._run_share(arg)
        elif cmd == "join":
            if arg and (arg.startswith("http://") or arg.startswith("https://")):
                self._run_join_url(arg)
            else:
                self._status("use :join http://IP:PORT -- get URL from 'clavus share' on host")
        elif cmd == "peers":
            asyncio.create_task(self._run_peers())
        elif cmd in ("help", "h", "?"):
            self.push_screen(HelpScreen())
        elif cmd == "p2p-host":
            self._status("starting P2P host in new terminal...")
            subprocess.Popen([
                "open", "-a", "Terminal",
                str(Path(sys.executable).parent / "python3"),
                "-m", "clavus", "p2p", "--host"
            ])
        elif cmd == "p2p-connect":
            if arg:
                self._status(f"P2P connecting to {arg}...")
                try:
                    _p = subprocess.run(
                        [sys.executable, "-m", "clavus", "p2p", "--connect", arg],
                        capture_output=True, text=True, encoding='utf-8', timeout=60)
                    out = (_p.stdout or "") + (_p.stderr or "")
                    self._log_event(f":p2p-connect {arg} -> {out.strip()[:200]}")
                    # Parse output for a clear summary
                    lines = [l.strip() for l in out.split("\n") if l.strip()]
                    if "CONFLICT" in out:
                        self.notify("! Sync blocked -- heads diverged (both modified since last sync)", timeout=6.0, severity="warning")
                        self._status("! P2P conflict -- sync both via relay first")
                    elif "Failed to connect" in out:
                        self.notify("-- Could not reach peer -- host may be offline", timeout=6.0, severity="error")
                        self._status("-- P2P connect failed")
                    elif "Sync result" in out:
                        import ast
                        sync_line = next((l for l in lines if l.startswith("Sync result")), "")
                        sync_text = sync_line.replace("Sync result: ", "")
                        try:
                            r = ast.literal_eval(sync_text)
                            dl = len(r.get("downloaded", []))
                            ul = len(r.get("uploaded", []))
                            err = r.get("error", "")
                            if err:
                                self.notify(f"! P2P sync error: {err[:60]}", timeout=6.0, severity="warning")
                                self._status(f"! P2P error: {err[:50]}")
                            elif dl or ul:
                                self.notify(f"[ok] P2P synced -- {dl} downloaded, {ul} uploaded", timeout=5.0)
                                self._status(f"[ok] P2P: {dl}dl {ul}ul")
                            else:
                                self.notify("[ok] P2P sync -- up to date (nothing to transfer)", timeout=4.0)
                                self._status("[ok] P2P: up to date")
                        except Exception:
                            self.notify("[ok] P2P sync complete", timeout=4.0)
                            self._status("[ok] P2P done")
                    elif "Connected" in out and "Sync result" not in out:
                        self.notify("[ok] Connected to peer -- no sync needed", timeout=4.0)
                        self._status("[ok] P2P connected")
                    else:
                        self.notify(out.strip()[:100], timeout=5.0)
                        self._status(out.strip()[:80])
                except subprocess.TimeoutExpired:
                    self._log_event(f":p2p-connect {arg} TIMEOUT")
                    self.notify("P2P sync timed out after 60s", timeout=5.0, severity="error")
                    self._status("-- P2P timeout")
                self._connect()
            else:
                self._status("use :p2p-connect <peer-dns>")
        elif cmd == "find":
            self._status("scanning for peers...")
            try:
                _env = {**os.environ, "PYTHONIOENCODING": "utf-8"}
                _p = subprocess.run(
                    [sys.executable, "-m", "clavus", "find", "--tailscale"],
                    capture_output=True, text=True, encoding='utf-8', timeout=15,
                    env=_env)
                out = (_p.stdout or "") + (_p.stderr or "")
                self._log_event(f":find -> {out.strip()[:300]}")
                # Show the actual output to the user
                summary = out.strip().split("\n")[-1][:80] if out.strip() else "no output"
                if "No Clavus servers" in out or "no peers" in out.lower():
                    self.notify("No peers found via Tailscale — try your relay URL directly", timeout=6.0)
                elif out.strip():
                    self.notify(f"Found: {summary}", timeout=5.0)
                self._status(summary)
            except subprocess.TimeoutExpired:
                self._status("find timed out (15s)")
            except Exception as e:
                self._status(f"find failed: {e}")
        elif cmd == "repair":
            self._status("repairing store...")
            try:
                _p = subprocess.run(
                    [sys.executable, "-m", "clavus", "repair"],
                    capture_output=True, text=True, encoding='utf-8', timeout=30)
                out = (_p.stdout or "") + (_p.stderr or "")
                self._log_event(f":repair -> {out.strip()[:300]}")
                self.notify("[ok] Store repair complete", timeout=5.0)
                self._status("repair done")
                self._connect()  # reload
            except Exception as e:
                self._status(f"repair failed: {e}")
        else:
            self._debug_log(f"dispatch: unknown cmd='{cmd}' arg='{arg}'")
            self._status(f"unknown: {cmd}")

    @work(exclusive=True)
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
        # Load active remote from project config
        self._peer_name = getattr(proj, 'active_remote', '') or ''
        self._peer_reachable = False
        if not self._peer_name:
            # No active remote saved for this project — auto-select localhost if available
            from clavus.sync import load_remotes
            remotes = load_remotes(self.store)
            localhost = next((r for r in remotes if r.url.rstrip("/") in ("http://localhost:7890", "http://localhost:7891")), None)
            if localhost:
                self._peer_name = localhost.name
                self._log_event(f"auto-selected '{localhost.name}' -- press P to push")
        if self._peer_name:
            self._probe_reachability()
        self.idx = 0  # reset cursor before loading
        self._load_cues_from_disk()
        self._load_snapshots_from_disk()
        self._ensure_initial_snapshot()  # baseline if project has .als but no snapshots
        self._update_header()
        self._render()
        self._update_footer()
        # Show result as a temporary label at top of cue list
        msg = f"  switched to project [bold]{name}[/]  --  {len(self.cues)} cues, {len(self.snaps)} snapshots"
        try:
            lv = self.query_one("#clv", ListView)
            lv.mount(ListItem(Label(msg, classes="project-list")), before=0)
            self.set_timer(3.0, lambda: self._clear_project_list())
        except NoMatches:
            pass

    @work(exclusive=True)
    async def _run_list_projects(self):
        projects = self.store.list_projects()
        if not projects:
            self._status("no projects found  --  run :init <path> to add one")
            return
        self._project_list = projects
        self._project_picker_active = True
        lv = self.query_one("#clv", ListView)
        lv.clear()
        for p in projects:
            head = p.head or ""
            active = " ◀" if p.name == self.project else ""
            share_icon = "[NET]" if p.shared else "[PK]"
            line = f"  {share_icon} {p.name}  @ {head[:12] if head else '(no snaps)':12s}{active}"
            lv.append(ListItem(Label(line, classes="project-picker-item")))
        # Footer hint
        self._footer_toast(f"[{C['accent']}]pick a project -> enter   [{C['dim']}]esc to cancel[/]", 10.0)
        self._update_footer_hint()

    def _clear_project_list(self):
        # Skip if a picker is active — its clear/rebuild would race with us
        if self._project_picker_active or self._remote_picker_active:
            return
        try:
            lv = self.query_one("#clv", ListView)
            for c in list(lv.children):
                # Direct Label with project-list class, or ListItem containing one
                is_project = hasattr(c, "classes") and "project-list" in c.classes
                if not is_project and hasattr(c, "query_one"):
                    try:
                        c.query_one(".project-list")
                        is_project = True
                    except NoMatches:
                        pass
                if is_project:
                    c.remove()
        except NoMatches:
            pass

    async def _run_init_project(self, path: str):
        """Import a project from a filesystem path — blocking I/O offloaded to thread."""
        import asyncio
        # Clean up pasted paths from Finder/Explorer:
        #   "~/some/path"  -> strip quotes + expand tilde
        #   ~'/some/path'  -> Finder prefill + quotes
        #   /absolute/path -> use as-is
        path = path.strip()
        # Strip any quoting (Finder wraps paths with spaces in single quotes)
        for q in ("'", '"'):
            if path.startswith(q) and path.endswith(q):
                path = path[1:-1]
                break
            # Handle ~'quoted/path' (tilde from prefill + Finder quotes)
            if path.startswith("~" + q) and path.endswith(q):
                path = path[1:-1]  # strip both ~ and quotes
                break
        # Expand tilde
        if path.startswith("~/"):
            path = os.path.expanduser(path)
        elif path == "~":
            path = os.path.expanduser("~")
        self._sync_status = "importing project..."
        self._update_header()
        self._status(f"importing {path}...")
        try:
            from clavus.cli import init_project
            # Offload blocking file I/O to thread so spinner animates
            name, logs = await asyncio.to_thread(init_project, path)
            for line in logs:
                self._log_event(line)
            if name is None:
                self._sync_status = ""
                self._update_header()
                err = logs[-1].replace("-- ", "") if logs else "unknown error"
                self._status(f"init failed: {err}")
                return
            self._sync_status = ""
            self._update_header()
            self._footer_toast(f"[{C['green']}]ok imported: {name}[/] -- loading...", 5.0)
            # Reload from disk — this sets self.project and loads cues/snaps
            self._connect()
            # After connect: if remotes exist and none selected, prompt to pick one
            from clavus.sync import load_remotes
            remotes = load_remotes(self.store)
            if remotes and not self._peer_name:
                self._status(f"{name} ready -- pick a remote to sync")
                # _run_list_remotes is @work — calling it directly starts the background worker
                self._run_list_remotes()
        except Exception as e:
            self._sync_status = ""
            self._update_header()
            self._log_event(f"init error: {e}")

    async def _run_inject(self):
        """Inject unresolved cues as Ableton markers via CLI subprocess."""
        if not self.project:
            self._status("no project selected")
            return
        import asyncio
        self._status("injecting cues into .als...")
        try:
            _inj_env = {**os.environ, "PYTHONIOENCODING": "utf-8"}
            proc = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "clavus", "cue-render", "--inject",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=_inj_env,
            )
            raw_out, raw_err = await asyncio.wait_for(proc.communicate(), timeout=30)
            out = raw_out.decode('utf-8', errors='replace').strip()
            err = raw_err.decode('utf-8', errors='replace').strip()
            if out:
                for line in out.split("\n"):
                    if line.strip():
                        self._log_event(line.strip())
            if proc.returncode != 0 and err:
                self._log_event(f"error: {err}")
            msg = "inject complete" if proc.returncode == 0 else f"inject failed: {err[:60] if err else 'unknown'}"
            self._status(msg)
            # Auto-snapshot so injected markers survive future :open
            if proc.returncode == 0 and out and "no changes" not in out:
                self._log_event("auto-snapshot to save injected markers...")
                asyncio.create_task(self._run_snapshot_for_inject())
        except Exception as e:
            self._status(f"inject error: {e}")

    async def _run_restore(self, hash_str: str = ""):
        """Restore the .als from a snapshot backup via CLI subprocess."""
        if not self.project:
            self._status("no project selected")
            return
        import asyncio
        self._status(f"restoring from {'HEAD' if not hash_str else hash_str}...")
        try:
            _rest_env = {**os.environ, "PYTHONIOENCODING": "utf-8"}
            cmd = [sys.executable, "-m", "clavus", "restore"]
            if hash_str:
                cmd.append(hash_str)
            proc = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=_rest_env,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=30)
            out = stdout.decode('utf-8', errors='replace').strip()
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
            self._status(f"added remote '{name}' -- pulling...")
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
        if not proj:
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
            self._status("raw .als blob missing -- try pulling")
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

        msg = f"restored {self.project}.als <- snapshot {resolved[:10]}"
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
                    text = line.decode('utf-8', errors='replace').strip()
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
                capture_output=True, text=True, encoding='utf-8', timeout=10,
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
                capture_output=True, text=True, encoding='utf-8', timeout=10,
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
          :remote rename Relay       -> renames connected remote to "Relay"
          :remote rename mac Studio  -> renames remote "mac" to "mac Studio"
          :remote rename "mac studio"-> renames connected remote to "mac studio"
        """
        import subprocess, sys
        try:
            cmd = [sys.executable, "-m", "clavus", "remote"]
            if action:
                parts = action.split()
                if parts[0] == "rename" and len(parts) >= 2:
                    if len(parts) == 2:
                        # :remote rename Relay -> rename connected remote
                        if self._peer_name:
                            cmd.extend(["rename", self._peer_name, parts[1]])
                            self._log_event(f"renaming '{self._peer_name}' -> '{parts[1]}'")
                        else:
                            self._status("no connected remote -- use :remote rename <old> <new>")
                            return
                    else:
                        # :remote rename mac Studio -> is "mac" an existing remote?
                        from clavus.sync import load_remotes
                        remotes = load_remotes(self.store)
                        first = parts[1]
                        match = next((r for r in remotes if r.name.lower() == first.lower()), None)
                        new_name = " ".join(parts[2:])
                        if match:
                            cmd.extend(["rename", match.name, new_name])
                            self._log_event(f"renaming '{match.name}' -> '{new_name}'")
                        elif self._peer_name:
                            # First word isn't a remote -> treat all as new name
                            full = " ".join(parts[1:])
                            cmd.extend(["rename", self._peer_name, full])
                            self._log_event(f"renaming '{self._peer_name}' -> '{full}'")
                        else:
                            self._status(f"remote '{first}' not found -- use :remote rename <old> <new>")
                            return
                else:
                    cmd.extend(parts)
            proc = subprocess.run(cmd, capture_output=True, text=True, encoding='utf-8', timeout=10)
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
                # On success, reload active remote so header updates immediately
                if parts[0] in ("rename", "add", "remove") and proc.returncode == 0:
                    proj = self.store.get_index(self.project) if self.project else None
                    self._peer_name = getattr(proj, 'active_remote', '') or ''
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
            proc = subprocess.run(cmd, capture_output=True, text=True, encoding='utf-8', timeout=10)
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
            _snap_env = {**os.environ, "PYTHONIOENCODING": "utf-8"}
            proc = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "clavus", "snapshot",
                "injected markers",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=_snap_env,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=30)
            out = stdout.decode('utf-8', errors='replace').strip()
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
                capture_output=True, text=True, encoding='utf-8', timeout=30,
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
                capture_output=True, text=True, encoding='utf-8', timeout=10,
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
            proc = subprocess.run(cmd, capture_output=True, text=True, encoding='utf-8', timeout=30)
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
            snap_hash, logs = create_snapshot(message, allow_frozen=self._allow_frozen)
            for line in logs:
                self._log_event(line)
            if snap_hash:
                frozen_warned = any("frozen" in line for line in logs)
                if frozen_warned:
                    self._sync_status = "● snap ok (! frozen)"
                    self._log_event(f"● {snap_hash[:10]} -- '{message}' (! frozen tracks)")
                else:
                    self._sync_status = "● snap ok"
                    self._log_event(f"● {snap_hash[:10]} -- '{message}'")
                    self.notify(f"[ok] snapshot saved", timeout=3.0)
                status_text = self._sync_status
                asyncio.create_task(self._delayed_clear_snapshot_status(status_text))
            else:
                # Surface the actual reason
                reason = "no changes or error"
                for line in logs:
                    if "No changes" in line:
                        reason = "no changes — save project first"
                        break
                    elif "frozen" in line and "pass allow_frozen" in line:
                        reason = "frozen tracks — :freeze to allow"
                        break
                    elif "frozen" in line:
                        reason = "frozen tracks (warning only — saved anyway?)"
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
                self._status(f"● {snap_hash[:10]} -- 'auto-pushing to relay...'")
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
            self._log_event(f"renamed snapshot {full[:12]}: {old_msg[:30]} -> {text[:30]}")
        except Exception as e:
            self._status(f"edit failed: {e}")
            self._log_event(f"snapshot edit failed: {e}")

    def action_reply(self):
        if not self._focused_is_cues():
            return
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
        if not self._focused_is_cues():
            return
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
                    f"[bold {C['yellow']}]! Sync Conflict[/]\n"
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
                    f"[bold {C['yellow']}]! Snapshot Message Conflict[/]\n"
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
        asyncio.create_task(self._run_inject())

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
        if target and target.id == "hlv" and self.snaps:
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

    def action_settings(self):
        """Open settings screen."""
        self.push_screen(SettingsScreen())

    def action_assign(self):
        if self._input_mode or time.time() - self._input_debounce < 0.3:
            return  # ignore while input active or within 300ms of dismiss
        if not self._focused_is_cues():
            return
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
        if not self._focused_is_cues():
            return
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
        if not self._focused_is_cues():
            return
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

    def _run_share(self, project: str = ""):
        """Start a relay and show connection URLs.

        If project is given, scope the relay to that project only.
        If no project is given, share all projects (show help text).
        """
        from clavus.config import ClavusConfig
        import subprocess, os

        cfg = ClavusConfig.load()
        lan_ip = self._lan_ip()
        tailscale_ip = self._tailscale_ip()
        port = cfg.port

        # Validate project if specified
        if project:
            proj_data = self.store.get_index(project)
            if not proj_data:
                self._worker_error(f"share: project '{project}' not found — use :projects to list")
                return

        # Build relay args — scope to project if provided
        relay_args = [sys.executable, "-m", "clavus", "relay", "--port", str(port)]
        if project:
            relay_args.append("--project")
            relay_args.append(project)

        # Spawn relay server
        proc = subprocess.Popen(
            relay_args,
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

        self.push_screen(ShareModal(lan_ip, tailscale_ip, port, stop_relay, project))

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
        """Get Tailscale MagicDNS hostname if available, empty string otherwise.

        Prefers MagicDNS (works cross-account), falls back to raw IP.
        """
        try:
            import subprocess, json
            r = subprocess.run(
                ["tailscale", "status", "--json"],
                capture_output=True, text=True, encoding='utf-8', timeout=5,
            )
            if r.returncode == 0:
                dns = json.loads(r.stdout).get("Self", {}).get("DNSName", "")
                if dns:
                    return dns.rstrip(".")
            # Fallback: raw IP
            r2 = subprocess.run(
                ["tailscale", "ip", "-4"],
                capture_output=True, text=True, encoding='utf-8', timeout=5,
            )
            return r2.stdout.strip() if r2.returncode == 0 else ""
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

    def _run_pull_all(self):
        """Pull ALL projects from the active remote.
        
        Runs as async WITHOUT @work to avoid event loop timing issues on Windows.
        """
        asyncio.create_task(self._run_pull_all_async())

    async def _run_pull_all_async(self):
        from clavus.sync import load_remotes, pull_from_remote, pull_snapshot_blobs, SyncClient
        from clavus.store import ClavusProject
        
        self._busy = True
        self._log_event("pull-all: running...")
        msgs: list[str] = []
        try:
            remotes = load_remotes(self.store)
            if not self._peer_name:
                self._worker_error("pull-all: no remote configured — use :remotes to pick one")
                return
            remote = next((r for r in remotes if r.name == self._peer_name), None)
            if not remote:
                self._worker_error(f"pull-all: remote '{self._peer_name}' not found in remotes list")
                return
            
            self._status(f"⬇ probing {remote.name} for projects...")
            client = SyncClient(remote.url)
            r, err = await asyncio.to_thread(
                client.request_with_retry, "GET", "/api/projects", timeout=10)
            if r is None or r.status_code != 200:
                client.close()
                self._worker_error(f"pull-all: cannot reach {remote.name} — {err or (r.status_code if r else 'no response')}")
                return

            projects = r.json().get("projects", [])
            client.close()
            self._log_event(f"pull-all: found {len(projects)} on relay")
            if not projects:
                self._worker_error("pull-all: no projects on relay")
                return
            
            for i, pdata in enumerate(projects):
                pname = pdata["name"]
                self._status(f"⬇ [{i+1}/{len(projects)}] {pname}...")
                self._log_event(f"pull-all: [{i+1}/{len(projects)}] {pname}")
                await asyncio.sleep(0.05)

                # Progress callback for per-project blob downloads
                project_progress = {"done": 0}
                def _on_blob_progress(category: str, done: int, total: int):
                    project_progress["done"] = done
                    self._sync_progress = f"{category}:{done}/{total}"
                    self.call_later(0, self._update_header)

                self._sync_start_time = time.time()
                self._sync_progress = ""
                self._update_header()
                
                proj_data = self.store.get_index(pname)
                if not proj_data:
                    from clavus.helpers import get_projects_dir
                    proj_dir = get_projects_dir() / pname
                    als_dir = proj_dir / f"{pname} Project"
                    default_als = str(als_dir / f"{pname}.als")
                    proj_data = ClavusProject(
                        name=pname, root_als=default_als, head=None,
                        created_at=time.time(),
                        description=f"Pulled from {remote.name}",
                    )
                    self.store.set_index(proj_data)
                
                try:
                    result = await asyncio.to_thread(pull_from_remote, self.store, proj_data, remote)
                    err = result.get("error", "")
                    c = result.get("cues", 0)
                    s = result.get("snapshots", 0)
                    if err:
                        msgs.append(f"{pname}: -- {err[:40]}")
                        self._log_event(f"pull-all: {pname} FAILED -- {err}")
                    elif c or s:
                        blob_count, failed = await asyncio.to_thread(pull_snapshot_blobs, self.store, proj_data, remote, _on_blob_progress)
                        self._sync_progress = ""
                        msgs.append(f"{pname}: {c}c {s}s" + (f" {blob_count}b" if blob_count else "") + (f" !{len(failed)}" if failed else ""))
                        self._log_event(f"pull-all: {pname} OK -- {c}c {s}s")
                    else:
                        msgs.append(f"{pname}: up to date")
                        self._log_event(f"pull-all: {pname} up to date")
                except Exception as e2:
                    msgs.append(f"{pname}: -- {str(e2)[:40]}")
                    self._log_event(f"pull-all: {pname} EXCEPTION -- {e2}")
                
                await asyncio.sleep(0.3)
            
            # Show summary with a long-duration toast (30s) — auto-clears naturally
            self._sticky_error = ""  # clear any previous sticky
            summary = " · ".join(msgs[:5])
            if len(msgs) > 5:
                summary += f" · +{len(msgs)-5} more"
            w = self._footer_stats
            if w is not None:
                w.update(f"[{C['dim']}]pull-all done: {summary}[/]")
                w.refresh()
            # Use real timer so _restore_footer() fires and unblocks _update_footer
            if hasattr(self, '_toast_timer') and self._toast_timer is not None:
                try:
                    self._toast_timer.stop()
                except AttributeError:
                    pass
            self._toast_timer = self.set_timer(30.0, lambda: self._restore_footer())
            self._log_event(f"pull-all: {summary}")
        except Exception as e:
            self._worker_error(f"pull-all error: {e}")
            self._log_event(f"pull-all error: {e}")
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

    def refresh_materialized_count(self):
        """Re-read sample counts from HEAD snapshot and update header.

        Call this after stem pull/materialize completes so the header
        reflects the new materialized state immediately.
        """
        self._load_sample_counts()
        self._update_header()

    @work
    async def action_force_push(self):
        """Force push — skip optimistic lock, overwrite relay state."""
        self._busy = True
        self._status("\u23f3 force pushing...")
        try:
            await self._do_push(force=True)
        finally:
            self._busy = False
            self._update_header()
            self.refresh()

    def _toggle_freeze(self):
        """Toggle frozen-track snapshot behavior: warn vs block."""
        self._allow_frozen = not self._allow_frozen
        if self._allow_frozen:
            self._status("● freeze: warn (snapshots allowed)")
            self._log_event(":freeze -> warn mode -- frozen snapshots allowed with warning")
        else:
            self._status("● freeze: block (unfreeze first)")
            self._log_event(":freeze -> block mode -- frozen snapshots rejected")

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
        out = stdout.decode('utf-8', errors='replace').strip()
        err = stderr.decode('utf-8', errors='replace').strip()
        msg = out.split("\n")[-1][:60] if out else (err.split("\n")[-1][:60] if err else "done")
        self._status(f"stems: {msg}")
        self._log_event(f"stems: {msg}")

    async def action_stem_import(self, path: str):
        """Import a single WAV file as a stem."""
        if not path:
            self._status("usage: :stem import /path/to/file.wav")
            return
        self._status(f"importing {path}...")
        import asyncio
        proc = await asyncio.create_subprocess_shell(
            f"clavus stem import '{path}'",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=60)
        out = stdout.decode('utf-8', errors='replace').strip()
        err = stderr.decode('utf-8', errors='replace').strip()
        msg = out.split("\n")[-1][:80] if out else (err.split("\n")[-1][:80] if err else "done")
        self._status(f"stems: {msg}")
        self._log_event(f"stems: {msg}")
        if out:
            self.notify(out[:120], timeout=5)

    async def action_stem_import_folder(self, path: str):
        """Import all WAV files from a folder as stems."""
        if not path:
            self._status("usage: :stem import-folder /path/to/folder")
            return
        self._status(f"importing stems from {path}...")
        import asyncio
        proc = await asyncio.create_subprocess_shell(
            f"clavus stem import-folder '{path}'",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=120)
        out = stdout.decode('utf-8', errors='replace').strip()
        err = stderr.decode('utf-8', errors='replace').strip()
        msg = out.split("\n")[-1][:80] if out else (err.split("\n")[-1][:80] if err else "done")
        self._status(f"stems: {msg}")
        self._log_event(f"stems: {msg}")
        if out:
            self.notify(out[:120], timeout=5)

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
            if not remotes or not self._peer_name:
                return
            remote = next((r for r in remotes if r.name == self._peer_name), None)
            if not remote:
                return
            import urllib.request
            url = remote.url.rstrip("/") + "/api/ping"
            req = urllib.request.Request(url)
            r = urllib.request.urlopen(req, timeout=2)
            was_reachable = self._peer_reachable
            self._peer_reachable = (r.status == 200)
            if was_reachable != self._peer_reachable:
                self._update_header()
            # Auto-refresh: check if new data arrived from a collaborator push
            if self._peer_reachable and self.project:
                self._auto_refresh_if_changed(remote)
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
                self._log_event(f"auto-refresh: cues {len(self.cues)}->{active_count}, head {str(prev_head or '?')[:8]}->{str(current_head or '?')[:8]}")
                
                # If HEAD moved, pull missing blobs and reconstruct .als on disk
                if head_changed and current_head and remote:
                    try:
                        from clavus.sync import pull_snapshot_blobs
                        _, failed = pull_snapshot_blobs(self.store, proj, remote)
                        if failed:
                            self.notify(f"! {len(failed)} blob(s) failed to download", severity="warning")
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
                    self._status("connected -- press p to pull projects")
                    self._log_event("remotes configured -- press p to pull")
                else:
                    self._status("no project -- use :join http://IP:PORT or :init <path>")
                    self._log_event("no remotes -- use :join http://IP:PORT")
            except Exception:
                self._status("no project -- use :init <path> to add one")
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
        # Load active remote from project config
        self._peer_name = getattr(match, 'active_remote', '') or ''
        self._peer_reachable = False
        # Quick health probe — use urllib (fast, no httpx overhead)
        if self._peer_name:
            try:
                from clavus.sync import load_remotes
                remotes = load_remotes(self.store)
                remote = next((r for r in remotes if r.name == self._peer_name), None)
                if remote:
                    import urllib.request
                    url = remote.url.rstrip("/") + "/api/ping"
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
        self.idx = min(self.idx or 0, len(self.cues) - 1) if self.cues else 0
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
        self._render_history()
        self._load_sample_counts()  # update header sample counts

        # ── Chain canary: detect orphan snapshots ──
        # If there are .meta files whose parent is reachable but they aren't,
        # HEAD may have been clobbered.  Warn the user so they can run repair.
        try:
            import json
            reachable = seen  # set of hashes from the chain walk above
            orphan_count = 0
            for meta_file in self.store.objects_dir.rglob("*.meta"):
                h = meta_file.name.replace(".meta", "")
                if h in reachable:
                    continue
                try:
                    data = json.loads(meta_file.read_text())
                    parent = data.get("parent", "")
                    if parent and parent in reachable:
                        orphan_count += 1
                except Exception:
                    pass
            if orphan_count:
                self._log_event(f"⚠️ {orphan_count} orphan snapshot(s) detected — run :repair to recover")
        except Exception:
            pass

    def _load_sample_counts(self):
        """Load (total, materialized) sample counts from HEAD snapshot.

        total = number of sample hashes referenced by HEAD snapshot
        materialized = how many of those hashes have actual blob files in the store

        Shows in footer as 'N samples' when a project is open and has samples.
        """
        self._sample_counts = None
        if not self.project:
            return
        try:
            proj = self.store.get_index(self.project)
            if not proj or not proj.head:
                return
            snap = self.store.load_snapshot(proj.head)
            if not snap or not snap.sample_hashes:
                return
            total = len(snap.sample_hashes)
            materialized = sum(1 for sh in snap.sample_hashes if self.store.has_object(sh))
            self._sample_counts = (total, materialized)
        except Exception:
            pass

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
                self._log_event(f"● initial snapshot {snap_hash[:10]} -- baseline saved")
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
            try:
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
            except (ValueError, TypeError):
                # Invalid position string — sort to the end
                return (999999, 0, 0, c.timestamp)
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
                # Auto-launch relay if no remotes configured
                relay_port = self._cfg.port if self._cfg.port else 7891
                relay_url = f"http://localhost:{relay_port}"
                relay_live = False
                try:
                    import urllib.request
                    r = urllib.request.urlopen(f"{relay_url}/api/ping", timeout=1)
                    relay_live = (r.status == 200)
                except Exception:
                    pass

                if not relay_live:
                    self._status("starting relay...")
                    self._log_event("auto-spawning clavus share")
                    import platform, subprocess as sp
                    if platform.system() == "Windows":
                        sp.Popen(["py", "-m", "clavus", "share"],
                                 stdout=sp.DEVNULL, stderr=sp.DEVNULL,
                                 creationflags=getattr(sp, "CREATE_NEW_PROCESS_GROUP", 0))
                    else:
                        sp.Popen(["python", "-m", "clavus", "share"],
                                 stdout=sp.DEVNULL, stderr=sp.DEVNULL,
                                 start_new_session=True)
                    for _ in range(30):
                        await asyncio.sleep(0.2)
                        try:
                            r = urllib.request.urlopen(f"{relay_url}/api/ping", timeout=1)
                            if r.status == 200:
                                relay_live = True
                                break
                        except Exception:
                            continue

                if relay_live:
                    from clavus.sync import save_remotes
                    local_remote = next((r for r in remotes if r.url == relay_url), None)
                    if not local_remote:
                        from clavus.sync import Remote
                        local_remote = Remote(name="localhost", url=relay_url, last_head="", last_sync=0)
                        remotes.append(local_remote)
                        save_remotes(self.store, remotes)
                        self._log_event("created localhost remote entry")
                    self._peer_name = local_remote.name
                    self._peer_reachable = True
                else:
                    self._sync_status = ""
                    self._update_header()
                    await asyncio.sleep(0)
                    self._status("-- relay failed to start -- run 'clavus share' manually")
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
                        r, err = await asyncio.to_thread(client.request_with_retry, "GET", "/api/projects", timeout=10)
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
                                self._sync_status = f"⬇ {time.strftime('%H:%M')} {pname}..."
                                self._update_header()
                                await asyncio.sleep(0)
                                r2, _ = await asyncio.to_thread(
                                    client.request_with_retry,
                                    "GET", "/api/sync/pull",
                                    params={"name": pname}, timeout=30)
                                if r2 is None or r2.status_code != 200:
                                    continue
                                info = r2.json().get("project", {})
                                from clavus.helpers import get_projects_dir
                                proj_dir = get_projects_dir() / pname
                                als_dir = proj_dir / f"{pname} Project"
                                default_als = info.get("root_als") or str(als_dir / f"{pname}.als")
                                new_proj = ClavusProject(
                                    name=pname,
                                    root_als=default_als,
                                    created_at=time.time(),
                                )
                                self.store.set_index(new_proj)
                                proj_index = new_proj
                            # Pull data (offload to thread to keep UI responsive)
                            remote_ref = remote
                            result = await asyncio.to_thread(pull_from_remote, self.store, proj_index, remote_ref)
                            blob_count, failed = await asyncio.to_thread(pull_snapshot_blobs, self.store, proj_index, remote_ref)

                            # Materialize audio samples from store -> project folder
                            head_ref = self.store.read_ref("HEAD")
                            if head_ref:
                                snap = self.store.load_snapshot(head_ref)
                                if snap and snap.sample_hashes:
                                    out_path = Path(proj_index.root_als)
                                    base_dir = out_path.parent
                                    base_dir.mkdir(parents=True, exist_ok=True)
                                    samples_dir = base_dir / "Samples"
                                    samples_dir.mkdir(exist_ok=True, parents=True)
                                    total_s = len(snap.sample_hashes)
                                    for i, sh in enumerate(snap.sample_hashes):
                                        fname = self.store.get_sample_filename(sh)
                                        relpath = self.store.get_sample_relpath(sh) or ""
                                        if fname and self.store.has_object(sh):
                                            try:
                                                if relpath and relpath.count("/") > 0:
                                                    out_dir = base_dir / Path(relpath).parent
                                                else:
                                                    out_dir = samples_dir
                                                out_dir.mkdir(parents=True, exist_ok=True)
                                                self.store.materialize_sample(sh, out_dir, fname, relpath)
                                            except Exception:
                                                pass
                                        # Update header every 5 samples or on last
                                        if i % 5 == 0 or i == total_s - 1:
                                            self._sync_status = f"\u2b07 samples {i+1}/{total_s}"
                                            self._update_header()
                                    # Refresh sample counts in header after materialize completes
                                    self.refresh_materialized_count()

                            parts = []
                            if result.get("cues"): parts.append(f"{result['cues']}c")
                            if result.get("snapshots"): parts.append(f"{result['snapshots']}s")
                            if blob_count: parts.append(f"{blob_count}b")
                            if failed: parts.append(f"!{len(failed)}")
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
            # Find the active remote for this project
            if not self._peer_name:
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                self._status("-- no remote selected -- use :remotes to pick one")
                return
            remote = next((r for r in remotes if r.name == self._peer_name), None)
            if not remote:
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                self._status(f"-- remote '{self._peer_name}' not found -- use :remotes")
                return

            # Auto-snapshot local work before overwriting with remote changes.
            # This ensures the user can always go back to what they had.
            #
            # Safety checks (see sync postmortem Bug A + Bug B):
            #  1. Skip if root_als is None/empty (.cfg projects, bundles, etc.)
            #  2. Skip if a snapshot meta already exists for this .als hash —
            #     re-snapshotting would recycle an old hash and destroy the chain.
            #  3. Compare .als hash to HEAD snapshot's als_hash (not head directly).
            try:
                if proj_index.root_als and proj_index.head:
                    als_path = Path(proj_index.root_als)
                    if als_path.exists():
                        raw_als = als_path.read_bytes()
                        current_hash = hashlib.sha256(raw_als).hexdigest()
                        # Guard: skip if a snapshot already exists for this .als state.
                        # Without this, save_snapshot() returns the existing snapshot
                        # (which may have parent=None) and the chain gets destroyed.
                        existing_meta = self.store.objects_dir / current_hash[:2] / f"{current_hash}.meta"
                        if existing_meta.exists():
                            self._log_event(f"● auto-snapshot skipped: snapshot exists for .als {current_hash[:8]}")
                        else:
                            head_snap = self.store.load_snapshot(proj_index.head)
                            head_als_hash = head_snap.als_hash if head_snap else None
                            if current_hash != head_als_hash:
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
                                        self.store.set_project_head(proj_index, snap.hash, source="auto-snapshot-pull")
                                        self._log_event(f"● auto-snapshot {snap.hash[:8]} (local changes saved)")
            except Exception:
                pass  # best-effort — don't block pull on snapshot failure

            self._sync_status = f"⬇ {time.strftime('%H:%M')} {remote.name}..."
            self._update_header()
            await asyncio.sleep(0)

            # Progress callback — called from thread pool, schedules UI update on main thread
            def _on_blob_progress(category: str, done: int, total: int):
                self._sync_progress = f"{category}:{done}/{total}"
                self.call_later(0, self._update_header)

            self._sync_start_time = time.time()
            self._sync_progress = ""
            self._update_header()

            # Always use pull_from_remote() so the relay manages HEAD consistently.
            # The old fast-path for localhost bypassed merging/HEAD-update logic
            # and caused snapshot chains to appear empty after pull.
            blobs = 0
            failed: list[str] = []
            result = await asyncio.to_thread(pull_from_remote, self.store, proj_index, remote)
            if result.get("error"):
                self._peer_reachable = False
                self._last_sync = f"\u2b07 \u2717 {time.strftime('%H:%M')}"
                self._sync_status = ""
                self._sync_progress = ""
                self._update_header()
                await asyncio.sleep(0)
                self._status(f"● {remote.name} unreachable")
                self._log_event(f"● {remote.name} unreachable -- pull failed")
                return
            cues_n = result.get("cues", 0)
            snaps_n = result.get("snapshots", 0)
            conflicts_n = result.get("conflicts", 0)
            blobs, failed = await asyncio.to_thread(pull_snapshot_blobs, self.store, proj_index, remote, _on_blob_progress)

            # Materialize audio samples from store -> project folder
            head_ref = self.store.read_ref("HEAD")
            if head_ref:
                snap = self.store.load_snapshot(head_ref)
                if snap and snap.sample_hashes:
                    out_path = Path(proj_index.root_als)
                    base_dir = out_path.parent
                    base_dir.mkdir(parents=True, exist_ok=True)
                    samples_dir = base_dir / "Samples"
                    samples_dir.mkdir(exist_ok=True, parents=True)
                    total_s = len(snap.sample_hashes)
                    for i, sh in enumerate(snap.sample_hashes):
                        fname = self.store.get_sample_filename(sh)
                        relpath = self.store.get_sample_relpath(sh) or ""
                        if fname and self.store.has_object(sh):
                            try:
                                # Use relpath if it has subdir structure, else write to Samples/
                                if relpath and relpath.count("/") > 0:
                                    out_dir = base_dir / Path(relpath).parent
                                else:
                                    out_dir = samples_dir
                                out_dir.mkdir(parents=True, exist_ok=True)
                                self.store.materialize_sample(sh, out_dir, fname, relpath)
                            except Exception:
                                pass
                        # Update header every 5 samples or on last
                        if i % 5 == 0 or i == total_s - 1:
                            self._sync_status = f"\u2b07 samples {i+1}/{total_s}"
                            self._update_header()
                    # Refresh sample counts in header after materialize completes
                    self.refresh_materialized_count()

            self._sync_status = f"{time.strftime('%H:%M')} {remote.name}  {cues_n}c {snaps_n}s" + (f" {blobs}b" if blobs else "") + (f" !{len(failed)}" if failed else "")
            if conflicts_n:
                self._sync_status += f"  !{conflicts_n}"
            self._sync_progress = ""  # Clear progress on completion
            self._update_header()
            await asyncio.sleep(0)
            self._peer_reachable = True
            self._last_sync = f"\u2b07 {time.strftime('%H:%M')}"
            self._update_header()
            asyncio.create_task(self._delayed_clear_sync())
            await asyncio.sleep(0)
            self._log_event(f"\u2b07 pulled from {remote.name}: {len(self.cues)} cues, {len(self.snaps)} snapshots")
            # Reload the remote to get updated last_head from pull
            remotes = load_remotes(self.store)
            updated = next((r for r in remotes if r.name == remote.name), None)
            if updated:
                self._log_event(f"  last_head now: {updated.last_head[:10] if updated.last_head else 'none'}")
            # Refresh from disk
            if self.project:
                self._load_cues_from_disk()
                self._load_snapshots_from_disk()
                self._update_header()
                await asyncio.sleep(0)
                self._render()
            self.set_timer(0.05, self._update_header)
        except Exception as e:
            self._sync_progress = ""
            self._log_event(f"-- pull error: {e}")
            self._status(f"-- pull error: {e}")

    async def _do_push(self, force: bool = False):
        """Push cues + snapshots + blobs to the project's active remote."""
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
            # Auto-launch relay if no remotes configured
            if not remotes:
                relay_port = self._cfg.port if self._cfg.port else 7891
                relay_url = f"http://localhost:{relay_port}"
                relay_live = False
                try:
                    import urllib.request
                    r = urllib.request.urlopen(f"{relay_url}/api/ping", timeout=1)
                    relay_live = (r.status == 200)
                except Exception:
                    pass

                if not relay_live:
                    self._status("starting relay...")
                    self._log_event("auto-spawning clavus share")
                    import platform, subprocess as sp
                    if platform.system() == "Windows":
                        sp.Popen(["py", "-m", "clavus", "share"],
                                 stdout=sp.DEVNULL, stderr=sp.DEVNULL,
                                 creationflags=getattr(sp, "CREATE_NEW_PROCESS_GROUP", 0))
                    else:
                        sp.Popen(["python", "-m", "clavus", "share"],
                                 stdout=sp.DEVNULL, stderr=sp.DEVNULL,
                                 start_new_session=True)
                    for _ in range(30):
                        await asyncio.sleep(0.2)
                        try:
                            r = urllib.request.urlopen(f"{relay_url}/api/ping", timeout=1)
                            if r.status == 200:
                                relay_live = True
                                break
                        except Exception:
                            continue

                if relay_live:
                    from clavus.sync import save_remotes, Remote
                    local_remote = next((r for r in remotes if r.url == relay_url), None)
                    if not local_remote:
                        local_remote = Remote(name="localhost", url=relay_url, last_head="", last_sync=0)
                        remotes.append(local_remote)
                        save_remotes(self.store, remotes)
                        self._log_event("created localhost remote entry")
                    self._peer_name = local_remote.name
                    self._peer_reachable = True
                # If relay failed to start, fall through to solo mode below

            if not self._peer_name:
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                als_path = Path(proj_index.root_als)
                if als_path.exists():
                    try:
                        raw_als = als_path.read_bytes()
                        current_hash = hashlib.sha256(raw_als).hexdigest()
                        if current_hash != (proj_index.head or ""):
                            from clavus import parse_als
                            project = parse_als(als_path)
                            if project:
                                snap = self.store.save_snapshot(
                                    project,
                                    message="local snapshot",
                                    parent=proj_index.head,
                                )
                                if snap.hash != proj_index.head:
                                    self.store.update_ref("HEAD", snap.hash)
                                    self.store.set_project_head(proj_index, snap.hash, source="solo-snapshot")
                                    self._log_event(f"● local snapshot {snap.hash[:8]}")
                                    self._last_sync = f"● {time.strftime('%H:%M')}"
                                    self._status(f"samples snapshot saved locally -- use :remotes to pick a remote before pushing")
                                else:
                                    self._status(f"ok up to date -- use :remotes to pick a remote before pushing")
                            else:
                                self._status(f"ok up to date -- use :remotes to pick a remote before pushing")
                        else:
                            self._last_sync = f"● {time.strftime('%H:%M')}"
                            self._status(f"ok up to date -- use :remotes to pick a remote before pushing")
                    except Exception as e:
                        self._status(f"ok working locally (snapshot failed: {e}) -- use :remotes to push")
                else:
                    self._status(f"ok no .als found -- use :remotes to pick a remote before pushing")
                return
            remote = next((r for r in remotes if r.name == self._peer_name), None)
            if not remote:
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                self._status(f"-- remote '{self._peer_name}' not found -- use :remotes")
                return
            # Allow localhost (solo host mode) to work without relay
            is_localhost = remote.url.startswith("http://localhost")
            if not self._peer_reachable and not is_localhost:
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                self._status("! relay unreachable -- is 'clavus share' running?")
                self._log_event("push blocked: relay not reachable -- run 'clavus share' first")
                return
            # Auto-snapshot local changes before pushing (conflict resolution, cue edits, etc.)
            # This ensures HEAD matches what we're about to send.
            #
            # Safety checks (see sync postmortem Bug A + Bug B):
            #  1. Skip if root_als is None/empty (.cfg projects, bundles, etc.)
            #  2. Skip if a snapshot meta already exists for this .als hash —
            #     re-snapshotting would recycle an old hash and destroy the chain.
            #  3. Compare .als hash to HEAD snapshot's als_hash (not head directly).
            try:
                if proj_index.root_als:
                    als_path = Path(proj_index.root_als)
                    if als_path.exists():
                        raw_als = als_path.read_bytes()
                        current_hash = hashlib.sha256(raw_als).hexdigest()
                        # Guard: skip if a snapshot already exists for this .als state.
                        existing_meta = self.store.objects_dir / current_hash[:2] / f"{current_hash}.meta"
                        if existing_meta.exists():
                            self._log_event(f"● auto-snapshot skipped: snapshot exists for .als {current_hash[:8]}")
                        else:
                            head_snap = self.store.load_snapshot(proj_index.head) if proj_index.head else None
                            head_als_hash = head_snap.als_hash if head_snap else None
                            if current_hash != head_als_hash:
                                from clavus import parse_als
                                project = parse_als(als_path)
                                if project:
                                    snap = self.store.save_snapshot(
                                        project,
                                        message="auto-snapshot before push",
                                        parent=proj_index.head,
                                    )
                                    if snap.hash != proj_index.head:
                                        self.store.update_ref("HEAD", snap.hash)
                                        self.store.set_project_head(proj_index, snap.hash, source="auto-snapshot-push")
                                        self._log_event(f"● auto-snapshot {snap.hash[:8]} (local changes saved)")
            except Exception:
                pass  # best-effort — don't block push on snapshot failure

            # Solo host mode: for localhost remote, check relay live right before push
            relay_live = False
            if is_localhost:
                try:
                    import urllib.request
                    r = urllib.request.urlopen(f"{remote.url.rstrip('/')}/api/ping", timeout=2)
                    relay_live = (r.status == 200)
                except Exception:
                    pass
                if not relay_live:
                    # Relay not running — save locally only
                    self._sync_status = f"-- {time.strftime('%H:%M')} local"
                    self._update_header()
                    await asyncio.sleep(0)
                    self.notify("-- saved locally -- relay offline", timeout=5.0, severity="warning")
                    self._status("-- saved locally -- relay offline")
                    self._log_event("solo push: saved locally, relay not running")
                    return

            self._sync_status = f"⬆ {time.strftime('%H:%M')} {remote.name}..."
            self._update_header()
            await asyncio.sleep(0)
            self._status(f"⬆ {'force-' if force else ''}pushing to {remote.name}...")

            # Progress callback for blob ops
            def _on_blob_progress(category: str, done: int, total: int):
                self._sync_progress = f"{category}:{done}/{total}"
                self.call_later(0, self._update_header)

            self._sync_start_time = time.time()
            self._sync_progress = ""
            self._update_header()

            result = await asyncio.to_thread(push_to_remote, self.store, proj_index, remote, force=force)
            if result.get("error"):
                self._peer_reachable = False
                self._last_sync = f"\u2b06 \u2717 {time.strftime('%H:%M')}"
                self._sync_status = ""
                self._update_header()
                await asyncio.sleep(0)
                err = result['error']
                if 'pull first' in err.lower() or 'conflict' in err.lower():
                    self._status(f"! conflict -- press p to pull, then P to push")
                    self._log_event(f"! push conflict: local_head={proj_index.head[:10] if proj_index.head else 'none'} remote_last_head={remote.last_head[:10] if remote.last_head else 'none'} -- {err}")
                else:
                    self._status(f"-- push failed: {err[:60]}")
                    self._log_event(f"push error: {err}")
                return
            cues_n = result.get("cues", 0)
            snaps_n = result.get("snapshots", 0)
            blobs = await asyncio.to_thread(push_snapshot_blobs, self.store, proj_index, remote, _on_blob_progress)
            self._sync_status = f"⬆ {time.strftime('%H:%M')} {remote.name}  {cues_n}c {snaps_n}s" + (f" {blobs}b" if blobs else "")
            self._sync_progress = ""  # Clear progress on completion
            self._update_header()
            await asyncio.sleep(0)
            self._peer_reachable = True
            self._last_sync = f"\u2b06 {time.strftime('%H:%M')}"
            self._update_header()
            asyncio.create_task(self._delayed_clear_sync())
            await asyncio.sleep(0)
            self._status(f"⬆ pushed: {len(self.cues)} cues, {len(self.snaps)} snaps")
            self._log_event(f"⬆ pushed to {remote.name}: {len(self.cues)} cues, {len(self.snaps)} snapshots")
            self.set_timer(0.05, self._update_header)
        except Exception as e:
            self._sync_status = ""
            self._sync_progress = ""
            self._update_header()
            await asyncio.sleep(0)
            self._status(f"-- push error: {e}")

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
            try:
                target.action_cursor_down()
            except IndexError:
                pass  # Textual race: list mutated during rapid navigation

    def action_cursor_up(self):
        target = self._focused_list_view()
        if target:
            try:
                target.action_cursor_up()
            except IndexError:
                pass  # Textual race: list mutated during rapid navigation

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

    def _focused_is_cues(self) -> bool:
        """True if the cues list pane is currently focused."""
        try:
            return self.query_one("#clv", ListView).has_focus
        except NoMatches:
            return False

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
        except NoMatches:
            pass

    def _update_footer_hint(self):
        """Simple hint in footer corner."""
        try:
            self.query_one("#footer-hint", Static).update(f"[{C['muted']}]? help[/]")
        except NoMatches:
            pass

    def on_list_view_highlighted(self, event: ListView.Highlighted):
        if event.list_view.id == "clv":
            self.idx = event.list_view.index
            self._update_footer()

    # ─── Status / Header / Footer ───────────────────────────────────────

    def _footer_toast(self, msg: str, duration: float = 4.0):
        """Write to footer, auto-restore project info after duration seconds."""
        self._sticky_error = ""  # clear any sticky error
        w = self._footer_stats
        if w is not None:
            w.update(msg)
            w.refresh()
        # Cancel any pending restore, then schedule new one
        if hasattr(self, "_toast_timer") and self._toast_timer is not None:
            try:
                self._toast_timer.stop()
            except AttributeError:
                pass  # sentinel object, not a real timer
        self._toast_timer = self.set_timer(duration, lambda: self._restore_footer())
        self._toast_set_at = time.time()  # safety net timestamp

    def _restore_footer(self):
        """Clear toast timer and restore footer to default state."""
        self._toast_timer = None
        self._update_footer()

    def _status(self, msg: str):
        """Short footer toast — auto-clears after 3s."""
        safe = msg.replace("[", "\\\\[").replace("]", "\\\\]")
        self._footer_toast(f"[{C['dim']}]{safe}[/]", 3.0)

    def _show_sticky(self, msg: str):
        """Show a sticky error using Textual's notify system.
        
        Bypasses the footer entirely — no CSS timing issues with input-mode.
        """
        self._sticky_error = msg  # keep for _update_footer compatibility
        # File logging for debugging
        import os
        try:
            log_dir = os.path.expanduser("~/.clavus")
            os.makedirs(log_dir, exist_ok=True)
            log_path = os.path.join(log_dir, "tui.log")
            with open(log_path, "a") as lf:
                lf.write(f"[{time.strftime('%H:%M:%S')}] _show_sticky: {msg}\n")
        except Exception:
            pass
        self.notify(msg, timeout=10.0, severity="warning")

    def _log_event(self, event: str):
        """Timestamped footer toast — auto-clears after 8s."""
        ts = time.strftime("%H:%M:%S")
        safe = event.replace("[", "\\\\[").replace("]", "\\\\]")
        self._footer_toast(f"[{C['dim']}]{ts}[/] [{C['accent']}⟩[/] {safe}", 8.0)

    def _worker_error(self, msg: str):
        """Log an error from a @work worker to disk AND show a native notification.

        Uses self.notify() — Textual's built-in notification system — which works
        reliably across all platforms and bypasses all footer CSS timing issues.
        Workers can't use set_timer reliably (known Textual bug), and the footer
        may be hidden (display:none) during input-mode on Windows.
        """
        from pathlib import Path
        log_path = Path.home() / ".clavus" / "errors.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        ts = time.strftime("%Y-%m-%d %H:%M:%S")
        with open(log_path, "a") as f:
            f.write(f"[{ts}] {msg}\n")
        # Show native OS notification — works on all platforms, no CSS dependency
        self.notify(msg, timeout=12.0, severity="error")

    def _debug_log(self, msg: str):
        """Write diagnostic message to debug log when --debug is active."""
        if not self._debug:
            return
        from pathlib import Path
        log_path = Path.home() / ".clavus" / "debug.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        ts = time.strftime("%Y-%m-%d %H:%M:%S")
        with open(log_path, "a") as f:
            f.write(f"[{ts}] {msg}\n")

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
        """Header: ⬡ hex logo, project, connection dot + remote, sync activity."""
        try:
            # Box-drawing left border + logo
            logo = f"[bold {C['accent2']}]┌─⬡[/] "
            # Project name — "clavus · projectname" when open
            proj = f"[bold white]clavus[/]" if not self.project else f"[bold white]clavus  ·  {self.project}[/]"
            # Separator
            sep = f"[{C['muted']}]│[/]" if proj else ""
            # Connection dot — green = reachable, yellow = offline, local = dim
            if self._peer_name and self._peer_reachable:
                peer = f"  [bold {C['green']}]●[/]"
            elif self._peer_name:
                peer = f"  [{C['yellow']}]○[/]"
            elif self.project:
                peer = f"  [{C['muted']}][local][/]"
            else:
                peer = ""
            # Sync activity — spinner during, timestamp after
            sync = ""
            if self._sync_status:
                s = self.BRAILLE[self._spinner_idx % len(self.BRAILLE)]
                if self._sync_progress:
                    elapsed = time.time() - self._sync_start_time
                    progress_parts = [p for p in self._sync_progress.split(" ") if p]
                    progress_str = " ".join(progress_parts)
                    sync = f"  [{C['yellow']}]{s} {self._sync_status} {progress_str}[/]"
                else:
                    sync = f"  [{C['yellow']}]{s} {self._sync_status}[/]"
            elif self._last_sync:
                sync = f"  [{C['green']}]{self._last_sync}[/]"
            widget = self.query_one("#header-title", Static)
            widget.update(f"{logo}{proj}{sep}{peer}{sync}")
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
        # Don't clobber active toasts — _restore_footer will handle it when ready
        if hasattr(self, '_toast_timer') and self._toast_timer is not None:
            # Sticky error STILL takes priority even with active toast
            if self._sticky_error:
                try:
                    status = self.query_one("#footer-status", Static)
                    status.update(f"[{C['dim']}]{self._sticky_error}[/]")
                except NoMatches:
                    pass
                return
            return
        try:
            status = self.query_one("#footer-status", Static)
            if not self.project:
                status.update(f"[{C['dim']}]welcome — :init <path> to open a project[/]")
                self._update_welcome()
                return

            # Show sync progress bar when active
            if self._sync_status and self._sync_progress:
                bar = self._progress_bar(self._sync_progress)
                parts = [self._sync_status, bar]
            elif self._sync_status:
                s = self.BRAILLE[self._spinner_idx % len(self.BRAILLE)]
                parts = [f"{s} {self._sync_status}"]
            else:
                parts = [f"[bold]{self.project}[/]"]
            self._update_welcome()  # hide welcome, show cues + history

            # Cues — always show, even 0
            n = len(self.cues)
            parts.append(f"{n} cue{'s' if n != 1 else ''}")

            # Sample materialization count
            if self._sample_counts is not None:
                total, mat = self._sample_counts
                if total > 0:
                    parts.append(f"[b]samples[/b] {mat}/{total}")

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
            # ASCII art banner — CLAVUS in doom font
            banner = [
                "  _____  _       ___  _   _ _   _ _____ ",
                " /  __ \\| |     / _ \\| | | | | | /  ___|",
                " | /  \\/| |    / /_\\ \\ | | | | | \\ `--. ",
                " | |    | |    |  _  | | | | | | |`--. \\",
                " | \\__/\\| |____| | | \\ \\_/ / |_| /\\__/ /",
                "  \\____/\\_____/\\_| |_/\\___/ \\___/\\____/ ",
            ]
            art = "\n".join(f"    [{C['accent2']}]{line}[/]" for line in banner)
            welcome.update(
                f"\n"
                f"{art}\n\n"
                f"    [{C['dim']}]Ableton Live cue management[/]\n"
                f"    [{C['dim']}]snap and sync your ideas across machines[/]\n\n"
                f"    [{C['accent']}]:init <path>[/]  [{C['dim']}]open a project[/]\n"
                f"    [{C['accent']}]:join <URL>[/]  [{C['dim']}]connect to a relay[/]\n\n"
                f"    [{C['dim']}]S[/] snapshot   [{C['dim']}]p[/] pull   [{C['dim']}]?[/] help\n"
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

    def _progress_bar(self, progress_str: str) -> str:
        """Convert 'content:3/10 sample:7/7' into a visual block bar.

        Returns a Rich-markup string like: '████████░░ 75%'.
        """
        parts = progress_str.split()
        total_done = 0
        total_all = 0
        for p in parts:
            if ":" in p:
                cat, frac = p.split(":", 1)
                if "/" in frac:
                    done_str, all_str = frac.split("/", 1)
                    try:
                        total_done += int(done_str)
                        total_all += int(all_str)
                    except ValueError:
                        pass
        if total_all == 0:
            return ""
        pct = total_done / total_all
        bar_width = 12
        filled = int(bar_width * pct)
        bar = "█" * filled + "░" * (bar_width - filled)
        return f"[{C['accent']}]{bar}[/] [{C['dim']}]{int(pct*100)}%[/]"

    def _render(self):
        if self._project_picker_active or self._remote_picker_active:
            return  # picker owns the ListView
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
            lv.append(ListItem(Label(
                f"\n    [{C['dim']}]no cues yet[/]\n"
                f"    [{C['accent']}]c[/] [{C['dim']}]to place one at the playhead[/]\n"
            )))
            return

        for i, c in enumerate(self.cues):
            color = C["yellow"] if c.status == "pending" else (
                C["green"] if c.status == "resolved" else C["muted"])
            dot = "[ok]" if c.status == "resolved" else "●"
            rc = f" [{C['dim']}]{len(c.replies)}r[/]" if c.replies else ""
            assignee_part = f" 👤 {c.assignee}" if c.assignee else ""
            in_prog = f" [{C['yellow']}]▶[/]" if c.in_progress else ""
            safe_text = c.text[:55].replace("[", "\\[").replace("]", "\\]")
            conflict_mark = f" [{C['yellow']}]![/]" if getattr(c, 'conflict', False) else ""
            ago = self._time_ago(c.timestamp)
            track_hint = f" [{C['muted']}]{c.track_name}[/]" if c.track_name else ""

            # Main cue line: dot + position + text + indicators
            header = (
                f"  [{color}]{dot}[/] [{color}]@{c.position}[/] "
                f"[{C['fg']}]{safe_text}[/]{track_hint}{rc}{conflict_mark}"
                + (f" [{C['dim']}]{ago}[/]" if ago else "")
            )
            lines = [header]

            if c.replies:
                for j, r in enumerate(c.replies):
                    tag = r.author or "anon"
                    r_ago = self._time_ago(r.timestamp)
                    conn = "╰─" if j == len(c.replies) - 1 else "├─"
                    safe_reply = r.text[:50].replace("[", "\\[").replace("]", "\\]")
                    lines.append(
                        f"  [{C['dim']}]{conn} {tag}[/] [{C['muted']}]{r_ago}[/]"
                        f"  [{C['dim']}]{safe_reply}[/]"
                    )

            # Meta line: assignee + in-progress
            if assignee_part or in_prog:
                lines.append(f"  [{C['dim']}]└─{assignee_part}{in_prog}[/]")

            lv.append(ListItem(Label("\n".join(lines))))

        if self.idx < len(lv.children):
            lv.index = self.idx

    def _render_history(self):
        lv = self.query_one("#hlv", ListView)
        lv.remove_children()
        if not self.snaps:
            lv.append(ListItem(Label(
                f"\n    [{C['dim']}]no snapshots yet[/]\n"
                f"    [{C['accent']}]S[/] [{C['dim']}]to capture the current state[/]\n"
            )))
            return
        for i, s in enumerate(self.snaps[:10]):
            ago = self._time_ago(s.timestamp)
            safe_msg = s.message[:45].replace("[", "\\[").replace("]", "\\]")
            conflict_mark = f" [{C['yellow']}]![/]" if s.conflict_message else ""

            # HEAD indicator: ▶ in bold teal, older snapshots: · in dim
            if i == 0:
                dot = f"[bold][{C['accent']}]▶[/][/]"
            else:
                dot = f"[{C['dim']}]·[/]"

            # Auto-snapshots get dimmed message
            is_auto = "auto-snapshot" in s.message.lower() if s.message else False
            msg_color = C['dim'] if is_auto else C['fg']

            lv.append(ListItem(Label(
                f"  {dot} [{msg_color}]{safe_msg}[/]{conflict_mark}  "
                f"[{C['dim']}]{s.hash[:8]}  {ago}[/]"
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
        border: solid {C['accent']};
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

    def __init__(self, lan_ip: str, tailscale_ip: str, port: int, stop_cb, project: str = "") -> None:
        super().__init__()
        self.lan_ip = lan_ip
        self.tailscale_ip = tailscale_ip
        self.port = port
        self.stop_cb = stop_cb
        self.project = project

    def compose(self) -> ComposeResult:
        join_url = f"http://{self.tailscale_ip or self.lan_ip}:{self.port}"
        scope_note = f" [PK] scoped to: {self.project}" if self.project else " (all projects)"
        with Vertical(id="share-box"):
            yield Static(f"[kbd]  Clavus Share — relay running{scope_note}", id="share-title")
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
        border: solid {C['accent']};
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

def run_tui(url: str = "", debug: bool = False) -> None:
    ClavusApp(url=url, debug=debug).run()


if __name__ == "__main__":
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument("--connect", "-c", default="")
    p.add_argument("--debug", "-d", action="store_true")
    a = p.parse_args()
    run_tui(a.connect, debug=a.debug)
