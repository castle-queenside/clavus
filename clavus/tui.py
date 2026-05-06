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
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass, field
from typing import Optional

import httpx

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Container, Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Static, Input, ListView, ListItem, Label, Button
from textual.message import Message
from textual.css.query import NoMatches
from textual.worker import WorkerState

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
    replies: list = field(default_factory=list)

@dataclass
class Snap:
    hash: str = ""
    message: str = ""
    timestamp: float = 0.0
    track_count: int = 0

    @classmethod
    def from_dict(cls, d: dict) -> "Snap":
        return cls(
            hash=d.get("hash", d.get("full_hash", ""))[:10],
            message=d.get("message", ""),
            timestamp=d.get("timestamp", 0.0),
            track_count=d.get("track_count", 0),
        )

# ─── ClavusClient ───────────────────────────────────────────────────────────

class ClavusClient:
    def __init__(self, base_url: str = "http://localhost:7890"):
        self.base_url = base_url.rstrip("/")
        self.client = httpx.AsyncClient(timeout=10.0)

    async def ping(self) -> bool:
        try:
            r = await self.client.get(f"{self.base_url}/api/ping")
            return r.status_code == 200
        except Exception:
            return False

    async def get_project_info(self, name: str = "") -> Optional[dict]:
        try:
            params = {"name": name} if name else {}
            r = await self.client.get(f"{self.base_url}/api/project", params=params)
            if r.status_code == 200:
                return r.json()
        except Exception:
            pass
        return None

    async def get_project(self) -> Optional[dict]:
        try:
            r = await self.client.get(f"{self.base_url}/api/project")
            if r.status_code == 200:
                return r.json()
        except Exception:
            pass
        return None

    async def list_projects(self) -> list[dict]:
        try:
            r = await self.client.get(f"{self.base_url}/api/projects")
            if r.status_code == 200:
                return r.json().get("projects", [])
        except Exception:
            pass
        return []

    async def init_project(self, path: str) -> Optional[dict]:
        """Register a project from a path. Returns project info or error dict."""
        try:
            r = await self.client.post(
                f"{self.base_url}/api/projects/init",
                params={"path": path},
                timeout=15,
            )
            return r.json()
        except Exception:
            return None

    async def browse_dir(self, directory: str = "") -> Optional[dict]:
        """Browse a directory for .als files. Empty string = home dir."""
        try:
            params = {"dir": directory} if directory else {}
            r = await self.client.get(
                f"{self.base_url}/api/projects/browse",
                params=params,
                timeout=10,
            )
            if r.status_code == 200:
                return r.json()
        except Exception:
            pass
        return None

    async def inject_cues(self, project: str) -> Optional[dict]:
        """Inject unresolved cues as Ableton markers into the .als file."""
        try:
            r = await self.client.post(
                f"{self.base_url}/api/projects/inject",
                params={"name": project},
                timeout=15,
            )
            if r.status_code == 200:
                return r.json()
        except Exception:
            pass
        return None

    async def restore_snapshot(self, project: str, hash: str = "") -> Optional[dict]:
        """Restore a project's .als from a snapshot backup."""
        try:
            params = {"name": project}
            if hash:
                params["hash"] = hash
            r = await self.client.post(
                f"{self.base_url}/api/projects/restore",
                params=params,
                timeout=15,
            )
            if r.status_code == 200:
                return r.json()
            # Return error dict for non-200
            try:
                return r.json()
            except Exception:
                return {"error": f"HTTP {r.status_code}"}
        except Exception as e:
            return {"error": str(e)}

    async def create_snapshot(self, project: str, message: str, tags: str = "") -> Optional[dict]:
        """Create a new snapshot of the current project state."""
        try:
            params = {"name": project}
            r = await self.client.post(
                f"{self.base_url}/api/projects/snapshot",
                params=params,
                json={"message": message, "tags": tags},
                timeout=15,
            )
            if r.status_code == 200:
                return r.json()
            try:
                return r.json()
            except Exception:
                return {"error": f"HTTP {r.status_code}"}
        except Exception as e:
            return {"error": str(e)}

    async def get_visual_diff(self, project: str, hash: str = "") -> Optional[str]:
        """Get visual diff HTML for a snapshot vs its parent."""
        try:
            params = {"name": project}
            if hash:
                params["before"] = hash
            r = await self.client.get(
                f"{self.base_url}/api/projects/compare",
                params=params, timeout=15,
            )
            if r.status_code == 200:
                return r.text
        except Exception:
            pass
        return None

    async def pull(self, project: str) -> tuple[list[Cue], list[Snap]]:
        try:
            r = await self.client.get(
                f"{self.base_url}/api/sync/pull",
                params={"name": project}, timeout=15,
            )
            if r.status_code == 200:
                data = r.json()
                cues = []
                fields = {"id","position","text","author","status",
                          "timestamp","track_name","snapshot_hash",
                          "assignee","in_progress"}
                for c in data.get("cues", []):
                    cue = Cue(**{k: v for k, v in c.items() if k in fields})
                    cue.replies = [
                        Reply(id=rr.get("id",""), author=rr.get("author",""),
                              text=rr.get("text",""), timestamp=rr.get("timestamp",0))
                        for rr in c.get("replies", [])
                    ]
                    cues.append(cue)
                snaps = [Snap.from_dict(s) for s in data.get("snapshots", [])]
                return cues, snaps
        except Exception:
            pass
        return [], []

    async def push(self, project: str, cues: list[Cue]) -> bool:
        try:
            def to_dict(r):
                if isinstance(r, dict):
                    return r
                return {"id": r.id, "author": r.author, "text": r.text,
                        "timestamp": r.timestamp}
            payload = [{"id": c.id, "position": c.position, "text": c.text,
                        "author": c.author, "status": c.status,
                        "timestamp": c.timestamp,
                        "assignee": c.assignee, "in_progress": c.in_progress,
                        "track_name": c.track_name, "snapshot_hash": c.snapshot_hash,
                        "replies": [to_dict(r) for r in c.replies]}
                       for c in cues]
            r = await self.client.post(
                f"{self.base_url}/api/sync/push",
                params={"name": project}, json={"cues": payload}, timeout=15,
            )
            return r.status_code == 200
        except Exception:
            return False

    async def close(self):
        await self.client.aclose()
        if self._ws:
            try:
                await self._ws.close()
            except Exception:
                pass
            self._ws = None

    async def archive_cue(self, project: str, cue_id: str) -> bool:
        """Archive a specific cue via the server API."""
        try:
            r = await self.client.post(
                f"{self.base_url}/api/cues/{cue_id}/archive",
                params={"name": project}, timeout=10,
            )
            return r.status_code == 200
        except Exception:
            return False

    async def delete_cue(self, project: str, cue_id: str) -> bool:
        """Delete a cue permanently via the server API."""
        try:
            r = await self.client.delete(
                f"{self.base_url}/api/cues/{cue_id}",
                params={"project": project}, timeout=10,
            )
            return r.status_code == 200
        except Exception:
            return False

    async def archive_resolved(self, project: str) -> int:
        """Archive all resolved/skipped cues. Returns count."""
        try:
            r = await self.client.get(
                f"{self.base_url}/api/cues/archived",
                params={"name": project}, timeout=10,
            )
            if r.status_code == 200:
                data = r.json()
                return len(data.get("cues", []))
            return 0
        except Exception:
            return 0

    # ─── WebSocket listener ────────────────────────────────────────────

    _ws = None  # type: ignore

    async def _ensure_connected(self) -> bool:
        """Ensure the HTTP client is ready. Return True if ok."""
        try:
            await self.client.get(f"{self.base_url}/api/ping")
            return True
        except Exception:
            return False

    async def ws_listen(self, project: str) -> None:
        """Connect WebSocket and listen for real-time cue events.
        Yields (event_type, data) tuples. Returns on disconnect/error."""
        ws_url = self.base_url.replace("http://", "ws://").replace("https://", "wss://")
        ws_url = f"{ws_url}/ws?project={project}"
        try:
            import websockets
            async with websockets.client.connect(ws_url) as ws:
                self._ws = ws
                while True:
                    msg = await ws.recv()
                    try:
                        import json
                        data = json.loads(msg)
                        yield data.get("event", ""), data.get("data", {})
                    except json.JSONDecodeError:
                        continue
        except Exception:
            pass
        finally:
            self._ws = None

