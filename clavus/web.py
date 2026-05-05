"""Clavus Web Companion — FastAPI web UI for project collaboration.

REST API + dark CRUX-themed web interface for viewing projects,
managing cues, and pushing/pulling with collaborators.

Run: python3 -m uvicorn clavus.web:app --port 7890
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path
from typing import Optional

# ─── FastAPI ─────────────────────────────────────────────────────────────

from fastapi import FastAPI, HTTPException, Query, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import HTMLResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

# ─── Clavus core ─────────────────────────────────────────────────────────

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from clavus.helpers import get_store_and_project, find_als_file
from clavus.cues import CueStore, CueFilter, format_cue_list, Cue, CueReply as CueReplyData
from clavus.store import BlobStore, ClavusProject, Snapshot, diff_projects, DEFAULT_CLAVUS_DIR, StemStore
from clavus import parse_als

# ─── Helpers ────────────────────────────────────────────────────────────

import concurrent.futures

def _parse_with_timeout(als_path: Path, timeout: float = 5.0):
    """Parse an .als file with a hard timeout to prevent hanging."""
    with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
        fut = pool.submit(parse_als, als_path)
        try:
            return fut.result(timeout=timeout)
        except concurrent.futures.TimeoutError:
            return None

# ─── App setup ──────────────────────────────────────────────────────────

app = FastAPI(title="Clavus Web", version="0.2.0")

# ─── WebSocket Manager ─────────────────────────────────────────────────

class ConnectionManager:
    """Manages websocket connections per project for real-time sync."""

    def __init__(self):
        self._connections: dict[str, list[WebSocket]] = {}

    async def connect(self, websocket: WebSocket, project: str):
        await websocket.accept()
        if project not in self._connections:
            self._connections[project] = []
        self._connections[project].append(websocket)

    def disconnect(self, websocket: WebSocket, project: str):
        if project in self._connections:
            self._connections[project] = [
                w for w in self._connections[project] if w != websocket
            ]

    async def broadcast(self, project: str, event: str, data: dict):
        """Send an event to all connected peers for a project."""
        if project not in self._connections:
            return
        message = {"event": event, "data": data, "timestamp": time.time()}
        stale = []
        for ws in self._connections[project]:
            try:
                await ws.send_json(message)
            except Exception:
                stale.append(ws)
        for ws in stale:
            self.disconnect(ws, project)


manager = ConnectionManager()


@app.websocket("/ws")
async def websocket_endpoint(websocket: WebSocket, project: str = ""):
    """WebSocket endpoint for real-time sync between peers.

    Query params:
        project: Project name to scope the connection.

    Messages received:
        {"event": "ping"} — keep alive (server responds with "pong")

    Messages broadcasted:
        {"event": "cue_new", "data": {...}}
        {"event": "cue_reply", "data": {...}}
        {"event": "cue_update", "data": {...}}
    """
    if not project:
        await websocket.close(code=4000)
        return

    await manager.connect(websocket, project)
    try:
        while True:
            data = await websocket.receive_json()
            if data.get("event") == "ping":
                await websocket.send_json({"event": "pong"})
    except (WebSocketDisconnect, Exception):
        manager.disconnect(websocket, project)


async def broadcast_cue_event(project: str, event: str, cue_data: dict):
    """Broadcast a cue change to all connected peers.

    Call this after any cue mutation (create, reply, update)
    so remotes get real-time updates.
    """
    await manager.broadcast(project, event, cue_data)

# HTML template path
HERE = Path(__file__).resolve().parent
HTML_DIR = HERE / "web"
HTML_DIR.mkdir(exist_ok=True)

_HTML_CACHE: dict[str, str] = {}


def _get_project(name: str = "") -> tuple[BlobStore, ClavusProject]:
    """Get a clavus project by name, or the active one."""
    store = BlobStore()
    if name:
        proj = store.get_index(name)
        if not proj:
            raise HTTPException(status_code=404, detail=f"Project '{name}' not found")
        return store, proj
    try:
        return get_store_and_project()
    except SystemExit:
        raise HTTPException(status_code=404, detail="No clavus project found. Run 'clavus init' first.")


# ─── Models ─────────────────────────────────────────────────────────────

class CueCreate(BaseModel):
    text: str
    position: str = "0.0.0"
    track: str = ""
    author: str = "web"
    project_name: str = ""


class CueReply(BaseModel):
    text: str


class SyncCueItem(BaseModel):
    id: str
    position: str = "0.0.0"
    text: str = ""
    author: str = ""
    status: str = "pending"
    timestamp: float = 0.0
    track_name: str = ""
    snapshot_hash: str = ""
    assignee: str = ""
    in_progress: bool = False
    replies: list[dict] = []


class SyncPushBody(BaseModel):
    cues: list[SyncCueItem] = []


# ─── API Routes ─────────────────────────────────────────────────────────

@app.get("/api/ping")
async def ping():
    return {"status": "ok", "app": "clavus-web", "version": "0.2.0"}


@app.get("/api/projects")
async def list_projects():
    """List all registered projects."""
    store = BlobStore()
    projects = store.list_projects()
    return {"projects": [
        {
            "name": p.name,
            "root_als": p.root_als,
            "head": p.head[:8] if p.head else None,
            "branch": p.branch,
        }
        for p in projects
    ]}


@app.post("/api/projects/init")
async def init_project(path: str = Query(..., description="Path to .als file or directory containing one")):
    """Register a new project from a file path.

    Accepts a path to a .als file or a directory containing one.
    Parses the file, creates an initial snapshot, and registers it.
    Returns the project info on success, or an error message.
    """
    target = Path(path).resolve()
    if not target.exists():
        return JSONResponse({"error": f"Path not found: {target}"}, status_code=400)

    als_path = find_als_file(target)
    if als_path is None:
        return JSONResponse(
            {"error": f"No .als file found at {target}. Provide a path to a .als file or a directory containing one."},
            status_code=400,
        )

    store = BlobStore()
    store.init()

    existing = store.get_index(als_path.stem)
    if existing:
        return JSONResponse({
            "info": f"Project '{als_path.stem}' already tracked",
            "project": {"name": existing.name, "root_als": existing.root_als, "head": existing.head[:8] if existing.head else None},
        })

    project = _parse_with_timeout(als_path, timeout=5.0)
    if project is None:
        return JSONResponse({"error": f"Parse timed out for {als_path}"}, status_code=500)

    clavus_proj = ClavusProject(
        name=als_path.stem,
        root_als=str(als_path),
        created_at=time.time(),
    )

    snap = store.save_snapshot(project, "Initial import", parent=None)
    clavus_proj.head = snap.hash
    store.update_ref("HEAD", snap.hash)
    store.update_ref(f"refs/tags/initial", snap.hash)
    store.set_index(clavus_proj)

    return {
        "success": True,
        "project": {
            "name": clavus_proj.name,
            "root_als": str(als_path),
            "tracks": project.track_count,
            "bpm": project.bpm,
            "head": snap.short_hash(),
        },
    }


@app.get("/api/projects/browse")
async def browse_directory(dir: str = Query("", description="Directory to browse")):
    """Browse a directory for .als files and subdirectories.

    Returns both the directory listing (subdirectories + .als files)
    and any already-registered projects.
    """
    target = Path(dir).resolve() if dir else Path.home()
    if not target.exists():
        return JSONResponse({"error": f"Path not found: {target}"}, status_code=400)
    if not target.is_dir():
        return JSONResponse({"error": f"Not a directory: {target}"}, status_code=400)

    try:
        subdirs = sorted([str(p.name) for p in target.iterdir() if p.is_dir() and not p.name.startswith(".")])
    except PermissionError:
        subdirs = []

    als_files = sorted([str(p.name) for p in target.glob("*.als")])

    store = BlobStore()
    registered = store.list_projects()
    already = {p.root_als for p in registered}

    return {
        "current_dir": str(target),
        "parent_dir": str(target.parent) if str(target) != "/" else None,
        "subdirs": subdirs,
        "als_files": [{"name": f, "registered": str(target / f) in already} for f in als_files],
    }


@app.get("/api/project")
def get_project_sync(name: str = Query("", description="Project name to load")):
    """Get current project info + snapshot history."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    # Parse the .als if it exists
    als_path = Path(proj.root_als)
    project_data = None
    if als_path.exists():
        try:
            project_obj = _parse_with_timeout(als_path, timeout=5.0)
            if project_obj is None:
                project_data = {"error": "Parse timed out after 5s"}
            else:
                project_data = {
                    "ableton_version": project_obj.ableton_version,
                    "tracks": [{"name": t.name, "type": t.track_type, "color": t.color,
                                 "clips": [{"name": c.name, "start": c.start_beats, "end": c.end_beats}
                                          for c in getattr(t, "clips", [])]}
                              for t in project_obj.tracks],
                    "return_tracks": [{"name": t.name} for t in project_obj.return_tracks],
                    "bpm": project_obj.bpm,
                    "time_signature": project_obj.time_signature,
                    "markers": [{"time": m.time, "name": m.name} for m in project_obj.markers],
                    "track_count": len(project_obj.tracks),
                }
        except Exception as e:
            project_data = {"error": str(e)}

    # Snapshot history
    history = []
    current = proj.head
    seen: set[str] = set()
    while current:
        if current in seen:
            # Self-referencing parent — break and auto-repair
            store.repair_snapshot(current)
            break
        seen.add(current)
        snap = store.load_snapshot(current)
        if not snap:
            break
        # Auto-repair if parent points to self
        if snap.parent == current:
            store.repair_snapshot(current)
            snap.parent = None
        history.append({
            "hash": snap.hash[:8],
            "full_hash": snap.hash,
            "timestamp": snap.timestamp,
            "time_str": time.strftime("%Y-%m-%d %H:%M", time.localtime(snap.timestamp)),
            "message": snap.message,
            "track_count": snap.track_count,
            "bpm": snap.bpm,
            "is_head": current == store.read_ref("HEAD"),
        })
        if snap.parent == current:
            # Self-referencing parent — stop here
            break
        current = snap.parent

    return {
        "name": proj.name,
        "root_als": str(proj.root_als),
        "branch": proj.branch,
        "project": project_data,
        "history": history,
        "head": proj.head[:8] if proj.head else None,
    }


