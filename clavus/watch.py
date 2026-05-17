"""
Clavus — file watcher daemon.

Auto-snapshots an Ableton project every time the .als file is modified on disk.
Uses mtime polling (cross-platform, no extra deps) with configurable debounce.

Tracks the active project from index.json and auto-reloads when you switch
projects via clavus project or the TUI.

Usage:
    clavus watch           # start daemon (foreground, for testing)
    clavus watch --once    # take one snapshot if changed and exit (for cron)
    clavus watch install   # install as system service (macOS: launchd, Linux: systemd)
    clavus watch start     # start the installed service
    clavus watch stop      # stop the service
    clavus watch restart   # stop + start
"""

from __future__ import annotations

import hashlib
import os
import sys
import time
import platform
from pathlib import Path
from typing import Optional

from clavus.store import BlobStore, ClavusProject
from clavus.parser import parse_als
from clavus.helpers import get_store_and_project
from clavus.store import diff_projects

# ─── Config ──────────────────────────────────────────────────────────────

DEFAULT_COOLDOWN = 30   # seconds to wait after last change before snapshotting
POLL_INTERVAL = 2       # seconds between file system checks
PROJECT_CHECK_INTERVAL = 10  # re-check active project every N polls
SERVICE_NAME = "com.clavus.watch"
LOG_FILE = Path.home() / ".clavus" / "watch.log"


def _auto_message() -> str:
    """Generate an auto-snapshot message."""
    return f"Auto-snapshot @ {time.strftime('%Y-%m-%d %H:%M:%S')}"


def _get_active_project() -> tuple[Optional[BlobStore], Optional[ClavusProject], Optional[str]]:
    """Re-read the index to get the currently active project.
    
    Returns (store, project, project_name) or (None, None, None) if no active project.
    """
    try:
        store = BlobStore()
        index = store.load_index()
        proj_name = index.get("_last_project")
        if not proj_name:
            return None, None, None
        proj = store.get_index(proj_name)
        if not proj:
            return None, None, None
        return store, proj, proj_name
    except Exception:
        return None, None, None