# ─── App ────────────────────────────────────────────────────────────────────

class ClavusApp(App):
    CSS = f"""
    ClavusApp {{ background: {C['bg']}; }}
    Screen {{ background: {C['bg']}; }}

    #main {{ layout: grid; grid-size: 1 3; grid-rows: auto 1fr auto; height: 100%; }}

    #header {{ height: 2; background: {C['surface']}; padding: 0 1; }}
    #header-title {{ color: {C['accent']}; text-style: bold; }}
    #header-status {{ color: {C['dim']}; }}

    #content {{ layout: grid; grid-size: 2 1; grid-columns: 5fr 2fr; height: 100%; }}

    #cues-list {{ height: 100%; min-height: 5; border: solid {C['border']}; border-left: solid transparent; background: transparent; }}
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
    #footer-keys {{ color: {C['accent']}; }}
    #footer-stats {{ color: {C['muted']}; text-align: right; }}
    #share-banner {{ display: none; padding: 0 1; background: {C['surface']}; color: {C['muted']}; height: 1; text-style: bold; }}
    #join-banner {{ display: none; padding: 0 1; background: {C['surface']}; color: {C['yellow']}; height: 1; text-style: bold; }}
    #footer-input {{ display: none; width: 100%; height: 3; background: {C['bg']}; border: solid {C['accent']}; color: {C['fg']}; padding: 0 1; }}
    #footer.input-mode #footer-input {{ display: block; }}
    #footer.input-mode #footer-keys {{ display: none; }}
    #footer.input-mode #footer-stats {{ display: none; }}

    Scrollbar {{ scrollbar-color: rgba(26,158,158,0.5) {C['border']}; }}
    Scrollbar > .scrollbar--grabber {{ background: rgba(26,158,158,0.4); }}
    Scrollbar.vertical > .scrollbar--grabber {{ min-height: 3; }}
    """

    BINDINGS = [
        Binding("q", "quit", "Quit"),
        Binding("r", "reply", "Reply"),
        Binding("e", "edit", "Edit"),
        Binding("c", "cue_new", "New cue"),
        Binding("s", "skip", "Skip"),
        Binding("T", "restore_snapshot", "Restore"),
        Binding("R", "resolve", "Resolve"),
        Binding("a", "assign", "Assign"),
        Binding("S", "start", "Start/Stop"),
        Binding("x", "archive", "Archive"),
        Binding("C", "snapshot", "Snapshot"),
        Binding("d", "diff", "Diff"),
        Binding("p", "pull", "Pull"),
        Binding("P", "push", "Push"),
        Binding("U", "stem_push", "Stem↑", show=False),
        Binding("tab", "focus_next_pane", "Pane"),
        Binding("j", "cursor_down", "Down", show=False),
        Binding("k", "cursor_up", "Up", show=False),
        Binding("down", "cursor_down", "Down", show=False),
        Binding("up", "cursor_up", "Up", show=False),
        Binding(":", "command", ":cmd", show=False),
        Binding("escape", "cancel_input", "Cancel input", show=False),
    ]

    def __init__(self, url: str = ""):
        super().__init__()
        self.server_url = url or os.environ.get("CLAVUS_SERVER", "http://localhost:7890")
        self.api = ClavusClient(self.server_url)
        self.project: str = ""
        self.connected: bool = False
        self.ws_connected: bool = False
        self.cues: list[Cue] = []
        self.snaps: list[Snap] = []
        self.idx: int = 0
        self._input_mode: str = ""  # "reply", "edit", "cue", "cue_pos", ""
        self._pending_cue_text: str = ""
        self._relay_proc = None  # Subprocess ref for auto-started relay
        from clavus.config import ClavusConfig
        _cfg = ClavusConfig.load()
        self.author = _cfg.author
        self._clavus_cfg = _cfg

    def _load_config(self) -> str:
        from clavus.config import ClavusConfig
        return ClavusConfig.load().author

    def _save_config(self):
        from clavus.config import ClavusConfig
        self._clavus_cfg.author = self.author
        self._clavus_cfg.save()

    def compose(self):
        with Container(id="main"):
            yield Horizontal(
                Static("~▼~ clavus", id="header-title"),
                Static("connecting...", id="header-status"),
                id="header",
            )
            yield Container(
                Static("", id="share-banner"),
                Static("", id="join-banner"),
                Container(ListView(id="clv"), id="cues-list"),
                Container(
                    Static(" History", classes="label"),
                    Container(ListView(id="hlv"), id="history-list"),
                    id="history",
                ),
                id="content",
            )
            yield Horizontal(
                Static("", id="footer-keys"),
                Input(placeholder="type here...", id="footer-input"),
                Static("", id="footer-stats"),
                id="footer",
            )

    def on_mount(self):
        self._update_header()
        self._update_footer()
        self._connect()

    # ─── Input bar ──────────────────────────────────────────────────────

    def _show_input(self, mode: str, prompt: str, prefill: str = ""):
        self._input_mode = mode
        footer = self.query_one("#footer")
        inp = self.query_one("#footer-input", Input)
        inp.value = prefill
        self.query_one("#footer-keys", Static).update(f"[{C['accent']}]{prompt}[/]")
        footer.add_class("input-mode")
        inp.focus()

    def _hide_input(self):
        self._input_mode = ""
        self.query_one("#footer").remove_class("input-mode")
        self.call_after_refresh(self._update_footer)

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
            self.query_one("#footer-keys", Static).update(
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
        elif mode == "command":
            self._do_command(text)

    def action_cancel_input(self):
        if self._input_mode:
            self._hide_input()
            self._focus_cues()
        else:
            # Dismiss any visible banner
            for bid in ("share-banner", "join-banner"):
                try:
                    banner = self.query_one(f"#{bid}", Static)
                    if banner.styles.display != "none":
                        banner.styles.display = "none"
                except NoMatches:
                    pass

    # ─── Command mode ──────────────────────────────────────────────────

    def action_command(self):
        self._show_input("command", ":", prefill="")

    def _do_command(self, text: str):
        parts = text.strip().split(maxsplit=1)
        if not parts:
            return
        cmd = parts[0].lower()
        arg = parts[1] if len(parts) > 1 else ""

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
        elif cmd in ("status", "info"):
            self._run_status()
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
            self.push_screen(JoinModal(arg))
        elif cmd in ("help", "h", "?"):
            self._status("commands: project, projects, init, browse, name, inject, restore, snapshot, archive, delete, share, join, backup, backups, restore-store, stem push/pull, status, help | C=snapshot")
        else:
            self._status(f"unknown: {cmd}")

    @work(exclusive=False)
    async def _run_switch_project(self, name: str):
        self._status(f"switching to {name}...")
        info = await self.api.get_project_info(name)
        if not info:
            self._status(f"project '{name}' not found")
            return
        self.project = name
        self.connected = True
        self._update_header()
        cues, snaps = await self.api.pull(self.project) if self.project else (None, None)
        if cues:
            self.cues = self._sort_cues(cues)
        if snaps:
            self.snaps = snaps
        self.idx = 0
        self._update_header()
        self._render()
        self._update_footer()
        # Restart WebSocket listener for new project
        self._start_ws_listener(name)
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
        projects = await self.api.list_projects()
        if not projects:
            self._status("no projects found  —  run :init <path> to add one")
            return
        lines = []
        for p in projects:
            name = p.get("name", "?")
            head = p.get("head", "")
            branch = p.get("branch", "main")
            active = " ◀" if name == self.project else ""
            lines.append(f"  {name}  @ {head or '(no snaps)':12s}  [{branch}]{active}")
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
        """Import a project from a filesystem path."""
        self._status(f"importing {path}...")
        result = await self.api.init_project(path)
        if result is None:
            self._status(f"failed to reach server for init")
            return
        if "error" in result:
            self._status(f"error: {result['error']}")
            return
        proj = result.get("project", {})
        if "info" in result:
            # Already registered — just switch to it
            self._status(f"already tracked, switching to {proj.get('name', '?')}")
        else:
            self._status(f"imported: {proj.get('name', '?')} ({proj.get('tracks', '?')} tracks @ {proj.get('bpm', '?')}bpm)")
        # Auto-switch to the new project
        if proj.get("name"):
            self.project = proj["name"]
            self.connected = True
            self._update_header()
            await self._do_pull()

    @work(exclusive=False)
    async def _run_browse(self, directory: str):
        """Browse a directory for .als files and navigate."""
        # Normalize relative paths — single directory names navigate into subdir
        if directory and "/" not in directory and directory not in ("..", ".", ""):
            if hasattr(self, "_last_browse_dir") and self._last_browse_dir:
                directory = os.path.join(self._last_browse_dir, directory)
        self._last_browse_dir = directory
        result = await self.api.browse_dir(directory)
        if result is None:
            self._status(f"failed to browse {directory}")
            return
        if "error" in result:
            self._status(f"error: {result['error']}")
            return
        cur = result.get("current_dir", "?")
        parent = result.get("parent_dir")
        subdirs = result.get("subdirs", [])
        als_files = result.get("als_files", [])
        # Show results in status bar
        registered = [f for f in als_files if f.get("registered")]
        unregistered = [f for f in als_files if not f.get("registered")]
        lines = [f"📁 {cur}"]
        if subdirs:
            lines.append(f"  dirs: {' '.join(subdirs[:8])}{'...' if len(subdirs)>8 else ''}")
        if registered:
            lines.append(f"  ✅ als: {' '.join(f['name'] for f in registered)}")
        if unregistered:
            lines.append(f"  🔵 als: {' '.join(f['name'] for f in unregistered)}")
        self._status(" | ".join(lines))
        # Offer subdir navigation or init
        if unregistered:
            hint = f" — :init {cur}/<name> to import"
        else:
            hint = ""
        self._show_input("browse", "browse (enter subdir, .. up, or :init): ", prefill="")

    @work(exclusive=False)
    async def _run_inject(self):
        """Inject cues as Ableton markers."""
        if not self.project:
            self._status("no project selected")
            return
        self._status("injecting cues into .als...")
        result = await self.api.inject_cues(self.project)
        if result is None:
            self._status("failed to inject — server unreachable")
            return
        if "error" in result:
            self._status(f"error: {result['error']}")
            return
        injected = result.get("injected", 0)
        self._status(f"injected {injected} cue(s) as Ableton markers")
        self._log_event(f"injected {injected} cues as markers")
        if injected > 0:
            self._status(f"save + reopen the .als in Ableton to see markers")

    @work(exclusive=False)
    async def _run_restore(self, hash_str: str = ""):
        """Restore the .als from a snapshot backup."""
        if not self.project:
            self._status("no project selected")
            return
        self._status(f"restoring from {'HEAD' if not hash_str else hash_str}...")
        result = await self.api.restore_snapshot(self.project, hash_str)
        if result is None:
            self._status("failed to restore — server unreachable")
            return
        if "error" in result:
            detail = result.get("detail", "")
            msg = f"error: {result['error']}"
            if detail:
                msg += f" — {detail}"
            self._status(msg)
            return
        msg = result.get("message", "?")
        captured = result.get("captured", "?")
        self._status(f"restored to snapshot: '{msg}' ({captured})")
        self._log_event(f"restored to '{msg}'")
        # Re-pull to update cues/snapshots display
        await self._do_pull()

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
        """Create a new snapshot of the current project state."""
        if not self.project:
            self._status("no project selected")
            return
        if not message:
            self._status("usage: :snapshot <message>")
            return
        self._status("creating snapshot...")
        result = await self.api.create_snapshot(self.project, message)
        if result is None:
            self._status("failed to create snapshot — server unreachable")
            return
        if "error" in result:
            self._status(f"error: {result['error']}")
            return
        status = result.get("status", "?")
        if status == "no_change":
            self._status(f"no changes — HEAD already at {result.get('hash', '?')}")
        else:
            h = result.get("hash", "?")
            t = result.get("tracks", "?")
            b = result.get("bpm", "?")
            self._status(f"snapshot {h} — {t} tracks @ {b}bpm")
            self._log_event(f"snapshot: {message[:30]}")
        # Re-pull to show new snapshot in history
        await self._do_pull()

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
            return
        cue.status = "resolved" if cue.status == "pending" else "pending"
        self._render()
        self._status("resolved" if cue.status == "resolved" else "unresolved")
        self._save()

    def action_skip(self):
        cue = self._get_cue()
        if not cue:
            return
        cue.status = "skipped" if cue.status != "skipped" else "pending"
        self._render()
        self._status("skipped" if cue.status == "skipped" else "unskipped")
        self._save()

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

    def action_diff(self):
        """Show a concise text summary of changes in the selected snapshot."""
        if not self.snaps:
            self._status("no snapshots to diff")
            return
        idx = self._get_history_idx()
        snap = self.snaps[idx]
        self._status(f"diff of {snap.hash}...")
        self.refresh()

        import io, contextlib
        from clavus.cli import get_store_and_project

        store, proj = get_store_and_project()
        from clavus.helpers import resolve_snapshot
        hash_str = resolve_snapshot(store, snap.hash)
        current_snap = store.load_snapshot(hash_str) if hash_str else None

        if not current_snap or not current_snap.parent:
            self._status("no parent snapshot to diff against")
            return

        parent_project = store.load_project(current_snap.parent)
        current_project = store.load_project(hash_str)
        if not parent_project or not current_project:
            self._status("could not load project data")
            return

        from clavus.store import diff_projects

        diff = diff_projects(parent_project, current_project)

        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            print(f"  {current_snap.short_hash()} — '{current_snap.message}'")
            print()

            # Changed tracks
            changed = [t for t in diff.tracks if t.status != "unchanged"]
            if changed:
                for td in changed:
                    icon = {"added": "+", "removed": "-", "modified": "~"}.get(td.status, " ")
                    desc = td.status
                    if td.devices_added:
                        desc += f" +{','.join(d[:12] for d in td.devices_added[:2])}"
                    if td.devices_removed:
                        desc += f" -{','.join(d[:12] for d in td.devices_removed[:2])}"
                    print(f"  {icon} {td.name}")
                    print(f"    {desc}")
            else:
                print("  (no structural track changes)")

            # Clip count changes
            parent_name_map = {t.name: t for t in parent_project.tracks}
            current_name_map = {t.name: t for t in current_project.tracks}
            clip_changes = []
            for td in diff.tracks:
                pt = parent_name_map.get(td.name)
                ct = current_name_map.get(td.name)
                bc = len(pt.clips) if pt else 0
                ac = len(ct.clips) if ct else 0
                if bc != ac:
                    clip_changes.append((td.name, bc, ac))
            if clip_changes:
                print()
                for name, bc, ac in clip_changes:
                    delta = ac - bc
                    sign = "+" if delta > 0 else ""
                    print(f"  · {name}: {bc}→{ac} clips ({sign}{delta})")

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
                print()
                print(f"  {' | '.join(parts)}")

            print()
            print(f"  For visual diff: open the web companion (port 7890)")

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
        """Prompt for a snapshot message then create one."""
        self._show_input("command", ":", prefill="snapshot ")

    def action_assign(self):
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
        cue = self._get_cue()
        if not cue:
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
                         prefill="")

    async def _do_archive_cue(self):
        """Actually archive the cue (after confirmation)."""
        cue = self._get_cue()
        if not cue:
            return
        self._status(f"archiving {cue.id[:8]}...")
        ok = await self.api.archive_cue(self.project, cue.id)
        if ok:
            self._status("archived — re-pulling")
            self._log_event(f"archived @{cue.position}")
            await self._do_pull()
        else:
            self._status("archive failed")

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
                         prefill="")

    async def _do_delete_cue(self):
        """Actually delete the cue (after confirmation)."""
        cue = self._get_cue()
        if not cue:
            return
        self._status(f"deleting {cue.id[:8]}...")
        ok = await self.api.delete_cue(self.project, cue.id)
        if ok:
            self._status("deleted — re-pulling")
            self._log_event(f"deleted @{cue.position}")
            await self._do_pull()
        else:
            self._status("delete failed")

    def _run_share(self):
        """Start a share session — spawn relay + show share code modal."""
        from clavus.discovery import generate_share_code
        from clavus.config import ClavusConfig
        import subprocess, os

        cfg = ClavusConfig.load()
        code = generate_share_code()
        lan_ip = self._lan_ip()
        port = cfg.port

        # Spawn the relay server as a subprocess with the share code
        env = os.environ.copy()
        env["CLAVUS_SHARE_CODE"] = code
        proc = subprocess.Popen(
            [sys.executable, "-m", "clavus", "relay", "--port", str(port)],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

        def stop_relay():
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()

        self.push_screen(ShareModal(code, lan_ip, port, stop_relay))

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

    def _start_relay(self) -> subprocess.Popen | None:
        """Start a relay server in the background."""
        import subprocess
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

    @work(exclusive=True)
    async def action_pull(self):
        self._status("pulling...")
        await self._do_pull()
        self._status("pulled")
        self._log_event("pulled from server")

    @work(exclusive=True)
    async def action_push(self):
        self._status("pushing...")
        await self._do_push()
        self._log_event("pushed to server")

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
        """Push changes to server in the background."""
        self.post_message(SaveRequest())

    @work(exclusive=False, group="save")
    async def _do_save(self):
        if not self.project:
            return
        ok = await self.api.push(self.project, self.cues)
        if ok:
            self._status("saved")
        else:
            self._status("save failed")

    def on_save_request(self, event: SaveRequest):
        self._do_save()

    # ─── Connection ─────────────────────────────────────────────────────

    @work(exclusive=True)
    async def _connect(self):
        """Worker entry point: check server, auto-start relay if needed, then connect."""
        self._status("connecting...")
        if await self.api.ping():
            await self._do_connect()
            return

        # No server running — try auto-start relay
        if os.environ.get("CLAVUS_NO_AUTO_RELAY"):
            self.connected = False
            self._status("server offline — start clavus relay")
            self._update_header()
            self._update_footer()
            return

        self._status("server offline — starting relay...")
        self._relay_proc = self._start_relay()
        if self._relay_proc:
            import asyncio
            for attempt in range(10):
                await asyncio.sleep(0.5)
                if await self.api.ping():
                    self._status("relay started, connecting...")
                    await self._do_connect()
                    return
            self._status("relay failed to start — run 'clavus relay' manually")
        else:
            self.connected = False
            self._status("server offline — run 'clavus relay'")
        self._update_header()
        self._update_footer()

    async def _do_connect(self):
        """Core connection logic: load project, pull cues, start WS listener.

        Auto-selects the last edited project if available, or the first
        project in the store. Falls back gracefully if no projects exist.
        """
        self._status("connected, loading project...")
        projects = await self.api.list_projects()
        if not projects:
            self.connected = False
            self._status("no project — run clavus init or :projects to switch")
            self._update_header()
            self._update_footer()
            return

        # Try _last_project from server, fall back to first available
        info = await self.api.get_project()
        target = info.get("name", "") if info else ""
        if target and any(p.get("name") == target for p in projects):
            self.project = target
        else:
            self.project = projects[0].get("name", "")

        self.connected = True
        self._status(f"project: {self.project}")

        self._update_header()
        self._update_footer()
        if self.connected:
            self._status("pulling...")
            await self._do_pull()
            self._status(f"loaded {len(self.cues)} cues")
            self._start_ws_listener(self.project)

    @work(exclusive=False, group="ws")
    async def _ws_listener(self):
        """Background worker: listen for real-time cue events over WebSocket.
        Auto-reconnects on disconnect with 3s backoff.
        Uses self._ws_target_project to know which project to listen for,
        so switching projects restarts the listener."""
        target = getattr(self, "_ws_target_project", self.project)
        if not target:
            return
        while True:
            try:
                self.ws_connected = True
                self._update_header()
                async for event, data in self.api.ws_listen(target):
                    self._status(f"ws: {event}")
                    if event == "cue_new":
                        await self._do_pull()
                    elif event in ("cue_reply", "cue_update"):
                        await self._do_pull()
            except asyncio.CancelledError:
                raise
            except Exception:
                pass
            self.ws_connected = False
            self._update_header()
            self._status("ws disconnected, reconnecting in 3s...")
            try:
                await asyncio.sleep(3)
            except asyncio.CancelledError:
                raise

    def _start_ws_listener(self, project: str):
        """Start WebSocket listener for a specific project.
        Cancels any existing WS listener first."""
        self._ws_target_project = project
        # Cancel previous WS worker if running (workers are stored by name on the app)
        try:
            existing = self.get_worker("_ws_listener")
            if existing and existing.state not in (WorkerState.SUCCESS, WorkerState.CANCELLED):
                existing.cancel()
        except Exception:
            pass
        self._ws_listener()

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
        if not self.project:
            return
        cues, snaps = await self.api.pull(self.project)
        if cues:
            self.cues = self._sort_cues(cues)
        self.snaps = snaps
        self.idx = min(self.idx, len(self.cues) - 1) if self.cues else 0
        self._update_header()
        self._render()
        # Restore scroll position after re-render
        try:
            lv = self.query_one("#clv", ListView)
            if self.idx < len(lv.children):
                lv.index = self.idx
                target = lv.children[self.idx]
                lv.scroll_to_widget(target, animate=False)
        except (NoMatches, IndexError):
            pass

    async def _do_push(self):
        if not self.project:
            return
        ok = await self.api.push(self.project, self.cues)
        self._status("pushed" if ok else "push failed")

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
                self._status("cues")
            elif self.query_one("#cues-list").has_focus or clv.has_focus:
                hlv.focus()
                self._status("history")
            else:
                clv.focus()
                self._status("cues")
        except NoMatches:
            pass

    def on_list_view_highlighted(self, event: ListView.Highlighted):
        if event.list_view.id == "clv":
            self.idx = event.list_view.index
            self._update_footer()

    # ─── Status / Header / Footer ───────────────────────────────────────

    def _status(self, msg: str):
        """Show a status message in the footer and log it to the event list."""
        try:
            safe_msg = msg.replace("[", "\\[").replace("]", "\\]")
            self.query_one("#footer-stats", Static).update(f"[{C['dim']}]{safe_msg}[/]")
            self.refresh()
        except NoMatches:
            pass

    def _log_event(self, event: str):
        """Append a timestamped event to the top of the cue list as a log entry."""
        try:
            ts = time.strftime("%H:%M:%S")
            safe = event.replace("[", "\\[").replace("]", "\\]")
            lv = self.query_one("#clv", ListView)
            label = Label(f"  [{C['dim']}]{ts}[/] [{C['accent']}⟩[/] {safe}", classes="event-log")
            lv.mount(label, before=0)
            # Keep max 5 log entries, remove oldest
            log_entries = [c for c in lv.children if hasattr(c, "classes") and "event-log" in c.classes]
            while len(log_entries) > 5:
                log_entries[-1].remove()
                log_entries.pop()
            self.set_timer(8.0, lambda: self._clear_log_events())
        except NoMatches:
            pass

    def _clear_log_events(self):
        try:
            lv = self.query_one("#clv", ListView)
            for c in list(lv.children):
                if hasattr(c, "classes") and "event-log" in c.classes:
                    c.remove()
        except NoMatches:
            pass

    def _update_header(self):
        try:
            ws_dot = "⚡" if self.ws_connected else ""
            dot = "⬤" if self.connected else "◌"
            conn_color = C['green'] if self.connected else C['dim']
            conn = "connected" if self.connected else "offline"
            proj = f"  [white]{self.project}[/]" if self.project else ""
            # Show server URL when relay is auto-started
            relay_info = ""
            if self._relay_proc and self._relay_proc.poll() is None:
                relay_info = f"  [{C['dim']}]⧩ relay[/]"
            self.query_one("#header-title", Static).update(
                f"[bold {C['accent']}]~▼~ clavus[/]{proj}{relay_info}")
            self.query_one("#header-status", Static).update(
                f"{ws_dot}[{conn_color}]{dot} {conn}[/]"
                f"  [{C['dim']}]{len(self.cues)} cues[/]"
                f"  [{C['muted']}]{self.server_url}[/]")
        except NoMatches:
            pass

    def _update_footer(self):
        try:
            self.query_one("#footer-keys", Static).update(
                f"[{C['accent']}]r[/] reply  "
                f"[{C['accent']}]R[/] resolve  "
                f"[{C['accent']}]e[/] edit  "
                f"[{C['accent']}]c[/] cue  "
                f"[{C['accent']}]C[/] snap  "
                f"[{C['accent']}]a[/] assign  "
                f"[{C['accent']}]x[/] archive  "
                f"[{C['accent']}]U[/] stems  "
                f"[{C['accent']}]q[/] quit  "
                f"[{C['accent']}]:[/] cmd"
            )
            self.query_one("#footer-stats", Static).update(
                f"[{C['muted']}]j/k navigate | {len(self.cues)} cues[/]")
        except NoMatches:
            pass

    def _focus_cues(self):
        try:
            self.query_one("#clv", ListView).focus()
        except NoMatches:
            pass

    # ─── Rendering ──────────────────────────────────────────────────────

    def _render(self):
        try:
            self._render_cues()
            self._render_history()
            self._update_footer()
        except Exception as e:
            self._status(f"render error: {e}")

    def _render_cues(self):
        lv = self.query_one("#clv", ListView)
        lv.clear()

        if not self.cues:
            lv.append(ListItem(Label(f"  [{C['dim']}]no cues yet  (c to create one)[/]")))
            return

        for i, c in enumerate(self.cues):
            color = C["yellow"] if c.status == "pending" else (
                C["green"] if c.status == "resolved" else C["muted"])
            dot = "●" if c.status == "pending" else ("✓" if c.status == "resolved" else "–")
            rc = f" [{C['dim']}]{len(c.replies)}r[/]" if c.replies else ""
            assignee_part = f"  👤 {c.assignee}" if c.assignee else ""
            in_prog = f" [{C['yellow']}]▶[/]" if c.in_progress else ""
            safe_text = c.text[:60].replace("[", "\\[").replace("]", "\\]")
            cue_line = (
                f"  [{color}]{dot}[/] [dim]@{c.position}[/] "
                f"[{C['fg']}]{safe_text}[/]"
                f" [{C['muted']}]{c.id[:8]}[/]{rc}"
            )
            lines = [cue_line]
            if assignee_part:
                lines.append(f"  [{C['dim']}]├──[/]{assignee_part}{in_prog}")
            elif in_prog:
                lines.append(f"  [{C['dim']}]├──[/]  [{C['yellow']}]▶[/]")
            if c.replies:
                for j, r in enumerate(c.replies):
                    tag = r.author or "anon"
                    ts = time.strftime("%H:%M", time.localtime(r.timestamp)) if r.timestamp else ""
                    conn = "╰─" if j == len(c.replies) - 1 else "├─"
                    safe_reply = r.text[:55].replace("[", "\\[").replace("]", "\\]")
                    lines.append(
                        f"  [{C['dim']}]{conn} {tag} [{C['muted']}]{ts}[/]"
                        f"  [{C['dim']}]{safe_reply}[/]"
                    )
            lv.append(ListItem(Label("\n".join(lines))))

        if self.idx < len(lv.children):
            lv.index = self.idx

    def _render_history(self):
        lv = self.query_one("#hlv", ListView)
        lv.clear()
        if not self.snaps:
            lv.append(ListItem(Label(f"  [{C['dim']}]no snapshots yet[/]")))
            return
        for s in self.snaps[:10]:
            ts = time.strftime("%m/%d %H:%M", time.localtime(s.timestamp)) if s.timestamp else ""
            safe_msg = s.message[:50].replace("[", "\\[").replace("]", "\\]")
            lv.append(ListItem(Label(
                f"[{C['accent']}]{s.hash}[/] [{C['dim']}]{ts}[/]"
                f"  [{C['fg']}]{safe_msg}[/]"
            )))


# ─── Messages ───────────────────────────────────────────────────────────────

class SaveRequest(Message):
    pass


# ─── Modals ──────────────────────────────────────────────────────────────────

class ShareModal(ModalScreen[None]):
    """Modal showing share code and instructions."""

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
    #share-code {{
        text-style: bold;
        color: {C['fg']};
        padding: 1 2;
        background: {C['surface2']};
    }}
    #share-hint, #share-url {{
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

    def __init__(self, code: str, lan_ip: str, port: int, stop_cb) -> None:
        super().__init__()
        self.code = code
        self.lan_ip = lan_ip
        self.port = port
        self.stop_cb = stop_cb

    def compose(self) -> ComposeResult:
        with Vertical(id="share-box"):
            yield Static("🔗  Share Session — relay running", id="share-title")
            yield Static(f"  {self.code}  ", id="share-code")
            yield Static(
                f"Tell your friend to run:  clavus join",
                id="share-hint",
            )
            yield Static(
                f"Or connect to:  http://{self.lan_ip}:{self.port}",
                id="share-url",
            )
            with Horizontal(classes="share-actions"):
                yield Button("Stop Share", id="stop-share", variant="error")
            yield Static("Esc to close", id="share-footer")

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
                    r = client.client.get(f"http://{peer.host}:{peer.port}/api/share", timeout=5)
                    if r.status_code == 200:
                        return r.json()
                except Exception:
                    pass
                return None

            relay_info = []
            with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
                fut_map = {pool.submit(_get_info, p): p for p in peers}
                for fut in concurrent.futures.as_completed(fut_map, timeout=5):
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