@app.get("/api/snapshots/{snap_hash}")
def get_snapshot_detail(snap_hash: str, name: str = Query("", description="Project name")):
    """Get full snapshot data including parsed project for visual diff."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    # Resolve short hash by walking the history chain
    head = store.read_ref("HEAD")
    resolved = _resolve_hash(store, proj, snap_hash, head)
    if not resolved:
        return JSONResponse({"error": f"Snapshot '{snap_hash}' not found"}, status_code=404)

    snap = store.load_snapshot(resolved)
    if not snap:
        return JSONResponse({"error": f"Failed to load snapshot '{snap_hash}'"}, status_code=404)

    # Parse project data from the .als
    project_data = None
    try:
        proj_obj = parse_als(Path(proj.root_als))
        if proj_obj:
            project_data = {
                "ableton_version": proj_obj.ableton_version,
                "tracks": [{"name": t.name, "type": t.track_type, "color": t.color,
                            "clips": [{"name": c.name, "start": c.start_beats, "end": c.end_beats}
                                     for c in getattr(t, "clips", [])]}
                           for t in proj_obj.tracks],
                "return_tracks": [{"name": t.name} for t in proj_obj.return_tracks],
                "bpm": proj_obj.bpm,
                "time_signature": proj_obj.time_signature,
                "markers": [{"time": m.time, "name": m.name} for m in proj_obj.markers],
                "track_count": len(proj_obj.tracks),
            }
    except Exception:
        project_data = None

    return {
        "hash": snap.hash[:8],
        "full_hash": snap.hash,
        "timestamp": snap.timestamp,
        "time_str": time.strftime("%Y-%m-%d %H:%M", time.localtime(snap.timestamp)),
        "message": snap.message,
        "track_count": snap.track_count,
        "bpm": snap.bpm,
        "parent": snap.parent[:8] if snap.parent else None,
        "is_head": snap.hash == head,
        "project": project_data,
    }


def _resolve_hash(store: BlobStore, proj: ClavusProject, short_hash: str, head: str | None) -> str | None:
    """Walk the snapshot chain to resolve a short hash to a full hash."""
    if head is None:
        return None
    current = head
    seen: set[str] = set()
    while current:
        if current in seen:
            return None
        seen.add(current)
        if current.startswith(short_hash):
            return current
        snap = store.load_snapshot(current)
        if not snap or snap.parent == current:
            return None
        current = snap.parent
    return None


@app.get("/api/projects/compare")
def compare_snapshots(
    before: str = Query(..., description="Before snapshot hash"),
    after: str = Query(..., description="After snapshot hash"),
    name: str = Query("", description="Project name"),
):
    """Compare two snapshots and return visual diff HTML."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    # Resolve short hashes by walking the chain
    head = store.read_ref("HEAD")
    before_full = _resolve_hash(store, proj, before, head)
    after_full = _resolve_hash(store, proj, after, head)
    if not before_full:
        return JSONResponse({"error": f"Snapshot '{before}' not found"}, status_code=404)
    if not after_full:
        return JSONResponse({"error": f"Snapshot '{after}' not found"}, status_code=404)

    snap_before = store.load_snapshot(before_full)
    snap_after = store.load_snapshot(after_full)
    if not snap_before or not snap_after:
        return JSONResponse({"error": "Failed to load one or both snapshots"}, status_code=404)

    # Build diff from stored project data
    from clavus.visual_diff import render_diff_html

    proj_before = store.load_project(before_full)
    proj_after = store.load_project(after_full)
    if not proj_before or not proj_after:
        return JSONResponse({"error": "Failed to load project data for one or both snapshots"}, status_code=404)

    diff = diff_projects(proj_before, proj_after)

    html = render_diff_html(diff, before_proj=proj_before, after_proj=proj_after)

    return HTMLResponse(html)