def watch(
    cooldown: int = DEFAULT_COOLDOWN,
    verbose: bool = True,
    once: bool = False,
    log_file: Optional[Path] = None,
) -> None:
    """Watch the active project's .als file for changes and auto-snapshot.

    Polls every POLL_INTERVAL seconds. Uses mtime as a cheap first-pass check.
    Debounce prevents snapshotting mid-write (Ableton takes ~1s to finish saving).

    Re-reads the active project from index.json on every poll cycle, so switching
    projects via 'clavus project' or the TUI is picked up automatically.

    Args:
        cooldown: Seconds to wait after last detected change before snapshotting
        verbose: Print snapshot info on each auto-snapshot
        once: If True, take one snapshot and return (for cron usage)
        log_file: If set, write all output to this file instead of stdout
    """
    poll_count = 0
    current_proj_name: Optional[str] = None

    def _print(msg: str) -> None:
        if verbose:
            if log_file:
                _log(log_file, msg)
            else:
                print(msg)

    _print(f"👁  Clavus watch daemon started")
    _print(f"   Cooldown: {cooldown}s  Poll: {POLL_INTERVAL}s")
    if once:
        _print(f"   Mode: one-shot")
    else:
        _print(f"   Will track whichever project is active in Clavus")
        _print(f"   Press Ctrl+C to stop")
    _print("")

    try:
        while True:
            time.sleep(POLL_INTERVAL)
            poll_count += 1

            # Periodically re-check which project is active
            # Always check on first poll, then every PROJECT_CHECK_INTERVAL cycles
            if poll_count == 1 or (poll_count % PROJECT_CHECK_INTERVAL == 0):
                store, proj, proj_name = _get_active_project()
                if proj_name != current_proj_name:
                    if proj_name:
                        _print(f"   -> Switching to project: {proj_name}")
                    current_proj_name = proj_name

            if not current_proj_name:
                # No active project — wait and keep polling
                continue

            store, proj, proj_name = _get_active_project()
            if not proj:
                # Project disappeared (maybe was deleted from index)
                current_proj_name = None
                continue

            als_path = Path(proj.root_als)

            # Check if .als file exists
            if not als_path.exists():
                # File might be moved, renamed, or project not open in Ableton
                # Keep watching — it might come back
                continue

            # Track state for this file
            try:
                current_mtime = als_path.stat().st_mtime
            except OSError:
                continue

            # Use file-specific tracking (keyed by project path)
            state_key = str(als_path)
            state = _WatchState.get(state_key)

            if current_mtime != state.last_mtime:
                # File was modified — reset cooldown
                state.last_mtime = current_mtime
                if not state.pending_snapshot:
                    state.pending_snapshot = True
                    state.pending_since = time.time()
                    remaining = cooldown
                    _print(f"   [edit]  [{proj_name}] Change detected — snapshot in {remaining:.0f}s...")
                else:
                    remaining = max(0, cooldown - (time.time() - state.pending_since))
                    if remaining > 0:
                        _print(f"   [edit]  [{proj_name}] Change detected — resetting countdown ({remaining:.0f}s)...")
                        state.pending_since = time.time()

            elif state.pending_snapshot:
                elapsed = time.time() - state.pending_since
                remaining = max(0, cooldown - elapsed)

                if remaining == 0:
                    # Cooldown expired — take snapshot
                    _print(f"   📸 [{proj_name}] Snapshotting...")
                    changed = _take_snapshot(store, proj, log_file)
                    state.pending_snapshot = False
                    state.last_snapshot_mtime = als_path.stat().st_mtime
                    state.last_snapshot_hash = changed

                    if once:
                        return

    except KeyboardInterrupt:
        _print("")
        _print("👋 Watch stopped.")


class _WatchState:
    """Per-file state for the watch daemon. Persists across polls."""

    _instances: dict[str, "_WatchState"] = {}
    last_mtime: float = 0.0
    last_snapshot_mtime: float = 0.0
    last_snapshot_hash: Optional[str] = None
    pending_snapshot: bool = False
    pending_since: float = 0.0

    def __init__(self, key: str):
        self.key = key
        self.last_mtime = 0.0
        self.last_snapshot_mtime = 0.0
        self.last_snapshot_hash = None
        self.pending_snapshot = False
        self.pending_since = 0.0

    @classmethod
    def get(cls, key: str) -> "_WatchState":
        if key not in cls._instances:
            cls._instances[key] = cls(key)
        return cls._instances[key]


def _log(log_file: Optional[Path], msg: str) -> None:
    """Write a message to the log file with timestamp."""
    if log_file:
        log_file.parent.mkdir(parents=True, exist_ok=True)
        with open(log_file, "a") as f:
            f.write(f"[{time.strftime('%Y-%m-%d %H:%M:%S')}] {msg}\n")


