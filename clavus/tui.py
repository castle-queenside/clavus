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
from typing import Optional

from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Container, Horizontal, Vertical
from textual.screen import ModalScreen
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

# ─── App ────────────────────────────────────────────────────────────────────

class ClavusApp(App):
    CSS = f"""
    ClavusApp {{ background: {C['bg']}; }}
    Screen {{ background: {C['bg']}; }}

    #main {{ layout: grid; grid-size: 1 3; grid-rows: auto 1fr auto; height: 100%; }}

    #header-title {{ color: {C['accent']}; text-style: bold; background: {C['surface']}; padding: 0 1; }}

    #content {{ layout: grid; grid-size: 2 1; grid-columns: 5fr 2fr; height: 100%; }}

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
        Binding("!", "resolve_conflict", "Conflict"),
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
        self._pending_cue_text: str = ""
        self._relay_proc = None
        self._busy: bool = False
        self._last_sync: str = ""     # "⬆ push ✓ 12:34" or "⬇ pull ✓ 12:34"
        self._peer_name: str = ""     # remote name (e.g. "mac")
        self._peer_reachable: bool = False
        _cfg = ClavusConfig.load()
        self.author = _cfg.author
        self._clavus_cfg = _cfg
        self._cue_store = None  # Lazy init per project

    def _load_config(self) -> str:
        from clavus.config import ClavusConfig
        return ClavusConfig.load().author

    def _save_config(self):
        from clavus.config import ClavusConfig
        self._clavus_cfg.author = self.author
        self._clavus_cfg.save()

    def compose(self):
        with Container(id="main"):
            yield Static("~▼~ clavus", id="header-title")
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
            self._status("commands: project, projects, init, setup, browse, name, inject, restore, open, snapshot, archive, delete, share, join, backup, backups, restore-store, stem push/pull, log, config, remote, branch, status, doctor, help | C=snapshot")
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
        """Import a project from a filesystem path via CLI subprocess."""
        import asyncio
        self._status(f"importing {path}...")
        try:
            proc = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "clavus", "init", path,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=60)
            out = stdout.decode().strip()
            if out:
                for line in out.split("\n"):
                    self._log_event(line.strip())
            if proc.returncode != 0:
                self._status("init failed")
                return
            self._status(f"imported: {path}")
            # Reload from disk
            self._connect()
        except Exception as e:
            self._status(f"init error: {e}")

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
        """Inject cues as Ableton markers via CLI subprocess."""
        if not self.project:
            self._status("no project selected")
            return
        import asyncio
        self._status("injecting cues into .als...")
        try:
            proc = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "clavus", "inject",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=30)
            out = stdout.decode().strip()
            if out:
                for line in out.split("\n"):
                    self._log_event(line.strip())
            self._status("inject complete" if proc.returncode == 0 else "inject failed")
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
        """Materialize the .als to a usable path and optionally open it."""
        if not self.project:
            self._status("no project selected")
            return
        from clavus.store import BlobStore
        from pathlib import Path
        store = BlobStore()
        proj = store.get_index(self.project)
        if not proj:
            self._status("project not found in store")
            return

        h = hash_str or proj.head
        if not h:
            self._status("no snapshots to open")
            return

        # Resolve short hash
        from clavus.helpers import resolve_snapshot, get_projects_dir
        resolved = resolve_snapshot(store, h)
        if not resolved:
            self._status(f"could not resolve hash: {h}")
            return

        snap = store.load_snapshot(resolved)
        if not snap or not snap.als_hash:
            self._status("snapshot has no .als backup")
            return

        raw = store.get_object(snap.als_hash)
        if not raw:
            self._status("raw .als blob missing — try pulling")
            return

        # Write to a proper Ableton project folder on Desktop
        project_name = self.project.replace(" ", " ")
        project_dir = get_projects_dir() / project_name
        out = project_dir / f"{project_name}.als"

        out.parent.mkdir(parents=True, exist_ok=True)

        # Materialize audio samples into the project folder first (so they exist)
        sample_count = 0
        if snap.sample_hashes:
            # Extract filename → RelativePath mapping from the original .als
            import gzip, re
            _xml = gzip.decompress(raw).decode("utf-8", errors="replace")
            _als_relpaths: dict[str, str] = {}
            for _m in re.finditer(r'<RelativePath\s+Value="([^"]+)"', _xml):
                _rp = _m.group(1)
                _fn = _rp.rsplit("/", 1)[-1].rsplit("\\", 1)[-1]
                if _fn not in _als_relpaths:
                    _als_relpaths[_fn] = _rp

            base_dir = out.parent  # Project folder root
            for sh in snap.sample_hashes:
                fname = store.get_sample_filename(sh)
                # Prefer .als RelativePath (preserves subdirectory structure)
                relpath = _als_relpaths.get(fname) or store.get_sample_relpath(sh) or ""
                if fname and store.has_object(sh):
                    try:
                        store.materialize_sample(sh, base_dir, fname, relpath)
                        sample_count += 1
                    except Exception:
                        pass

        # Rewrite .als sample paths to point to local project folder (cross-OS fix)
        from clavus.parser import rewrite_als_sample_paths
        raw = rewrite_als_sample_paths(raw, out.parent)

        out.write_bytes(raw)

        msg = f"opened {self.project}.als → {out}"
        if sample_count:
            msg += f" + {sample_count} samples"
        self._status(msg)
        self._log_event(msg)

        # Launch if Ableton available (macOS only for now)
        import platform, subprocess as sp
        if platform.system() == "Darwin":
            for v in ["12", "11", "10"]:
                able = Path(f"/Applications/Ableton Live {v}/Ableton Live {v}.app")
                if able.exists():
                    sp.Popen(["open", "-a", str(able), str(out)])
                    break
            else:
                sp.Popen(["open", str(out)])
        elif platform.system() == "Windows":
            sp.Popen(["start", "", str(out)], shell=True)

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
        """Manage remotes: list, add, remove."""
        import subprocess, sys
        try:
            cmd = [sys.executable, "-m", "clavus", "remote"]
            if action:
                cmd.append(action)
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
            out = proc.stdout.strip()
            if out:
                for line in out.split("\n")[:5]:
                    self._log_event(line)
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
        """Create a new snapshot via CLI subprocess."""
        if not self.project:
            self._status("no project selected")
            return
        if not message:
            self._status("usage: :snapshot <message>")
            return
        import asyncio
        self._status("creating snapshot...")
        try:
            proc = await asyncio.create_subprocess_exec(
                sys.executable, "-m", "clavus", "snapshot", message,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=30)
            out = stdout.decode().strip()
            if out:
                for line in out.split("\n"):
                    if line.strip():
                        self._log_event(line.strip())
            self._status("snapshot complete" if proc.returncode == 0 else "snapshot failed")
        except Exception as e:
            self._status(f"snapshot error: {e}")
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

    def action_resolve_conflict(self):
        """Resolve a sync conflict on the selected cue — keep local or remote version."""
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
        """Show what changed in the selected snapshot vs its parent."""
        if not self.snaps:
            self._status("no snapshots to diff")
            return
        idx = self._get_history_idx()
        snap = self.snaps[idx]

        # Load the snapshot and its parent
        current_snap = self.store.load_snapshot(snap.hash)
        if not current_snap or not current_snap.parent:
            self._status("no parent snapshot to diff against")
            return

        parent_project = self.store.load_project(current_snap.parent)
        current_project = self.store.load_project(snap.hash)
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
                         prefill="")

    async def _do_delete_cue(self):
        """Actually delete the cue (after confirmation)."""
        cue = self._get_cue()
        if not cue:
            return
        if not self._cue_store:
            self._status("no cue store")
            return
        self._status(f"deleting {cue.id[:8]}...")
        try:
            self._cue_store.delete_cue(cue.id)
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

    @work(exclusive=True)
    async def action_pull(self):
        self._busy = True
        self._status("\u23f3 pulling...")
        try:
            await self._do_pull()
        finally:
            self._busy = False

    @work(exclusive=True)
    async def action_push(self):
        self._busy = True
        self._status("\u23f3 pushing...")
        try:
            await self._do_push()
        finally:
            self._busy = False

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
        except Exception:
            self._status("save failed")

    # ─── Connection ─────────────────────────────────────────────────────

    @work(exclusive=True)
    async def _connect(self):
        """Load project data directly from the local store (no web server needed)."""
        self._status("loading from disk...")
        projects = self.store.list_projects()
        if not projects:
            self._status("no project — use :init <path> to add one")
            self._update_header()
            self._update_footer()
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
        # Assume reachable if we have remotes (verified on first pull/push)
        self._peer_reachable = bool(remotes)
        self._update_header()
        self._update_footer()

        # Load cues from disk
        self._load_cues_from_disk()
        # Load snapshots from store
        self._load_snapshots_from_disk()
        self._status(f"{len(self.cues)} cues, {len(self.snaps)} snapshots")

    def _load_cues_from_disk(self):
        """Load cues for the current project from CueStore."""
        if not self.project:
            self._log_event("_load_cues: no project set")
            return
        from clavus.cues import CueStore, CueFilter
        cue_store = CueStore(self.project, store=self.store)
        self._cue_store = cue_store
        all_cues = cue_store.list_cues(CueFilter())
        self._log_event(f"_load_cues: {len(all_cues)} cue(s) from {cue_store.cues_dir}")
        self.cues = self._sort_cues(all_cues)
        self.idx = min(self.idx, len(self.cues) - 1) if self.cues else 0

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
        self.snaps = history
        self._render_history()

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
        import time
        from clavus.sync import load_remotes, pull_from_remote, pull_snapshot_blobs, SyncClient
        from clavus.store import ClavusProject
        self._status("\u23f3 pulling...")
        try:
            proj_index = self.store.get_index(self.project) if self.project else None
            remotes = load_remotes(self.store)

            # If no local project, auto-discover from remotes
            if not proj_index:
                if not remotes:
                    self._status("\u274c no remotes — use :join http://...")
                    self._log_event("\u274c no remotes — use :join http://...")
                    return

                self._log_event(f"no local project — trying {len(remotes)} remote(s)...")
                # Try external remotes first (localhost is slow/unreachable on Windows)
                remotes_sorted = sorted(remotes, key=lambda r: 0 if "localhost" in r.url else -1)
                pulled_any = False
                for remote in remotes_sorted:
                    self._log_event(f"  probing {remote.name} ({remote.url})...")
                    client = SyncClient(remote.url)
                    try:
                        r = client.client.get(f"{remote.url}/api/projects", timeout=10)
                        if r.status_code != 200:
                            continue
                        projects = r.json().get("projects", [])
                        if not projects:
                            continue
                        for pdata in projects:
                            pname = pdata["name"]
                            if self.store.get_index(pname):
                                proj_index = self.store.get_index(pname)
                            else:
                                self._log_event(f"pulling {pname} from {remote.name}...")
                                r2 = client.client.get(
                                    f"{remote.url}/api/sync/pull",
                                    params={"name": pname}, timeout=30)
                                if r2.status_code != 200:
                                    continue
                                info = r2.json().get("project", {})
                                new_proj = ClavusProject(
                                    name=pname,
                                    root_als=info.get("root_als", f"~/{pname}/{pname}.als"),
                                    created_at=time.time(),
                                )
                                self.store.set_index(new_proj)
                                proj_index = new_proj
                                self._log_event(f"  created local project '{pname}'")
                            # Pull data
                            remote_ref = remote
                            result = pull_from_remote(self.store, proj_index, remote_ref)
                            blob_count = pull_snapshot_blobs(self.store, proj_index, remote_ref)
                            parts = []
                            if result.get("cues"): parts.append(f"{result['cues']} cues")
                            if result.get("snapshots"): parts.append(f"{result['snapshots']} snapshots")
                            if blob_count: parts.append(f"{blob_count} blob(s)")
                            msg = f"  got {', '.join(parts)}" if parts else "  already up to date"
                            self._log_event(msg)
                            pulled_any = True
                        if pulled_any:
                            break
                    except Exception as e:
                        self._log_event(f"  {remote.name}: {e}")
                        continue
                    finally:
                        client.close()

                if not pulled_any:
                    self._status("\u274c no projects found on any remote")
                    self._log_event("\u274c no projects found — is relay running?")
                    return

                # Switch to the pulled project
                if proj_index:
                    self.project = proj_index.name
                    self._peer_name = remotes[0].name if remotes else ""
                    self._peer_reachable = True
                    self._load_cues_from_disk()
                    self._load_snapshots_from_disk()
                    self._update_header()
                    self._render()
                    self._status(f"\u2705 pulled {self.project}: {len(self.cues)} cues, {len(self.snaps)} snaps")
                    self._log_event(f"\u2705 pull: {self.project} — {len(self.cues)} cues, {len(self.snaps)} snaps")
                return

            # ── Normal pull for existing project ──
            if not remotes:
                self._status("\u274c no remotes configured")
                self._log_event("\u274c no remotes — use :join http://...")
                return
            for remote in remotes:
                self._log_event(f"pulling from {remote.name}...")
                self._status(f"\u23f3 pulling from {remote.name}...")
                result = pull_from_remote(self.store, proj_index, remote)
                if result.get("error"):
                    self._log_event(f"\u274c {result['error']}")
                    self._status(f"\u274c {result['error']}")
                    return
                cues_n = result.get("cues", 0)
                snaps_n = result.get("snapshots", 0)
                conflicts_n = result.get("conflicts", 0)
                log_msg = f"  got {cues_n} cues, {snaps_n} snapshots"
                if conflicts_n:
                    log_msg += f" — \u26a0 {conflicts_n} conflict(s)"
                self._log_event(log_msg)
                blobs = pull_snapshot_blobs(self.store, proj_index, remote)
                if blobs:
                    self._log_event(f"  {blobs} blob(s)")
            ts = time.strftime("%H:%M")
            self._peer_reachable = True
            self._last_sync = f"\u2b07 pull \u2713 {ts}"
            self._update_header()
            self._log_event(f"\u2705 pull: {len(self.cues)} cues, {len(self.snaps)} snapshots")
            self._status(f"\u2705 pull: {len(self.cues)} cues, {len(self.snaps)} snapshots")
            # Refresh from disk
            if self.project:
                self._load_cues_from_disk()
                self._load_snapshots_from_disk()
                self._update_header()
                self._render()
        except Exception as e:
            self._log_event(f"\u274c pull error: {e}")
            self._status(f"\u274c pull error: {e}")

    async def _do_push(self):
        """Push cues + snapshots + blobs to remotes — direct function calls, no subprocess."""
        import time
        from clavus.sync import load_remotes, push_to_remote, push_snapshot_blobs
        self._status("\u23f3 pushing...")
        try:
            proj_index = self.store.get_index(self.project)
            if not proj_index:
                self._status("\u274c no project")
                return
            remotes = load_remotes(self.store)
            if not remotes:
                self._status("\u274c no remotes configured")
                self._log_event("\u274c no remotes — use :join http://...")
                return
            for remote in remotes:
                self._log_event(f"pushing to {remote.name}...")
                self._status(f"\u23f3 pushing to {remote.name}...")
                result = push_to_remote(self.store, proj_index, remote)
                if result.get("error"):
                    self._log_event(f"\u274c {result['error']}")
                    self._status(f"\u274c {result['error']}")
                    return
                cues_n = result.get("cues", 0)
                snaps_n = result.get("snapshots", 0)
                self._log_event(f"  {cues_n} cues, {snaps_n} snapshots")
                blobs = push_snapshot_blobs(self.store, proj_index, remote)
                if blobs:
                    self._log_event(f"  {blobs} blob(s)")
            ts = time.strftime("%H:%M")
            self._peer_reachable = True
            self._last_sync = f"\u2b06 push \u2713 {ts}"
            self._update_header()
            self._log_event("\u2705 push complete")
            self._status("\u2705 push complete")
        except Exception as e:
            self._log_event(f"\u274c push error: {e}")
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
            lv.mount(ListItem(label), before=0)
            # Keep max 5 log entries, remove oldest
            log_entries = [c for c in lv.children if isinstance(c, ListItem) and
                          c.children and hasattr(c.children[0], "classes") and
                          "event-log" in c.children[0].classes]
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
                if (isinstance(c, ListItem) and c.children and
                    hasattr(c.children[0], "classes") and
                    "event-log" in c.children[0].classes):
                    c.remove()
        except NoMatches:
            pass

    def _update_header(self):
        try:
            proj = f"  [white]{self.project}[/]" if self.project else ""
            cue_part = f"  [{C['dim']}]{len(self.cues)} cues[/]"
            # Last sync status
            sync_part = ""
            if self._last_sync:
                sync_part = f"  [{C['green']}]{self._last_sync}[/]"
            # Peer dot
            if self._peer_name and self._peer_reachable:
                peer = f"  [bold {C['green']}]\u25cf[/]"
            elif self._peer_name:
                peer = f"  [{C['yellow']}]\u25cb[/]"
            else:
                peer = f"  [{C['dim']}]\u25cb[/]"
            self.query_one("#header-title", Static).update(
                f"[bold {C['accent']}]~▼~ clavus[/]{proj}{cue_part}{peer}{sync_part}")
        except Exception:
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
                f"[{C['accent']}]![/] conflict  "
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
            conflict_mark = f" [{C['yellow']}]⚠[/]" if c.conflict else ""
            cue_line = (
                f"  [{color}]{dot}[/] [dim]@{c.position}[/] "
                f"[{C['fg']}]{safe_text}[/]"
                f" [{C['muted']}]{c.id[:8]}[/]{rc}{conflict_mark}"
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