@app.get("/api/cues")
async def get_cues(pending_only: bool = False, name: str = Query("", description="Project name")):
    """List all cues."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    filter_ = CueFilter()
    if pending_only:
        filter_.status = "pending"
    all_cues = cues_store.list_cues(filter_)

    cues_data = []
    for c in all_cues:
        cues_data.append({
            "id": c.id,
            "text": c.text,
            "position": c.position,
            "author": c.author,
            "track_name": c.track_name,
            "status": c.status,
            "assignee": c.assignee,
            "in_progress": c.in_progress,
            "timestamp": c.timestamp,
            "time_str": time.strftime("%m/%d %H:%M", time.localtime(c.timestamp)),
            "replies": [
                {"author": r.author, "text": r.text, "timestamp": r.timestamp}
                for r in (c.replies or [])
            ],
        })

    return {
        "total": len(cues_data),
        "cues": cues_data,
    }


@app.post("/api/cues")
async def create_cue(cue: CueCreate):
    """Add a new cue."""
    try:
        store, proj = _get_project(cue.project_name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    head = store.read_ref("HEAD")

    new_cue = cues_store.add_cue(
        text=cue.text,
        position=cue.position,
        author=cue.author or os.environ.get("USER", "anonymous"),
        snapshot_hash=head or "",
        track_name=cue.track,
    )

    # Broadcast to connected peers
    await broadcast_cue_event(proj.name, "cue_new", {
        "id": new_cue.id, "text": new_cue.text,
        "position": new_cue.position, "author": new_cue.author,
        "status": new_cue.status, "timestamp": new_cue.timestamp,
        "track_name": new_cue.track_name,
        "assignee": new_cue.assignee, "in_progress": new_cue.in_progress,
    })

    return {
        "id": new_cue.id,
        "text": new_cue.text,
        "position": new_cue.position,
        "status": "created",
    }


@app.post("/api/projects/inject")
async def inject_cues(name: str = Query("", description="Project name to inject cues into")):
    """Inject unresolved cues as Ableton markers into the project's .als file."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    als_path = Path(proj.root_als)
    if not als_path.exists():
        return JSONResponse({"error": f".als file not found: {als_path}"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    unresolved = cues_store.list_cues(CueFilter(status="pending"))
    if not unresolved:
        return {"injected": 0, "message": "No unresolved cues to inject"}

    from clavus.cues import render_cues_as_markers
    result = render_cues_as_markers(unresolved, "", inject_into_als=str(als_path))
    if not result:
        return {"injected": 0, "message": "All cues already present in the project"}
    return {"injected": len(unresolved), "message": f"Injected {len(unresolved)} cue(s) as markers"}


@app.post("/api/projects/restore")
async def restore_snapshot_endpoint(
    hash: str = Query("", description="Snapshot hash to restore (default: HEAD)"),
    name: str = Query("", description="Project name"),
):
    """Restore a project's .als file from a snapshot's raw backup."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    als_path = Path(proj.root_als)
    if not als_path.exists():
        return JSONResponse({"error": f".als file not found: {als_path}"}, status_code=404)

    hash_str = hash or store.read_ref("HEAD")
    if not hash_str:
        return JSONResponse({"error": "No snapshots to restore from"}, status_code=404)

    # Resolve hash prefix
    if len(hash_str) < 64:
        # Short hash — try to find it
        prefix = hash_str
        hash_str = store.read_ref(f"refs/tags/{prefix}")
        if not hash_str:
            # Search by prefix in objects
            for obj_dir in store.objects_dir.iterdir():
                if obj_dir.is_dir():
                    for f in obj_dir.iterdir():
                        if f.name.endswith(".meta") and f.stem.startswith(prefix):
                            hash_str = f.stem
                            break
                    if hash_str and len(hash_str) >= 8:
                        break

    snap = store.load_snapshot(hash_str) if hash_str else None
    if not snap:
        return JSONResponse({"error": f"Snapshot not found: {hash}"}, status_code=404)

    if not snap.als_hash:
        return JSONResponse({
            "error": "Snapshot has no raw .als backup",
            "detail": "Only snapshots created after the restore feature was built store raw .als data. Create a fresh snapshot first.",
        }, status_code=400)

    raw_als = store.get_object(snap.als_hash)
    if not raw_als:
        return JSONResponse({"error": "Raw .als data missing from blob store"}, status_code=404)

    # Backup existing .als (only first time)
    bak_path = als_path.with_suffix(".als.bak")
    if not bak_path.exists():
        bak_path.write_bytes(als_path.read_bytes())

    # Write the restored .als
    als_path.write_bytes(raw_als)

    # Update HEAD
    store.update_ref("HEAD", hash_str)
    proj.head = hash_str
    store.set_index(proj)

    snap_time = time.strftime("%Y-%m-%d %H:%M", time.localtime(snap.timestamp))
    return {
        "status": "ok",
        "hash": snap.short_hash(),
        "message": snap.message,
        "captured": snap_time,
        "tracks": snap.track_count,
        "bpm": snap.bpm,
        "backup": str(bak_path) if bak_path.exists() else "",
    }


class SnapshotCreate(BaseModel):
    message: str
    tags: str = ""


@app.post("/api/projects/snapshot")
async def create_snapshot_endpoint(
    body: SnapshotCreate,
    name: str = Query("", description="Project name"),
):
    """Create a new snapshot of the current project state."""
    from clavus import parse_als

    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    als_path = Path(proj.root_als)
    if not als_path.exists():
        return JSONResponse({"error": f".als file not found: {als_path}"}, status_code=404)

    project = parse_als(als_path)
    snap = store.save_snapshot(
        project,
        message=body.message,
        parent=proj.head,
        tags=body.tags.split(",") if body.tags else [],
    )

    # Check if anything actually changed
    if snap.hash == proj.head and proj.head is not None:
        return {
            "status": "no_change",
            "hash": snap.short_hash(),
            "message": "No changes detected — project state is identical to last snapshot.",
        }

    # Update references
    store.update_ref("HEAD", snap.hash)
    proj.head = snap.hash
    store.set_index(proj)

    return {
        "status": "ok",
        "hash": snap.short_hash(),
        "message": body.message,
        "tracks": snap.track_count,
        "bpm": snap.bpm,
    }


@app.post("/api/cues/{cue_id}/reply")
async def reply_to_cue(cue_id: str, reply: CueReply,
                       name: str = Query("", description="Project name")):
    """Reply to a cue."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    head = store.read_ref("HEAD")
    result = cues_store.reply(cue_id, reply.text, snapshot_hash=head or "")
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    # Broadcast to connected peers
    await broadcast_cue_event(proj.name, "cue_reply", {
        "cue_id": cue_id, "reply": reply.text,
        "timestamp": time.time(),
    })

    return {"status": "ok", "replies": 0}


@app.post("/api/cues/{cue_id}/resolve")
async def resolve_cue(cue_id: str, name: str = Query("", description="Project name")):
    """Resolve a cue."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    result = cues_store.resolve(cue_id)
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    # Broadcast to connected peers
    await broadcast_cue_event(proj.name, "cue_update", {
        "cue_id": cue_id, "status": "resolved",
    })

    return {"status": "resolved"}


@app.post("/api/cues/{cue_id}/skip")
async def skip_cue(cue_id: str, name: str = Query("", description="Project name")):
    """Skip a cue."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    result = cues_store.skip(cue_id)
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    # Broadcast to connected peers
    await broadcast_cue_event(proj.name, "cue_update", {
        "cue_id": cue_id, "status": "skipped",
    })

    return {"status": "skipped"}


@app.post("/api/cues/{cue_id}/assign")
async def assign_cue(cue_id: str, name: str = Query("", description="Person to assign"),
                     project: str = Query("", description="Project name")):
    """Assign a cue to someone."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    if not name:
        return JSONResponse({"error": "Name required"}, status_code=400)

    cues_store = CueStore(proj.name, store=store)
    result = cues_store.assign(cue_id, name)
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    await broadcast_cue_event(proj.name, "cue_update", {
        "cue_id": cue_id, "assignee": name, "in_progress": False,
    })
    return {"status": "assigned", "assignee": name}


@app.post("/api/cues/{cue_id}/unassign")
async def unassign_cue(cue_id: str, project: str = Query("", description="Project name")):
    """Remove assignee from a cue."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    result = cues_store.unassign(cue_id)
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    await broadcast_cue_event(proj.name, "cue_update", {
        "cue_id": cue_id, "assignee": "", "in_progress": False,
    })
    return {"status": "unassigned"}


@app.post("/api/cues/{cue_id}/start")
async def start_cue(cue_id: str, project: str = Query("", description="Project name")):
    """Mark a cue as in-progress."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    result = cues_store.start(cue_id)
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    await broadcast_cue_event(proj.name, "cue_update", {
        "cue_id": cue_id, "in_progress": True,
    })
    return {"status": "in_progress"}


@app.post("/api/cues/{cue_id}/stop")
async def stop_cue(cue_id: str, project: str = Query("", description="Project name")):
    """Mark a cue as no longer in-progress."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    result = cues_store.stop(cue_id)
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    await broadcast_cue_event(proj.name, "cue_update", {
        "cue_id": cue_id, "in_progress": False,
    })
    return {"status": "stopped"}


@app.delete("/api/cues/{cue_id}")
async def delete_cue(cue_id: str, project: str = Query("", description="Project name")):
    """Permanently delete a cue."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    if not cues_store.delete(cue_id):
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    await broadcast_cue_event(proj.name, "cue_delete", {
        "cue_id": cue_id,
    })
    return {"status": "deleted"}


@app.post("/api/cues/{cue_id}/archive")
async def archive_cue(cue_id: str, project: str = Query("", description="Project name")):
    """Archive a specific cue (move to archive/ subdirectory)."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    dst = cues_store.archive(cue_id)
    if not dst:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")

    return {"status": "archived", "path": str(dst)}


@app.get("/api/cues/archived")
async def get_archived_cues(project: str = Query("", description="Project name")):
    """List archived cues (in archive/ subdirectory)."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    archive_dir = store.root / "cues" / proj.name / "archive"
    if not archive_dir.exists():
        return {"total": 0, "cues": []}

    archived = []
    for f in sorted(archive_dir.glob("*.json")):
        try:
            data = json.loads(f.read_text())
            archived.append({
                "id": data.get("id", f.stem),
                "text": data.get("text", ""),
                "position": data.get("position", ""),
                "status": data.get("status", "archived"),
                "time_str": time.strftime("%m/%d %H:%M", time.localtime(data.get("timestamp", 0))),
            })
        except (json.JSONDecodeError, OSError):
            continue

    return {"total": len(archived), "cues": archived}


# ─── Stem Endpoints ───────────────────────────────────────────────────


@app.get("/api/stems/{project}/manifest/{snapshot_hash}")
async def get_stem_manifest(project: str, snapshot_hash: str):
    """Get the stem manifest for a given snapshot."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": f"Project '{project}' not found"}, status_code=404)

    stem_store = StemStore(proj.name, store)
    manifest = stem_store.get_manifest(snapshot_hash)
    if not manifest:
        return JSONResponse({"stems": [], "snapshot_hash": snapshot_hash})

    return {
        "snapshot_hash": manifest.snapshot_hash,
        "stems": [{
            "track_name": s.track_name,
            "file_name": s.file_name,
            "hash": s.hash,
            "size": s.size,
            "format": s.format,
            "sample_rate": s.sample_rate,
            "bit_depth": s.bit_depth,
            "channels": s.channels,
            "duration_seconds": s.duration_seconds,
        } for s in manifest.stems],
    }


@app.get("/api/stems/blob/{stem_hash}")
async def get_stem_blob(stem_hash: str):
    """Download a stem blob by content hash."""
    store = BlobStore()
    data = store.get_object(stem_hash)
    if not data:
        return JSONResponse({"error": "Stem blob not found"}, status_code=404)

    # Determine content type from magic bytes
    content_type = "application/octet-stream"
    if data[:4] == b"RIFF":
        content_type = "audio/wav"
    elif data[:4] == b"fLaC":
        content_type = "audio/flac"
    elif data[:3] == b"ID3" or data[:2] in (b"\xff\xfb", b"\xff\xf3", b"\xff\xf2"):
        content_type = "audio/mpeg"

    from fastapi.responses import Response
    return Response(
        content=data,
        media_type=content_type,
        headers={
            "Content-Disposition": f"attachment; filename={stem_hash[:12]}.wav",
            "Content-Length": str(len(data)),
        },
    )