def _take_snapshot(
    store: BlobStore,
    proj: ClavusProject,
    log_file: Optional[Path] = None,
    verbose: bool = True,
) -> Optional[str]:
    """Parse the current .als and create a snapshot if anything changed.

    Returns the snapshot hash if one was taken, None otherwise.
    """
    als_path = Path(proj.root_als)

    # Quick check: compare raw .als hash to last snapshot
    raw_bytes = als_path.read_bytes()
    current_als_hash = hashlib.sha256(raw_bytes).hexdigest()
    prev = store.load_snapshot(proj.head) if proj.head else None

    if prev and prev.als_hash == current_als_hash:
        _log(log_file, f"   ⏭  No change detected, skipping")
        return None  # identical content

    # Parse the .als to get metadata for the snapshot message
    try:
        project = parse_als(als_path)
    except Exception as e:
        _log(log_file, f"-- Failed to parse .als: {e}")
        return None

    # Double-check after parsing (in case of transient write)
    raw_bytes_after = als_path.read_bytes()
    if hashlib.sha256(raw_bytes_after).hexdigest() != current_als_hash:
        _log(log_file, f"   ⏭  File changed during parse, skipping")
        return None

    snap = store.save_snapshot(
        project,
        message=_auto_message(),
        parent=proj.head,
    )

    # Final check: store the snapshot hash
    store.update_ref("HEAD", snap.hash)
    store.set_project_head(proj, snap.hash, source="cmd-snapshot")

    # Build summary
    if prev:
        try:
            prev_project = store.load_project(prev.hash)
            if prev_project:
                diff = diff_projects(prev_project, project)
                summary = diff.summary
            else:
                summary = f"{project.track_count} tracks @ {project.bpm}bpm"
        except Exception:
            summary = f"{project.track_count} tracks @ {project.bpm}bpm"
    else:
        summary = f"{project.track_count} tracks @ {project.bpm}bpm"

    _log(log_file, f"📸 Auto-snapshot: {snap.short_hash()} — {summary}")
    return snap.hash


def watch_once(
    store: BlobStore,
    proj: ClavusProject,
    verbose: bool = True,
) -> bool:
    """Take a single auto-snapshot if the file has changed since last snapshot.

    Returns True if a snapshot was taken.
    """
    als_path = Path(proj.root_als)
    if not als_path.exists():
        return False

    current_als_hash = hashlib.sha256(als_path.read_bytes()).hexdigest()
    prev = store.load_snapshot(proj.head) if proj.head else None
    if prev and prev.als_hash == current_als_hash:
        return False  # no change

    # Guard: skip if a snapshot already exists for this .als state.
    # Prevents recycling an old root snapshot and destroying the chain.
    existing_meta = store.objects_dir / current_als_hash[:2] / f"{current_als_hash}.meta"
    if existing_meta.exists():
        return False  # snapshot already exists for this .als content

    try:
        project = parse_als(als_path)
    except Exception:
        return False

    snap = store.save_snapshot(
        project,
        message=_auto_message(),
        parent=proj.head,
    )

    store.update_ref("HEAD", snap.hash)
    store.set_project_head(proj, snap.hash, source="auto-snapshot-watch")

    if verbose:
        print(f"📸 Auto-snapshot: {snap.short_hash()} — {project.track_count} tracks @ {project.bpm}bpm")

    return True


# ─── Service Management ─────────────────────────────────────────────────

def install_service() -> bool:
    """Install the watch daemon as a system service.

    macOS: launchd plist in ~/Library/LaunchAgents/
    Linux: systemd user service in ~/.config/systemd/user/

    Returns True on success.
    """
    system = platform.system()

    if system == "Darwin":
        return _install_launchd()
    elif system == "Linux":
        return _install_systemd()
    else:
        print(f"-- Service installation not supported on {system}.")
        print(f"   Run 'clavus watch' in a terminal multiplexer instead.")
        return False


def _launchd_plist() -> str:
    """Generate the launchd plist for macOS."""
    python_path = sys.executable

    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{SERVICE_NAME}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{python_path}</string>
        <string>-m</string>
        <string>clavus</string>
        <string>watch</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>PathState</key>
        <dict>
            <key>~/Clavus/Projects</key>
            <true/>
        </dict>
    </dict>
    <key>StandardOutPath</key>
    <string>{LOG_FILE}</string>
    <key>StandardErrorPath</key>
    <string>{LOG_FILE}</string>
</dict>
</plist>"""


def _install_launchd() -> bool:
    """Install as a macOS launchd service."""
    plist_dir = Path.home() / "Library" / "LaunchAgents"
    plist_dir.mkdir(parents=True, exist_ok=True)
    plist_path = plist_dir / f"{SERVICE_NAME}.plist"

    content = _launchd_plist()
    plist_path.write_text(content)

    print(f"[ok] Service installed: {plist_path}")
    print(f"   To start now:  launchctl load {plist_path}")
    print(f"   To start on login: already enabled (RunAtLoad)")
    print(f"   Logs: {LOG_FILE}")
    return True


def _systemd_service() -> str:
    """Generate the systemd unit file for Linux."""
    python_path = sys.executable

    return f"""[Unit]
