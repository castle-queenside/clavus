"""Clavus API Server — FastAPI backend for TUI/CLI sync and collaboration.

REST API for snapshot management, cues, stem sync, and P2P relay.
No web UI — designed for headless/relay operation.

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
from fastapi.responses import JSONResponse
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

    return JSONResponse({"diff": html})


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
        "assignee": c.assignee, "in_progress": c.in_progress,
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


# ─── Snapshot Blob Sync Endpoints ─────────────────────────────────────────


class SyncCheckBlobsBody(BaseModel):
    """List of content blob hashes to check for presence."""
    hashes: list[str] = []


class SyncBlobUpload(BaseModel):
    """Single blob to upload: hash + base64-encoded data."""
    hash: str
    data: str  # Base64-encoded bytes


@ app.post("/api/sync/check-blobs")
async def sync_check_blobs(body: SyncCheckBlobsBody):
    """Given a list of blob hashes, return which ones are missing locally."""
    store = BlobStore()
    missing = [h for h in body.hashes if not store.has_object(h)]
    return {"missing": missing}


@ app.post("/api/sync/push-blobs")
async def sync_push_blobs(body: list[SyncBlobUpload]):
    """Upload a batch of content-addressed blobs to the relay.

    Each blob is a {hash, base64_data} pair. The relay stores them
    using put_object so they're content-addressed and deduplicated.
    """
    import base64
    store = BlobStore()
    stored = 0
    for blob in body:
        raw = base64.b64decode(blob.data)
        store.put_object(raw, blob.hash)
        stored += 1
    return {"status": "ok", "stored": stored}


@ app.post("/api/sync/push-als-blobs")
async def sync_push_als_blobs(body: list[SyncBlobUpload]):
    """Upload .als backup blobs (raw .als file bytes) to the relay."""
    import base64
    store = BlobStore()
    stored = 0
    for blob in body:
        raw = base64.b64decode(blob.data)
        store.put_object(raw, blob.hash)
        stored += 1
    return {"status": "ok", "stored": stored}


@ app.get("/api/blobs/{blob_hash}")
async def get_blob(blob_hash: str):
    """Generic GET endpoint for any content-addressed blob.

    Returns raw bytes. Used by pull_snapshot_blobs to fetch
    content blobs and .als backups from the relay.
    """
    store = BlobStore()
    data = store.get_object(blob_hash)
    if not data:
        return JSONResponse({"error": "Blob not found"}, status_code=404)

    from fastapi.responses import Response
    return Response(
        content=data,
        media_type="application/octet-stream",
        headers={
            "Content-Length": str(len(data)),
        },
    )


# ─── Web UI: Main page ──────────────────────────────────────────────────

# In-memory share state (set by relay on startup)
_SHARE_CODE: str = ""

def set_share_code(code: str) -> None:
    global _SHARE_CODE
    _SHARE_CODE = code


@app.get("/api/share")
async def get_share_info():
    """Return share info for clavus join discovery.

    Returns the current share code, project info, and author.
    """
    import socket
    store = BlobStore()
    projects = store.list_projects()
    proj_info = None
    if projects:
        p = projects[0]
        proj_info = {"name": p.name, "head": p.head[:8] if p.head else None}

    from clavus.config import ClavusConfig
    cfg = ClavusConfig.load()

    return {
        "share_code": _SHARE_CODE,
        "author": cfg.author or "",
        "project": proj_info,
        "version": "0.6.0",
        "hostname": socket.gethostname(),
    }

# ─── Web companion stripped (HTML/CSS/JS removed) ────────────────


# ─── Template generators were here — removed with web companion ──

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





def _get_tailscale_url(port: int = 7890) -> str:
    """Try to detect the Tailscale IP for sharing."""
    import socket
    try:
        # Method 1: use 'tailscale ip' command (most reliable)
        import subprocess
        r = subprocess.run(
            ["tailscale", "ip", "-4"],
            capture_output=True, text=True, timeout=5,
        )
        if r.returncode == 0:
            ip = r.stdout.strip()
            if ip and ip.startswith("100."):
                return f"http://{ip}:{port}"
    except Exception:
        pass

    try:
        # Method 2: check network interfaces for 100.x.y.z
        import subprocess
        r = subprocess.run(
            ["ifconfig", "-l"],
            capture_output=True, text=True, timeout=3,
        )
        if r.returncode == 0:
            for iface in r.stdout.strip().split():
                r2 = subprocess.run(
                    ["ifconfig", iface],
                    capture_output=True, text=True, timeout=3,
                )
                for line in r2.stdout.splitlines():
                    cleaned = line.strip()
                    if "inet " in cleaned and "100." in cleaned:
                        ip = cleaned.split()[1]
                        return f"http://{ip}:{port}"
    except Exception:
        pass

    try:
        # Method 3: old approach via gethostbyname_ex
        for ip in socket.gethostbyname_ex(socket.gethostname())[2]:
            if ip.startswith("100."):
                return f"http://{ip}:{port}"
    except Exception:
        pass

    try:
        # Method 4: check all interfaces via netifaces fallback
        # Try to find any 100.x.x.x on any interface
        # by listing all IPs on the system
        import subprocess
        r = subprocess.run(
            ["ipconfig", "getifaddr", "en0"],
            capture_output=True, text=True, timeout=3,
        )
        if r.returncode == 0 and r.stdout.strip().startswith("100."):
            return f"http://{r.stdout.strip()}:{port}"
        r = subprocess.run(
            ["ipconfig", "getifaddr", "en1"],
            capture_output=True, text=True, timeout=3,
        )
        if r.returncode == 0 and r.stdout.strip().startswith("100."):
            return f"http://{r.stdout.strip()}:{port}"
    except Exception:
        pass

    return ""


# ─── Web companion server (run_web_server) removed — use relay ──


def run_relay_server(host: str = "0.0.0.0", port: int = 7890, share_code: str = "") -> None:
    """Run stripped-down relay server — no HTML, no mDNS, just API + WebSocket.

    The relay is an always-on version of the web companion designed to run
    on a VPS, Raspberry Pi, or old laptop. It serves the same API routes
    and WebSocket hub, without the TUI, HTML template generation, or LAN
    advertising. Perfect for Tailscale deployment.

    If share_code is provided, it's exposed via /api/share for the
    clavus share/join discovery flow.
    """
    if share_code:
        set_share_code(share_code)
    tailscale_url = _get_tailscale_url(port)

    # Detect LAN IP
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

    import uvicorn
    print()
    print(f"  ⧩  Clavus Relay")
    print(f"  {'─' * 40}")
    print(f"  Local:   http://localhost:{port}")
    if lan_url:
        print(f"  LAN:     {lan_url}")
    if tailscale_url:
        print(f"  Remote:  {tailscale_url}")
        print(f"  (via Tailscale — share this URL with collaborators)")
    else:
        print(f"  No Tailscale detected — install for remote access.")
    if share_code:
        print(f"  {'─' * 40}")
        print(f"  🔗 Share code: {share_code}")
        print(f"  Tell a friend to run: clavus join")
    print(f"  {'─' * 40}")
    print()
    print(f"  Press Ctrl+C to stop.")
    print()
    uvicorn.run(app, host=host, port=port, log_level="warning")