@app.post("/api/stems/check")
async def check_stems(body: dict):
    """Given a list of stem hashes, return which ones are missing locally.
    Used by remote sync to determine which stems need to be transferred."""
    store = BlobStore()
    hashes = body.get("hashes", [])
    missing = [h for h in hashes if not store.has_object(h)]
    return {"missing": missing}


@app.post("/api/stems/{project}/manifest/{snapshot_hash}")
async def receive_stem_manifest(project: str, snapshot_hash: str, body: dict):
    """Receive a stem manifest pushed from a remote peer."""
    try:
        store, proj = _get_project(project)
    except HTTPException:
        return JSONResponse({"error": f"Project '{project}' not found"}, status_code=404)

    stem_store = StemStore(proj.name, store)
    stems_data = body.get("stems", [])

    from clavus.store import StemManifest, StemEntry
    manifest = StemManifest(snapshot_hash=snapshot_hash, created_at=time.time())
    for s in stems_data:
        manifest.stems.append(StemEntry(
            track_name=s.get("track_name", ""),
            file_name=s.get("file_name", ""),
            hash=s.get("hash", ""),
            size=s.get("size", 0),
            format=s.get("format", "wav"),
            sample_rate=s.get("sample_rate", 44100),
            bit_depth=s.get("bit_depth", 24),
            channels=s.get("channels", 2),
            duration_seconds=s.get("duration_seconds", 0),
        ))
    stem_store.save_manifest(manifest)
    return {"status": "ok", "stems": len(manifest.stems)}


@app.post("/api/stems/blob/{stem_hash}")
async def receive_stem_blob(stem_hash: str, request: Request):
    """Receive a stem blob uploaded from a remote peer."""
    store = BlobStore()
    body = await request.body()
    store.put_object(body, stem_hash)
    return {"status": "stored", "hash": stem_hash[:12]}


# ─── Sync Endpoints ──────────────────────────────────────────────────────


@app.get("/api/sync/pull")
async def sync_pull(name: str = Query(..., description="Project name")):
    """Pull all cues and snapshot history for a project."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "Project not found"}, status_code=404)

    # Cues
    cues_store = CueStore(proj.name, store=store)
    all_cues = cues_store.list_cues(CueFilter())
    cues_data = [{
        "id": c.id, "position": c.position, "text": c.text,
        "author": c.author, "status": c.status, "timestamp": c.timestamp,
        "track_name": c.track_name,
        "replies": [{"id": r.id, "author": r.author, "text": r.text,
                     "timestamp": r.timestamp, "snapshot_hash": r.snapshot_hash}
                   for r in (c.replies or [])],
    } for c in all_cues]

    # Snapshot history
    history = []
    current = proj.head
    seen: set[str] = set()
    while current:
        if current in seen:
            store.repair_snapshot(current)
            break
        seen.add(current)
        snap = store.load_snapshot(current)
        if not snap:
            break
        if snap.parent == current:
            store.repair_snapshot(current)
            snap.parent = None
        history.append({
            "hash": snap.hash[:8], "full_hash": snap.hash,
            "timestamp": snap.timestamp, "message": snap.message,
            "track_count": snap.track_count, "bpm": snap.bpm,
            "is_head": current == store.read_ref("HEAD"),
        })
        if snap.parent == current:
            break
        current = snap.parent

    return {
        "project": {"name": proj.name, "head": proj.head[:8] if proj.head else None, "branch": proj.branch},
        "cues": cues_data,
        "snapshots": history,
        "timestamp": time.time(),
    }


@app.post("/api/sync/push")
async def sync_push(body: SyncPushBody, name: str = Query(..., description="Project name")):
    """Push (merge) cues into a project using last-write-wins."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "Project not found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    merged = 0
    skipped = 0

    for item in body.cues:
        cue = Cue(
            id=item.id, position=item.position, text=item.text,
            author=item.author, status=item.status, timestamp=item.timestamp,
            track_name=item.track_name, snapshot_hash=item.snapshot_hash,
            assignee=item.assignee, in_progress=item.in_progress,
        )
        cues_store.import_cue(cue)
        merged += 1

        # Import replies
        for r in item.replies:
            reply = CueReplyData(
                id=r.get("id", ""),
                text=r.get("text", ""),
                author=r.get("author", ""),
                timestamp=r.get("timestamp", 0.0),
                snapshot_hash=r.get("snapshot_hash", ""),
            )
            if cues_store.import_reply(item.id, reply):
                merged += 1
            else:
                skipped += 1

    return {"status": "ok", "merged": merged, "skipped": skipped}


class SyncPushSnapshotsBody(BaseModel):
    snapshots: list[dict] = []


@app.post("/api/sync/push-snapshots")
async def sync_push_snapshots(body: SyncPushSnapshotsBody,
                               name: str = Query(..., description="Project name")):
    """Push (import) snapshots from a remote peer."""
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return JSONResponse({"error": "Project not found"}, status_code=404)

    imported = 0
    for s in body.snapshots:
        snap_hash = s.get("full_hash", s.get("hash", ""))
        if not snap_hash:
            continue

        # Store snapshot metadata
        meta_dir = store.objects_dir / snap_hash[:2]
        meta_dir.mkdir(parents=True, exist_ok=True)
        meta_path = meta_dir / f"{snap_hash}.meta"

        # Skip if we already have this snapshot
        if meta_path.exists():
            continue

        from dataclasses import asdict
        snap = Snapshot(
            hash=snap_hash,
            timestamp=s.get("timestamp", 0.0),
            message=s.get("message", ""),
            parent=s.get("parent", None),
            project_path=s.get("project_path", ""),
            track_count=s.get("track_count", 0),
            bpm=s.get("bpm", 120.0),
            tags=s.get("tags", []),
        )
        meta_path.write_text(json.dumps(asdict(snap), indent=2, default=str))
        imported += 1

    # Update HEAD if we got new snapshots and remote has a later HEAD
    if imported > 0:
        proj.head = store.read_ref("HEAD") or proj.head
        store.set_index(proj)

    return {"status": "ok", "imported": imported}


# ─── Web UI: Main page ──────────────────────────────────────────────────

@app.get("/api/m4l/status")
async def m4l_status(name: str = Query("", description="Project name")):
    """Lightweight status endpoint for M4L device.
    
    Returns only what the Max for Live device needs:
    - Server alive check
    - Pending cue count
    - Last snapshot info
    
    This keeps the M4L HTTP payloads tiny.
    """
    try:
        store, proj = _get_project(name)
    except HTTPException:
        return {"alive": False, "project": None}
    
    # Count pending cues
    cues_store = CueStore(proj.name, store=store)
    pending = cues_store.list_cues(CueFilter(status="pending"))
    
    # Last snapshot info
    head = proj.head
    snap_info = None
    if head:
        snap = store.load_snapshot(head)
        if snap:
            snap_info = {
                "hash": snap.short_hash(),
                "message": snap.message,
                "timestamp": snap.timestamp,
            }
    
    return {
        "alive": True,
        "project": proj.name,
        "pending_cues": len(pending),
        "head": snap_info,
        "user": os.environ.get("USER", "unknown"),
    }


@app.post("/api/m4l/cue")
async def m4l_add_cue(body: CueCreate):
    """Minimal cue creation endpoint for M4L device.
    
    Same as POST /api/cues but returns only the fields
    the M4L device cares about — tiny response body.
    """
    try:
        store, proj = _get_project(body.project_name)
    except HTTPException:
        return JSONResponse({"error": "Project not found"}, status_code=404)
    
    cues_store = CueStore(proj.name, store=store)
    head = store.read_ref("HEAD")
    
    new_cue = cues_store.add_cue(
        text=body.text,
        position=body.position or "0.0.0",
        author=body.author or os.environ.get("USER", "Live"),
        snapshot_hash=head or "",
        track_name=body.track,
    )
    
    return {
        "id": new_cue.id,
        "position": new_cue.position,
        "text": new_cue.text,
        "status": "created",
    }


def _read_template(name: str) -> str:
    """Read and cache an HTML template."""
    if name not in _HTML_CACHE:
        path = HTML_DIR / name
        if path.exists():
            _HTML_CACHE[name] = path.read_text()
        else:
            _HTML_CACHE[name] = ""
    return _HTML_CACHE[name]


@app.get("/", response_class=HTMLResponse)
async def index():
    """Serve the main Clavus dashboard."""
    html = _read_template("index.html")
    if not html:
        html = _generate_index_html()
        _write_template("index.html", html)
    return HTMLResponse(html)


@app.get("/app.js", response_class=HTMLResponse)
async def app_js():
    """Serve the JavaScript client."""
    js = _read_template("app.js")
    if not js:
        js = _generate_app_js()
        _write_template("app.js", js)
    return HTMLResponse(js, media_type="application/javascript")


@app.get("/app.css", response_class=HTMLResponse)
async def app_css():
    """Serve the CSS stylesheet."""
    css = _read_template("app.css")
    if not css:
        css = _generate_app_css()
        _write_template("app.css", css)
    return HTMLResponse(css, media_type="text/css")