Description=Clavus Ableton Watch Daemon
After=default.target

[Service]
Type=simple
ExecStart={python_path} -m clavus watch --quiet
Restart=on-failure
RestartSec=10
StandardOutput=append:{LOG_FILE}
StandardError=append:{LOG_FILE}
WorkingDirectory={Path.home()}

[Install]
WantedBy=default.target
"""


def _install_systemd() -> bool:
    """Install as a systemd user service on Linux."""
    service_dir = Path.home() / ".config" / "systemd" / "user"
    service_dir.mkdir(parents=True, exist_ok=True)
    service_path = service_dir / f"{SERVICE_NAME}.service"

    content = _systemd_service()
    service_path.write_text(content)

    print(f"[ok] Service installed: {service_path}")
    print(f"   To start now:  systemctl --user start {SERVICE_NAME}")
    print(f"   To start on login: systemctl --user enable {SERVICE_NAME}")
    print(f"   Logs: {LOG_FILE}")
    return True


def start_service() -> bool:
    """Start the installed watch service."""
    system = platform.system()
    if system == "Darwin":
        plist_path = Path.home() / "Library" / "LaunchAgents" / f"{SERVICE_NAME}.plist"
        if not plist_path.exists():
            print(f"-- Service not installed. Run: clavus watch install")
            return False
        import subprocess
        r = subprocess.run(["launchctl", "load", str(plist_path)], capture_output=True, text=True)
        if r.returncode == 0:
            print(f"[ok] Watch service started")
            return True
        else:
            print(f"-- Failed to start: {r.stderr}")
            return False
    elif system == "Linux":
        import subprocess
        r = subprocess.run(["systemctl", "--user", "start", SERVICE_NAME], capture_output=True, text=True)
        if r.returncode == 0:
            print(f"[ok] Watch service started")
            return True
        else:
            print(f"-- Failed to start: {r.stderr}")
            return False
    else:
        print(f"-- Not supported on {system}")
        return False


def stop_service() -> bool:
    """Stop the installed watch service."""
    system = platform.system()
    if system == "Darwin":
        plist_path = Path.home() / "Library" / "LaunchAgents" / f"{SERVICE_NAME}.plist"
        if not plist_path.exists():
            print(f"-- Service not installed. Run: clavus watch install")
            return False
        import subprocess
        r = subprocess.run(["launchctl", "unload", str(plist_path)], capture_output=True, text=True)
        if r.returncode == 0:
            print(f"[ok] Watch service stopped")
            return True
        else:
            print(f"-- Failed to stop: {r.stderr}")
            return False
    elif system == "Linux":
        import subprocess
        r = subprocess.run(["systemctl", "--user", "stop", SERVICE_NAME], capture_output=True, text=True)
        if r.returncode == 0:
            print(f"[ok] Watch service stopped")
            return True
        else:
            print(f"-- Failed to stop: {r.stderr}")
            return False
    else:
        print(f"-- Not supported on {system}")
        return False


def service_status() -> Optional[str]:
    """Check if the service is running. Returns status string or None."""
    system = platform.system()
    if system == "Darwin":
        import subprocess
        r = subprocess.run(
            ["launchctl", "list", SERVICE_NAME],
            capture_output=True, text=True,
        )
        if r.returncode == 0:
            return f"running (PID from launchctl)"
        else:
            return "not loaded"
    elif system == "Linux":
        import subprocess
        r = subprocess.run(
            ["systemctl", "--user", "is-active", SERVICE_NAME],
            capture_output=True, text=True,
        )
        if r.stdout.strip() == "active":
            return "running"
        else:
            return "not running"
    return None