# ─── Template generators ────────────────────────────────────────────────

def _generate_index_html() -> str:
    return """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no">
<meta name="apple-mobile-web-app-capable" content="yes">
<title>clavus</title>
<link rel="stylesheet" href="/app.css">
</head>
<body>
  <header>
    <div class="header-left">
      <span class="logo-icon">⧩</span>
      <span class="logo-text">clavus</span>
      <span class="conn-dot" id="connDot" title="connecting..."></span>
    </div>
    <div class="header-center">
      <select class="project-switcher" id="projectSwitcher" onchange="switchProject(this.value)">
        <option value="">Select project…</option>
      </select>
    </div>
    <div class="header-right">
      <button id="refreshBtn" onclick="loadAll()" title="Refresh">↻</button>
    </div>
  </header>
  <div class="lan-url" id="lanUrl"></div>

  <main id="mainContent">
    <!-- TAB BAR -->
    <nav class="tab-bar" id="tabBar">
      <button class="tab-btn active" data-tab="project" onclick="switchTab('project')">Project</button>
      <button class="tab-btn" data-tab="cues" onclick="switchTab('cues')">Cues</button>
      <button class="tab-btn" data-tab="snapshots" onclick="switchTab('snapshots')">Snapshots</button>
    </nav>

    <!-- PROJECT TAB -->
    <section class="tab-content active" id="tab-project">
      <div class="pane-header">
        <span class="pane-title">Tracks</span>
        <span class="pane-badge" id="trackCount">—</span>
      </div>
      <div class="project-info">
        <div class="info-chip"><span class="chip-label">BPM</span><span class="chip-value" id="bpm">—</span></div>
        <div class="info-chip"><span class="chip-label">Time</span><span class="chip-value" id="timeSig">—</span></div>
        <div class="info-chip"><span class="chip-label">Live</span><span class="chip-value" id="abletonVer">—</span></div>
      </div>
      <div class="track-list" id="trackList">
        <div class="empty-state">No tracks loaded</div>
      </div>
    </section>

    <!-- CUES TAB -->
    <section class="tab-content" id="tab-cues">
      <div class="pane-header">
        <span class="pane-title">Cues</span>
        <span class="pane-badge" id="cueCount">0</span>
      </div>
      <div class="cue-composer" id="cueComposer">
        <div class="cue-composer-row">
          <input type="text" class="cue-text-input" id="cueText" placeholder="Add a cue..." autocomplete="off">
          <button class="cue-send-btn" id="cueSendBtn" onclick="postCue()">+</button>
        </div>
        <div class="cue-composer-row cue-filter-row">
          <input type="text" class="cue-position-input" id="cuePosition" placeholder="@1:23" value="0.0.0">
          <div class="filter-chips">
            <button class="filter-chip active" data-filter="all" onclick="setFilter('all')">All</button>
            <button class="filter-chip" data-filter="pending" onclick="setFilter('pending')">Open</button>
            <button class="filter-chip" data-filter="archived" onclick="setFilter('archived')">Archived</button>
          </div>
        </div>
      </div>
      <div class="cue-list" id="cueList">
        <div class="empty-state">Loading cues...</div>
      </div>
    </section>

    <!-- SNAPSHOTS TAB -->
    <section class="tab-content" id="tab-snapshots">
      <div class="pane-header">
        <span class="pane-title">History</span>
        <span class="pane-badge" id="snapshotCount">0</span>
      </div>
      <div class="compare-bar" id="compareBar">
        <span class="compare-info" id="compareInfo">Select two to compare</span>
        <button onclick="clearCompare()">✕</button>
      </div>
      <div class="snapshot-list" id="snapshotList">
        <div class="empty-state loading">Loading history...</div>
      </div>
      <div class="diff-panel" id="snapshotDiffPanel">
        <div class="diff-header">
          <span id="diffTitle">Diff</span>
          <button onclick="hideDiff()">✕</button>
        </div>
        <div class="diff-scroll" id="diffContent"></div>
      </div>
    </section>
  </main>

  <footer>
    <span class="footer-left">clavus v<span id="version">0.2</span></span>
    <span class="footer-right">
      <button class="inject-btn" onclick="injectCues()" title="Inject cues as markers">📌</button>
    </span>
  </footer>

  <script src="/app.js"></script>
</body>
</html>"""


def _generate_app_css() -> str:
    return """:root {
  --bg: #0b1418;
  --bg2: #0f1a20;
  --bg3: #15242b;
  --hover: #1a2d36;
  --accent: #1a9e9e;
  --accent-dim: #0f6b6b;
  --fg: #b8c8c8;
  --fg2: #6a8a8a;
  --fg3: #3a5a65;
  --border: #1a3040;
  --danger: #d45a5a;
  --success: #4a9e6a;
  --warning: #d4a04a;
  --font: -apple-system, 'SF Mono', 'Cascadia Code', 'JetBrains Mono', monospace;
}

* { margin: 0; padding: 0; box-sizing: border-box; }
html, body {
  height: 100%;
  background: var(--bg);
  color: var(--fg);
  font-family: var(--font);
  font-size: 14px;
  -webkit-text-size-adjust: 100%;
  overflow-x: hidden;
}

/* ── Header ── */
header {
  display: flex;
  align-items: center;
  padding: 8px 12px;
  background: var(--bg2);
  border-bottom: 1px solid var(--border);
  gap: 8px;
  position: sticky;
  top: 0;
  z-index: 100;
}
.header-left { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
.header-center { flex: 1; min-width: 0; }
.header-right { flex-shrink: 0; }
.logo-icon { color: var(--accent); font-size: 18px; }
.logo-text { color: var(--accent); font-weight: bold; font-size: 14px; }
.conn-dot {
  width: 8px; height: 8px; border-radius: 50%;
  display: inline-block;
  background: var(--fg3);
}
.conn-dot.connected { background: var(--success); }
.conn-dot.error { background: var(--danger); }

.project-switcher {
  width: 100%;
  background: var(--bg3);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--fg2);
  font-family: var(--font);
  font-size: 13px;
  padding: 6px 8px;
  cursor: pointer;
  -webkit-appearance: none;
  appearance: none;
}
.project-switcher option { background: var(--bg); color: var(--fg); }
#refreshBtn {
  background: var(--bg3);
  border: 1px solid var(--border);
  color: var(--fg2);
  width: 34px; height: 34px;
  border-radius: 50%;
  cursor: pointer;
  font-size: 18px;
  line-height: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* ── LAN URL ── */
.lan-url {
  text-align: center;
  padding: 6px 12px;
  font-size: 12px;
  color: var(--accent);
  background: var(--bg2);
  border-bottom: 1px solid var(--border);
  font-family: var(--font);
  display: none;
}
.lan-url code { color: var(--accent); font-size: 13px; }

/* ── Tab Bar ── */
.tab-bar {
  display: flex;
  background: var(--bg2);
  border-bottom: 1px solid var(--border);
  position: sticky;
  top: 49px;
  z-index: 99;
}
.tab-btn {
  flex: 1;
  padding: 10px 8px;
  background: none;
  border: none;
  border-bottom: 2px solid transparent;
  color: var(--fg3);
  font-family: var(--font);
  font-size: 13px;
  font-weight: 600;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
  touch-action: manipulation;
}
.tab-btn.active {
  color: var(--accent);
  border-bottom-color: var(--accent);
}

/* ── Tab Content ── */
.tab-content { display: none; }
.tab-content.active { display: block; }

/* ── Pane Header ── */
.pane-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg);
}
.pane-title {
  font-size: 12px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 1px;
  color: var(--fg2);
}
.pane-badge {
  font-size: 11px;
  color: var(--accent);
  background: var(--bg3);
  padding: 2px 10px;
  border-radius: 10px;
}

/* ── Project Info Chips ── */
.project-info {
  padding: 8px 12px;
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.info-chip {
  background: var(--bg3);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 12px;
  display: flex;
  flex-direction: column;
  align-items: center;
  min-width: 64px;
}
.chip-label { font-size: 9px; color: var(--fg3); text-transform: uppercase; letter-spacing: 0.5px; }
.chip-value { font-size: 16px; font-weight: 700; }

/* ── Track List ── */
.track-list { padding: 4px 0; }
.track-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border-left: 3px solid transparent;
  margin: 1px 0;
}
.track-item:active { background: var(--hover); }
.track-dot { width: 10px; height: 10px; border-radius: 3px; flex-shrink: 0; }
.track-name { flex: 1; font-weight: 500; font-size: 14px; }
.track-type { font-size: 11px; color: var(--fg3); }
.track-clip-count {
  font-size: 11px; color: var(--accent-dim);
  background: var(--bg3); padding: 2px 8px;
  border-radius: 8px; white-space: nowrap;
}

/* ── Cue Composer ── */
.cue-composer {
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.cue-composer-row { display: flex; gap: 6px; align-items: center; }
.cue-text-input {
  flex: 1;
  background: var(--bg3);
  border: 1px solid var(--border);
  border-radius: 8px;
  color: var(--fg);
  padding: 10px 12px;
  font-family: var(--font);
  font-size: 14px;
}
.cue-text-input:focus { outline: none; border-color: var(--accent-dim); }
.cue-text-input::placeholder { color: var(--fg3); }
.cue-send-btn {
  background: var(--accent);
  border: none;
  color: var(--bg);
  font-weight: bold;
  font-size: 22px;
  width: 42px; height: 42px;
  border-radius: 50%;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}
.cue-send-btn:active { background: var(--accent-dim); }
.cue-position-input {
  width: 80px;
  background: var(--bg3);
  border: 1px solid var(--border);
  border-radius: 8px;
  color: var(--fg);
  padding: 8px 10px;
  font-family: var(--font);
  font-size: 13px;
  text-align: center;
}
.cue-position-input:focus { outline: none; border-color: var(--accent-dim); }

/* ── Filter Chips ── */
.filter-chips { display: flex; gap: 4px; }
.cue-filter-row { justify-content: space-between; }
.filter-chip {
  background: none;
  border: 1px solid var(--border);
  color: var(--fg3);
  padding: 4px 12px;
  border-radius: 14px;
  cursor: pointer;
  font-family: var(--font);
  font-size: 12px;
  -webkit-tap-highlight-color: transparent;
  touch-action: manipulation;
}
.filter-chip:active { background: var(--bg3); }
.filter-chip.active {
  background: var(--accent-dim);
  border-color: var(--accent-dim);
  color: var(--fg);
}

/* ── Cue List ── */
.cue-list { }
.cue-card {
  margin: 6px 12px;
  padding: 10px 12px;
  background: var(--bg2);
  border: 1px solid var(--border);
  border-radius: 10px;
  border-left: 3px solid var(--accent-dim);
}
.cue-card.status-resolved { border-left-color: var(--success); opacity: 0.65; }
.cue-card.status-skipped { border-left-color: var(--fg3); opacity: 0.55; }
.cue-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 6px;
}
.cue-position { font-size: 12px; color: var(--accent); font-weight: 600; }
.cue-meta { font-size: 11px; color: var(--fg3); }
.cue-text { font-size: 14px; line-height: 1.4; }
.cue-status { font-size: 11px; text-transform: uppercase; }
.cue-status.pending { color: var(--warning); }
.cue-status.resolved { color: var(--success); }
.cue-status.skipped { color: var(--fg3); }

.cue-actions {
  margin-top: 8px;
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.cue-action-btn {
  font-size: 12px;
  background: var(--bg3);
  border: 1px solid var(--border);
  color: var(--fg2);
  padding: 6px 12px;
  border-radius: 8px;
  cursor: pointer;
  font-family: var(--font);
  -webkit-tap-highlight-color: transparent;
}
.cue-action-btn:active { background: var(--hover); border-color: var(--accent-dim); }
.cue-action-btn.resolve:active { border-color: var(--success); color: var(--success); }

.cue-reply-composer {
  display: flex;
  gap: 6px;
  margin-top: 8px;
}
.cue-reply-composer input {
  flex: 1;
  background: var(--bg3);
  border: 1px solid var(--border);
  border-radius: 8px;
  color: var(--fg);
  padding: 8px 10px;
  font-family: var(--font);
  font-size: 13px;
}
.cue-reply-composer button {
  background: var(--accent-dim);
  border: none;
  color: var(--fg);
  padding: 8px 14px;
  border-radius: 8px;
  cursor: pointer;
  font-size: 13px;
  font-family: var(--font);
}
.cue-reply {
  font-size: 12px;
  color: var(--fg2);
  padding: 6px 0 3px 10px;
  border-left: 1px solid var(--border);
  margin-top: 4px;
}
.cue-reply .reply-author { color: var(--accent-dim); font-weight: 600; }
.cue-reply .reply-text { color: var(--fg2); }

/* ── Snapshot List ── */
.snapshot-list { }
.snapshot-timeline { position: relative; padding: 4px 0; }
.snapshot-timeline::before {
  content: ''; position: absolute; left: 20px; top: 10px;
  bottom: 10px; width: 1px;
  background: var(--border);
}
.snapshot-item {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 10px 12px;
  cursor: pointer;
  -webkit-tap-highlight-color: transparent;
}
.snapshot-item:active { background: var(--hover); }
.snapshot-item.HEAD { background: var(--bg2); }
.snapshot-dot {
  width: 10px; height: 10px; border-radius: 50%;
  background: var(--bg3);
  border: 2px solid var(--border);
  flex-shrink: 0; margin-top: 4px;
  position: relative; z-index: 1;
}
.snapshot-item.HEAD .snapshot-dot {
  background: var(--accent);
  border-color: var(--accent);
}
.snapshot-item.compare-selected .snapshot-dot {
  background: var(--warning);
  border-color: var(--warning);
}
.snapshot-content { flex: 1; min-width: 0; }
.snapshot-top {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}
.snap-hash { font-size: 12px; color: var(--accent); font-weight: 600; }
.snap-time { font-size: 11px; color: var(--fg3); }
.snap-msg { font-size: 14px; margin-top: 2px; }
.snap-meta { font-size: 11px; color: var(--fg2); margin-top: 1px; }
.snap-delta { font-size: 11px; }

/* ── Compare Bar ── */
.compare-bar {
  display: none;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  background: var(--bg3);
  border-bottom: 1px solid var(--border);
  font-size: 12px;
}
.compare-bar.visible { display: flex; }
.compare-bar .compare-info { color: var(--warning); }
.compare-bar button {
  background: none; border: 1px solid var(--accent-dim);
  color: var(--accent); padding: 4px 12px; border-radius: 8px;
  cursor: pointer; font-family: var(--font); font-size: 13px;
}

/* ── Diff Panel ── */
.diff-panel {
  display: none;
  border-top: 1px solid var(--border);
  background: var(--bg2);
}
.diff-panel.visible { display: block; }
.diff-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
}
.diff-header span { color: var(--accent); font-size: 12px; font-weight: 600; }
.diff-header button {
  background: var(--bg3); border: 1px solid var(--border);
  color: var(--fg2); padding: 4px 12px; border-radius: 8px;
  cursor: pointer; font-family: var(--font); font-size: 14px;
}
.diff-scroll {
  overflow-x: auto;
  padding: 8px 12px;
  font-size: 12px;
  max-height: 50vh;
  overflow-y: auto;
}
.diff-loading { color: var(--fg3); padding: 20px; text-align: center; }

/* ── Empty States ── */
.empty-state {
  padding: 24px 12px;
  text-align: center;
  color: var(--fg3);
  font-size: 13px;
}
.empty-state.loading { color: var(--accent-dim); }

/* ── Footer ── */
footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 12px;
  background: var(--bg2);
  border-top: 1px solid var(--border);
  font-size: 11px;
  color: var(--fg3);
}
.inject-btn {
  background: var(--bg3);
  border: 1px solid var(--border);
  color: var(--fg2);
  padding: 4px 12px;
  border-radius: 8px;
  cursor: pointer;
  font-size: 14px;
  font-family: var(--font);
}
.inject-btn:active { border-color: var(--accent-dim); color: var(--accent); }

/* ── Scrollbar ── */
::-webkit-scrollbar { width: 4px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border); border-radius: 2px; }

/* ── Animations ── */
@keyframes clavus-fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}

/* ── Desktop Upgrades ── */
@media (min-width: 768px) {
  header { padding: 10px 24px; }
  .tab-btn { font-size: 14px; padding: 12px 16px; }
  .project-info { padding: 12px 24px; gap: 12px; }
  .track-item { padding: 10px 24px; }
  .pane-header { padding: 10px 24px; }
  .cue-composer { padding: 10px 24px; }
  .cue-card { margin: 6px 20px; }
  .snapshot-item { padding: 10px 24px; }
  .compare-bar { padding: 8px 24px; }
  .diff-panel .diff-header { padding: 10px 24px; }
  .diff-scroll { padding: 12px 24px; }
  footer { padding: 8px 24px; }
  .lan-url { padding: 8px 24px; }
  .track-list { padding: 8px 0; }
  .cue-card { max-width: 640px; }
  .project-switcher { max-width: 300px; }
  main { max-width: 800px; margin: 0 auto; }
}

/* ── Wide Desktop ── */
@media (min-width: 1200px) {
  main { max-width: 960px; }
}
"""
def _generate_app_js() -> str:
    return """// Clavus Web Companion
let currentFilter = 'all';
let currentProject = localStorage.getItem('clavus_project') || '';

const $ = id => document.getElementById(id);

async function api(path, options = {}) {
  const url = '/api' + path;
  try {
    const resp = await fetch(url, {
      headers: { 'Content-Type': 'application/json' },
      ...options,
    });
    if (!resp.ok) {
      const text = await resp.text();
      console.error('API error:', url, resp.status, text);
      return { error: text };
    }
    return resp.json();
  } catch (e) {
    return { error: e.message };
  }
}

async function loadProject() {
  const query = currentProject ? '?name=' + encodeURIComponent(currentProject) : '';
  const data = await api('/project' + query);
  if (data.error) {
    $('connDot').className = 'conn-dot error';
    return;
  }
  $('connDot').className = 'conn-dot connected';
  currentProject = data.name || currentProject;

  if (data.project) {
    const p = data.project;
    $('bpm').textContent = p.bpm ? p.bpm.toFixed(1) : '—';
    $('timeSig').textContent = p.time_signature || '—';
    $('abletonVer').textContent = p.ableton_version || '—';
    $('trackCount').textContent = p.track_count || 0;

    const trackList = $('trackList');
    if (p.tracks && p.tracks.length) {
      trackList.innerHTML = p.tracks.map(t => {
        const color = ABLETON_COLORS[t.color] || '#3a5a65';
        const clipCount = t.clips ? t.clips.length : 0;
        return `<div class="track-item" style="border-left-color:${color}">
          <span class="track-dot" style="background:${color}"></span>
          <span class="track-name">${esc(t.name)}</span>
          <span class="track-type">${t.type}</span>
          ${clipCount > 0 ? `<span class="track-clip-count">${clipCount}</span>` : ''}
        </div>`;
      }).join('');
    } else {
      trackList.innerHTML = '<div class="empty-state">No tracks</div>';
    }
  }

  // History
  if (data.history && data.history.length) {
    $('snapshotCount').textContent = data.history.length;
    $('snapshotList').innerHTML = '<div class="snapshot-timeline">'
      + data.history.map((s, i) => {
        const prev = i < data.history.length - 1 ? data.history[i + 1] : null;
        const headClass = s.is_head ? ' HEAD' : '';
        return `<div class="snapshot-item${headClass}" onclick="handleSnapshot('${s.hash}','${s.full_hash}',event)">
          <div class="snapshot-dot"></div>
          <div class="snapshot-content">
            <div class="snapshot-top">
              <span class="snap-hash">${s.hash}</span>
              ${prev ? snapDelta(prev, s) : ''}
              <span class="snap-time">${timeAgo(s.timestamp)}</span>
            </div>
            <div class="snap-msg">${esc(s.message)}</div>
            <div class="snap-meta">${s.track_count} tracks @ ${s.bpm}bpm</div>
          </div>
        </div>`;
      }).join('') + '</div>';
  } else {
    $('snapshotCount').textContent = '0';
    $('snapshotList').innerHTML = '<div class="empty-state">No snapshots</div>';
  }
}

async function loadCues() {
  if (currentFilter === 'archived') {
    const query = currentProject ? '?project=' + encodeURIComponent(currentProject) : '';
    const data = await api('/cues/archived' + query);
    if (data.error) { $('cueList').innerHTML = '<div class="empty-state">Failed to load</div>'; return; }
    const cues = data.cues || [];
    if (!cues.length) { $('cueList').innerHTML = '<div class="empty-state">No archived cues</div>'; return; }
    $('cueList').innerHTML = cues.map(c => `<div class="cue-card">
      <div class="cue-card-header">
        <span class="cue-position">@${esc(c.position)}</span>
        <span class="cue-meta">${c.time_str}</span>
      </div>
      <div class="cue-text">${esc(c.text)}</div>
    </div>`).join('');
    return;
  }

  let url = '/cues?pending_only=' + (currentFilter === 'pending' ? 'true' : 'false');
  if (currentProject) url += '&name=' + encodeURIComponent(currentProject);
  const data = await api(url);
  if (data.error) { $('cueList').innerHTML = '<div class="empty-state">Failed to load</div>'; return; }

  let cues = data.cues || [];
  if (currentFilter !== 'all' && currentFilter !== 'pending') {
    cues = cues.filter(c => c.status === currentFilter);
  }

  $('cueCount').textContent = cues.length;

  if (!cues.length) {
    $('cueList').innerHTML = '<div class="empty-state">No cues yet</div>';
    return;
  }

  $('cueList').innerHTML = cues.map(c => `<div class="cue-card status-${c.status}">
    <div class="cue-card-header">
      <span class="cue-position">@${esc(c.position)}</span>
      <span class="cue-meta">${esc(c.author)} · ${c.time_str}</span>
    </div>
    <div class="cue-text">${esc(c.text)}</div>
    ${c.track_name ? `<div class="cue-meta">Track: ${esc(c.track_name)}</div>` : ''}
    ${c.assignee ? `<div class="cue-meta">${esc(c.assignee)}${c.in_progress ? ' ▶' : ''}</div>` : ''}
    <div class="cue-status ${c.status}">${c.status}${c.in_progress ? ' ▶' : ''}</div>
    ${(c.replies || []).map(r =>
      `<div class="cue-reply"><span class="reply-author">${esc(r.author)}:</span> <span class="reply-text">${esc(r.text)}</span></div>`
    ).join('')}
    <div class="cue-actions">
      <button class="cue-action-btn" onclick="showReply('${c.id}')">💬</button>
      ${c.assignee ? `<button class="cue-action-btn" onclick="assign('${c.id}','')">👤</button>
        ${c.in_progress ? `<button class="cue-action-btn" onclick="stop('${c.id}')">⏸</button>`
          : `<button class="cue-action-btn" onclick="start('${c.id}')">▶</button>`}
      ` : `<button class="cue-action-btn" onclick="assign('${c.id}')">👤</button>`}
      ${c.status === 'pending' ? `<button class="cue-action-btn resolve" onclick="resolve('${c.id}')">✅</button>
        <button class="cue-action-btn" onclick="skip('${c.id}')">⏭</button>`
      : c.status === 'resolved' ? `<button class="cue-action-btn" onclick="unresolve('${c.id}')">↩</button>
        <button class="cue-action-btn" onclick="archive('${c.id}')">📦</button>`
      : c.status === 'skipped' ? `<button class="cue-action-btn" onclick="unskip('${c.id}')">↩</button>
        <button class="cue-action-btn" onclick="archive('${c.id}')">📦</button>` : ''}
      <button class="cue-action-btn" onclick="del('${c.id}')">🗑</button>
    </div>
    <div class="cue-reply-composer" id="reply-${c.id}" style="display:none">
      <input type="text" id="reply-text-${c.id}" placeholder="Reply..." onkeydown="if(event.key==='Enter')postReply('${c.id}')">
      <button onclick="postReply('${c.id}')">Send</button>
    </div>
  </div>`).join('');
}

function showReply(id) {
  const el = $('reply-' + id);
  const show = el.style.display === 'none' || !el.style.display;
  el.style.display = show ? 'flex' : 'none';
  if (show) $('reply-text-' + id).focus();
}

async function postCue() {
  const text = $('cueText').value.trim();
  if (!text) return;
  const position = $('cuePosition').value.trim() || '0.0.0';
  $('cueSendBtn').textContent = '…';
  await api('/cues', {
    method: 'POST',
    body: JSON.stringify({ text, position, project_name: currentProject }),
  });
  $('cueSendBtn').textContent = '+';
  $('cueText').value = '';
  $('cuePosition').value = '0.0.0';
  loadCues();
}

async function postReply(id) {
  const text = $('reply-text-' + id).value.trim();
  if (!text) return;
  const q = currentProject ? '?name=' + encodeURIComponent(currentProject) : '';
  await api('/cues/' + id + '/reply' + q, { method: 'POST', body: JSON.stringify({ text }) });
  $('reply-text-' + id).value = '';
  $('reply-' + id).style.display = 'none';
  loadCues();
}

async function resolve(id) { await api('/cues/' + id + '/resolve?name=' + encodeURIComponent(currentProject), { method: 'POST' }); loadCues(); }
async function unresolve(id) { await api('/cues/' + id + '/resolve?name=' + encodeURIComponent(currentProject), { method: 'POST' }); loadCues(); }
async function skip(id) { await api('/cues/' + id + '/skip?name=' + encodeURIComponent(currentProject), { method: 'POST' }); loadCues(); }
async function unskip(id) { await api('/cues/' + id + '/skip?name=' + encodeURIComponent(currentProject), { method: 'POST' }); loadCues(); }
async function del(id) { if (!confirm('Delete?')) return; await api('/cues/' + id + '?project=' + encodeURIComponent(currentProject), { method: 'DELETE' }); loadCues(); }
async function archive(id) { await api('/cues/' + id + '/archive?project=' + encodeURIComponent(currentProject), { method: 'POST' }); loadCues(); }

async function assign(id, name) {
  if (!name) { name = prompt('Assign to:'); if (!name) return; }
  const q = '?project=' + encodeURIComponent(currentProject) + '&name=' + encodeURIComponent(name);
  await api('/cues/' + id + '/assign' + q, { method: 'POST' });
  loadCues();
}

async function start(id) { await api('/cues/' + id + '/start?project=' + encodeURIComponent(currentProject), { method: 'POST' }); loadCues(); }
async function stop(id) { await api('/cues/' + id + '/stop?project=' + encodeURIComponent(currentProject), { method: 'POST' }); loadCues(); }

function setFilter(filter) {
  currentFilter = filter;
  document.querySelectorAll('.filter-chip').forEach(b => b.classList.remove('active'));
  document.querySelector(`.filter-chip[data-filter="${filter}"]`).classList.add('active');
  loadCues();
}

// ─── Tab Switching ───
function switchTab(tab) {
  document.querySelectorAll('.tab-btn').forEach(b => b.classList.remove('active'));
  document.querySelectorAll('.tab-content').forEach(t => t.classList.remove('active'));
  document.querySelector(`.tab-btn[data-tab="${tab}"]`).classList.add('active');
  $(`tab-${tab}`).classList.add('active');
}

// ─── Snapshot Diff ───
let compareHash = null;
let compareFullHash = null;

function hideDiff() {
  $('snapshotDiffPanel').classList.remove('visible');
  $('diffContent').innerHTML = '';
}

function clearCompare() {
  compareHash = null;
  compareFullHash = null;
  $('compareBar').classList.remove('visible');
  document.querySelectorAll('.snapshot-item.compare-selected').forEach(el => el.classList.remove('compare-selected'));
}

async function showDiff(hash) {
  $('diffContent').innerHTML = '<div class="diff-loading">Loading...</div>';
  $('snapshotDiffPanel').classList.add('visible');
  $('diffTitle').textContent = 'Diff ' + hash;

  const snapData = await api('/snapshots/' + hash + '?name=' + encodeURIComponent(currentProject));
  if (snapData.error || !snapData.parent) {
    $('diffContent').innerHTML = '<div class="diff-loading">No parent to compare</div>';
    return;
  }

  const url = '/api/projects/compare?before=' + snapData.parent
    + '&after=' + hash
    + '&name=' + encodeURIComponent(currentProject);
  const resp = await fetch(url);
  if (!resp.ok) { $('diffContent').innerHTML = '<div class="diff-loading">Failed to load diff</div>'; return; }
  $('diffContent').innerHTML = await resp.text();
}

async function showCompareDiff(before, after) {
  $('diffContent').innerHTML = '<div class="diff-loading">Loading...</div>';
  $('snapshotDiffPanel').classList.add('visible');
  $('diffTitle').textContent = 'Diff ' + before + ' → ' + after;

  const url = '/api/projects/compare?before=' + before + '&after=' + after + '&name=' + encodeURIComponent(currentProject);
  const resp = await fetch(url);
  if (!resp.ok) { $('diffContent').innerHTML = '<div class="diff-loading">Failed</div>'; return; }
  $('diffContent').innerHTML = await resp.text();
}

function handleSnapshot(hash, fullHash, event) {
  if (event.ctrlKey || event.metaKey) {
    if (!compareHash) {
      compareHash = hash;
      compareFullHash = fullHash;
      event.currentTarget.classList.add('compare-selected');
      $('compareBar').classList.add('visible');
      $('compareInfo').textContent = 'Compare: ' + hash;
    } else if (compareHash === hash) {
      clearCompare();
    } else {
      showCompareDiff(compareHash, hash);
      clearCompare();
    }
  } else {
    showDiff(hash);
    // If on mobile, switch to snapshots tab
    if (window.innerWidth < 768) {
      switchTab('snapshots');
    }
  }
}

// ─── Project List ───
async function loadProjectList() {
  const data = await api('/projects');
  if (data.error || !data.projects) return;
  const select = $('projectSwitcher');
  const val = select.value || currentProject;
  select.innerHTML = data.projects.map(p =>
    `<option value="${esc(p.name)}"${p.name === val ? ' selected' : ''}>${esc(p.name)}</option>`
  ).join('');
}

function switchProject(name) {
  currentProject = name;
  if (name) localStorage.setItem('clavus_project', name);
  else localStorage.removeItem('clavus_project');
  loadAll();
}

async function loadAll() {
  await Promise.all([loadProjectList(), loadProject(), loadCues()]);
}

async function injectCues() {
  if (!currentProject) return;
  const result = await api('/projects/inject?name=' + encodeURIComponent(currentProject), { method: 'POST' });
}

// ─── Utilities ───
function esc(s) { return (s || '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }

const ABLETON_COLORS = [
  '#e8e8e8','#cc5050','#e88c50','#e8c850','#50cc50','#50cc8c','#50cccc','#508ce8',
  '#8c50e8','#cc50cc','#cc508c','#e8a060','#a0e860','#60e8a0','#60e8e8','#a060e8',
  '#e860cc','#e860a0','#404040','#808080','#b0b0b0','#d08040','#40d080','#6080d0'
];

function timeAgo(ts) {
  const d = Date.now() / 1000 - ts;
  if (d < 60) return 'now';
  if (d < 3600) return Math.floor(d / 60) + 'm';
  if (d < 86400) return Math.floor(d / 3600) + 'h';
  if (d < 604800) return Math.floor(d / 86400) + 'd';
  return new Date(ts * 1000).toLocaleDateString();
}

function snapDelta(prev, curr) {
  if (!prev) return '';
  return '<span class="snap-delta" style="color:var(--fg3)">➡ 0</span>';
}

document.addEventListener('DOMContentLoaded', () => {
  $('cueText').addEventListener('keydown', e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); postCue(); } });
  loadAll();
  setInterval(loadAll, 5000);
});
"""


def _write_template(name: str, content: str) -> None:
    path = HTML_DIR / name
    path.write_text(content)
    _HTML_CACHE[name] = content


def _get_tailscale_url(port: int = 7890) -> str:
    """Try to detect the Tailscale IP for sharing."""
    import socket
    try:
        # Tailscale uses 100.x.y.z range
        for ip in socket.gethostbyname_ex(socket.gethostname())[2]:
            if ip.startswith("100."):
                return f"http://{ip}:{port}"
    except Exception:
        pass
    try:
        # Try resolving the Tailscale hostname directly
        for info in socket.getaddrinfo(
            f"{socket.gethostname()}.tail?????.ts.net", 7890,
            socket.AF_INET, socket.SOCK_STREAM
        ):
            pass
    except Exception:
        pass
    return ""


def run_web_server(host: str = "0.0.0.0", port: int = 7890) -> None:
    """Run the web server for the Clavus Web Companion."""
    # Templates must be generated before import since they're cached at module level
    index_html = _generate_index_html()
    _generate_app_css()
    _generate_app_js()

    tailscale_url = _get_tailscale_url(port)

    # Detect LAN IP for phone sharing
    lan_url = ""
    try:
        import socket
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80))
        lan_ip = s.getsockname()[0]
        s.close()
        if lan_ip and not lan_ip.startswith("127."):
            lan_url = f"http://{lan_ip}:{port}"
    except Exception:
        pass

    # Inject LAN URL
    if lan_url:
        index_html = index_html.replace(
            '<div class="lan-url" id="lanUrl"></div>',
            f'<div class="lan-url" id="lanUrl" style="display:block">📱 Phone: <code>{lan_url}</code></div>'
        )
        path = _HTML_CACHE.get("index.html") or HTML_DIR / "index.html"
        if isinstance(path, str):
            path = HTML_DIR / "index.html"
        path.write_text(index_html)
        _HTML_CACHE["index.html"] = index_html

    import uvicorn
    print()
    print(f"  🌐  Clavus Web Companion")
    print(f"  {'─' * 40}")
    print(f"  Local:   http://localhost:{port}")
    if lan_url:
        print(f"  Phone:   {lan_url}")
    if tailscale_url:
        print(f"  Remote:  {tailscale_url}")
        print(f"  (via Tailscale — share this link)")
    else:
        print(f"  Share via Tailscale or Cloudflare tunnel.")
    print(f"  {'─' * 40}")
    print()

    # Start LAN advertising
    try:
        from clavus.discovery import ClavusAdvertiser
        from clavus.config import ClavusConfig

        cfg = ClavusConfig.load()
        proj_name = ""
        # Try to get current project name from store
        try:
            from clavus.store import BlobStore
            store = BlobStore()
            projects = store.list_projects()
            if projects:
                proj_name = projects[0].name
        except Exception:
            pass

        advertiser = ClavusAdvertiser()
        advertiser.start(
            port=port,
            project=proj_name,
            user=cfg.author,
            version="0.5.0",
        )
        # Store on app for lifecycle
        app.state.advertiser = advertiser
    except ImportError:
        pass
    except Exception as e:
        print(f"  ⚠️  LAN advertising failed: {e}")

    print(f"  Press Ctrl+C to stop.")
    print()
    uvicorn.run(app, host=host, port=port, log_level="warning")

    # Stop advertising on shutdown
    try:
        if hasattr(app.state, "advertiser"):
            app.state.advertiser.stop()
    except Exception:
        pass
