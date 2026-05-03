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

from fastapi import FastAPI, HTTPException, Query, WebSocket, WebSocketDisconnect
from fastapi.responses import HTMLResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

# ─── Clavus core ─────────────────────────────────────────────────────────

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from clavus.helpers import get_store_and_project, find_als_file
from clavus.cues import CueStore, CueFilter, format_cue_list, Cue, CueReply as CueReplyData
from clavus.store import BlobStore, ClavusProject, Snapshot, diff_projects, DEFAULT_CLAVUS_DIR
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
                    "tracks": [{"name": t.name, "type": t.track_type, "color": t.color}
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
    while current:
        snap = store.load_snapshot(current)
        if not snap:
            break
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
        current = snap.parent

    return {
        "name": proj.name,
        "root_als": str(proj.root_als),
        "branch": proj.branch,
        "project": project_data,
        "history": history,
        "head": proj.head[:8] if proj.head else None,
    }


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

    return {"status": "ok", "replies": len(result.replies) if result else 0}


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
    while current:
        snap = store.load_snapshot(current)
        if not snap:
            break
        history.append({
            "hash": snap.hash[:8], "full_hash": snap.hash,
            "timestamp": snap.timestamp, "message": snap.message,
            "track_count": snap.track_count, "bpm": snap.bpm,
            "is_head": current == store.read_ref("HEAD"),
        })
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
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>clavus</title>
<link rel="stylesheet" href="/app.css">
</head>
<body>
  <header>
    <div class="logo">
      <span class="logo-icon">⧩</span>
      <span class="logo-text">clavus</span>
      <select class="project-switcher" id="projectSwitcher" onchange="switchProject(this.value)">
        <option value="">—</option>
      </select>
    </div>
    <div class="header-actions">
      <span class="connection-status" id="connStatus">⬤ connecting...</span>
      <button id="refreshBtn" onclick="loadAll()" title="Refresh">⟳</button>
    </div>
  </header>
  <div class="tailscale-url" id="tailscaleUrl"></div>

  <main>
    <!-- LEFT: Project Pane -->
    <section class="pane pane-project">
      <div class="pane-header">
        <h2>Project</h2>
        <span class="pane-badge" id="trackCount">—</span>
      </div>
      <div class="project-info">
        <div class="info-row"><span class="label">BPM</span><span class="value" id="bpm">—</span></div>
        <div class="info-row"><span class="label">Time Sig</span><span class="value" id="timeSig">—</span></div>
        <div class="info-row"><span class="label">Ableton</span><span class="value" id="abletonVer">—</span></div>
      </div>
      <div class="track-list" id="trackList">
        <div class="empty-state">No tracks loaded</div>
      </div>
      <div class="marker-list" id="markerList">
        <h3>Markers</h3>
        <div class="empty-state">No markers</div>
      </div>
    </section>

    <!-- CENTER: Cues Timeline -->
    <section class="pane pane-cues">
      <div class="pane-header">
        <h2>Cues</h2>
        <div class="pane-filters">
          <button class="filter-btn active" data-filter="all" onclick="setFilter('all')">All</button>
          <button class="filter-btn" data-filter="pending" onclick="setFilter('pending')">Pending</button>
          <button class="filter-btn" data-filter="resolved" onclick="setFilter('resolved')">Resolved</button>
        </div>
      </div>
      <div class="cue-composer" id="cueComposer">
        <input type="text" class="cue-position-input" id="cuePosition" placeholder="@1:23" value="0.0.0">
        <input type="text" class="cue-text-input" id="cueText" placeholder="Leave a cue...">
        <button class="cue-send-btn" onclick="postCue()">+ Cue</button>
      </div>
      <div class="cue-list" id="cueList">
        <div class="empty-state loading">Loading cues...</div>
      </div>
    </section>

    <!-- RIGHT: History -->
    <section class="pane pane-history">
      <div class="pane-header">
        <h2>History</h2>
        <span class="pane-badge" id="snapshotCount">—</span>
      </div>
      <div class="snapshot-list" id="snapshotList">
        <div class="empty-state">No snapshots</div>
      </div>
    </section>
  </main>

  <footer>
    <span class="footer-left">clavus <span id="version">0.2.0</span></span>
    <span class="footer-right">
      <span class="sync-info" id="syncInfo">local</span>
      <a href="#" class="server-link" onclick="showServerInfo()">info</a>
    </span>
  </footer>

  <script src="/app.js"></script>
</body>
</html>"""


def _generate_app_css() -> str:
    return """:root {
  --bg-pri: #0b1418;
  --bg-sec: #0f1a20;
  --bg-ter: #15242b;
  --bg-hover: #1a2d36;
  --accent: #1a9e9e;
  --accent-dim: #0f6b6b;
  --fg: #b8c8c8;
  --fg-dim: #6a8a8a;
  --fg-muted: #3a5a65;
  --border: #1a3040;
  --danger: #d45a5a;
  --success: #4a9e6a;
  --warning: #d4a04a;
  --radius: 4px;
  --font: 'SF Mono', 'Cascadia Code', 'JetBrains Mono', 'Consolas', monospace;
}

* { margin: 0; padding: 0; box-sizing: border-box; }

html, body {
  height: 100%;
  background: var(--bg-pri);
  color: var(--fg);
  font-family: var(--font);
  font-size: 13px;
  overflow: hidden;
}

/* ── Header ── */
header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 16px;
  background: var(--bg-sec);
  border-bottom: 1px solid var(--border);
  -webkit-app-region: drag;
}
.logo { display: flex; align-items: center; gap: 8px; }
.logo-icon { color: var(--accent); font-size: 18px; }
.logo-text { color: var(--accent); font-weight: bold; font-size: 14px; }
.project-switcher {
  background: var(--bg-ter);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg-dim);
  font-family: var(--font);
  font-size: 12px;
  padding: 2px 6px;
  cursor: pointer;
  max-width: 200px;
  -webkit-app-region: no-drag;
}
.project-switcher:focus { outline: none; border-color: var(--accent-dim); }
.project-switcher:hover { border-color: var(--accent-dim); color: var(--accent); }
.project-name { color: var(--fg-dim); font-size: 12px; }
.header-actions { display: flex; align-items: center; gap: 12px; -webkit-app-region: no-drag; }
.connection-status { font-size: 11px; color: var(--success); }
.connection-status.error { color: var(--danger); }
#refreshBtn {
  background: none; border: 1px solid var(--border); color: var(--fg-dim);
  padding: 4px 10px; border-radius: var(--radius); cursor: pointer; font-size: 14px;
}
#refreshBtn:hover { border-color: var(--accent); color: var(--accent); }

/* ── Main Layout ── */
main {
  display: grid;
  grid-template-columns: 280px 1fr 280px;
  height: calc(100vh - 56px);
  gap: 1px;
  background: var(--border);
}

/* ── Panes ── */
.pane {
  background: var(--bg-pri);
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.pane-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-sec);
}
.pane-header h2 {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 1px;
  color: var(--fg-dim);
}
.pane-badge {
  font-size: 10px;
  color: var(--accent);
  background: var(--bg-ter);
  padding: 2px 8px;
  border-radius: 10px;
}

/* ── Project Info ── */
.project-info {
  padding: 10px 12px;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px;
}
.info-row {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.info-row .label { font-size: 10px; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.5px; }
.info-row .value { font-size: 14px; font-weight: 600; }

/* ── Tracks ── */
.track-list { padding: 6px 12px; flex: 1; overflow-y: auto; }
.track-item {
  display: flex; align-items: center; gap: 8px;
  padding: 4px 0; font-size: 12px;
}
.track-dot {
  width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0;
}
.track-name { flex: 1; }
.track-type { font-size: 10px; color: var(--fg-muted); }

/* ── Markers ── */
.marker-list { padding: 6px 12px; border-top: 1px solid var(--border); }
.marker-list h3 { font-size: 10px; color: var(--fg-muted); text-transform: uppercase; margin-bottom: 6px; }
.marker-item { font-size: 11px; padding: 2px 0; color: var(--fg-dim); }
.marker-item .pos { color: var(--accent-dim); }

/* ── Cue Composer ── */
.cue-composer {
  display: flex;
  gap: 6px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-sec);
}
.cue-position-input {
  width: 72px;
  background: var(--bg-ter);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg);
  padding: 6px 8px;
  font-family: var(--font);
  font-size: 12px;
  text-align: center;
}
.cue-position-input:focus { outline: none; border-color: var(--accent-dim); }
.cue-text-input {
  flex: 1;
  background: var(--bg-ter);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg);
  padding: 6px 8px;
  font-family: var(--font);
  font-size: 12px;
}
.cue-text-input:focus { outline: none; border-color: var(--accent-dim); }
.cue-text-input::placeholder { color: var(--fg-muted); }
.cue-send-btn {
  background: var(--accent);
  border: none;
  color: var(--bg-pri);
  font-weight: bold;
  padding: 6px 14px;
  border-radius: var(--radius);
  cursor: pointer;
  font-family: var(--font);
  font-size: 12px;
}
.cue-send-btn:hover { background: var(--accent-dim); }

/* ── Cue List ── */
.cue-list { flex: 1; overflow-y: auto; padding: 6px 0; }
.cue-card {
  margin: 4px 8px;
  padding: 8px 10px;
  background: var(--bg-sec);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  border-left: 3px solid var(--accent-dim);
}
.cue-card.status-resolved { border-left-color: var(--success); opacity: 0.6; }
.cue-card.status-skipped { border-left-color: var(--fg-muted); opacity: 0.5; }
.cue-card-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}
.cue-position { font-size: 11px; color: var(--accent); font-weight: 600; }
.cue-meta { font-size: 10px; color: var(--fg-muted); }
.cue-text { font-size: 12px; line-height: 1.4; }
.cue-status { font-size: 10px; text-transform: uppercase; }
.cue-status.pending { color: var(--warning); }
.cue-status.resolved { color: var(--success); }
.cue-status.skipped { color: var(--fg-muted); }
.cue-actions { margin-top: 6px; display: flex; gap: 6px; }
.cue-action-btn {
  font-size: 10px;
  background: none;
  border: 1px solid var(--border);
  color: var(--fg-dim);
  padding: 2px 8px;
  border-radius: var(--radius);
  cursor: pointer;
  font-family: var(--font);
}
.cue-action-btn:hover { border-color: var(--accent-dim); color: var(--accent); }
.cue-action-btn.resolve:hover { border-color: var(--success); color: var(--success); }
.cue-reply-composer {
  display: flex;
  gap: 4px;
  margin-top: 6px;
}
.cue-reply-composer input {
  flex: 1;
  background: var(--bg-ter);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg);
  padding: 4px 6px;
  font-family: var(--font);
  font-size: 11px;
}
.cue-reply-composer button {
  background: var(--accent-dim);
  border: none;
  color: var(--fg);
  padding: 4px 10px;
  border-radius: var(--radius);
  cursor: pointer;
  font-size: 11px;
  font-family: var(--font);
}
.cue-reply {
  font-size: 11px;
  color: var(--fg-dim);
  padding: 4px 0 2px 8px;
  border-left: 1px solid var(--border);
  margin-top: 4px;
}
.cue-reply .reply-author { color: var(--accent-dim); font-weight: 600; }
.cue-reply .reply-text { color: var(--fg-dim); }

/* ── Snapshot List ── */
.snapshot-list { padding: 6px 0; flex: 1; overflow-y: auto; }
.snapshot-item {
  padding: 8px 12px;
  border-bottom: 1px solid var(--border);
  cursor: pointer;
}
.snapshot-item:hover { background: var(--bg-sec); }
.snapshot-item.noselect { opacity: 0.5; }
.snapshot-item .snap-hash { font-size: 11px; color: var(--accent); font-weight: 600; }
.snapshot-item .snap-time { font-size: 10px; color: var(--fg-muted); float: right; }
.snapshot-item .snap-msg { font-size: 12px; margin-top: 2px; }
.snapshot-item .snap-meta { font-size: 10px; color: var(--fg-dim); margin-top: 2px; }
.snapshot-item .head-indicator { color: var(--success); font-size: 10px; }
.snapshot-item.active { background: var(--bg-ter); border-left: 3px solid var(--accent); }

/* ── Filters ── */
.pane-filters { display: flex; gap: 2px; }
.filter-btn {
  background: none; border: none; color: var(--fg-muted);
  padding: 2px 8px; border-radius: var(--radius);
  cursor: pointer; font-family: var(--font); font-size: 10px;
}
.filter-btn:hover { color: var(--fg-dim); }
.filter-btn.active { background: var(--bg-ter); color: var(--accent); }

/* ── Empty / Loading States ── */
.empty-state {
  padding: 24px 12px;
  text-align: center;
  color: var(--fg-muted);
  font-size: 12px;
}
.empty-state.loading { color: var(--accent-dim); }

/* ── Footer ── */
footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 4px 16px;
  background: var(--bg-sec);
  border-top: 1px solid var(--border);
  font-size: 10px;
  color: var(--fg-muted);
}
footer a { color: var(--accent-dim); text-decoration: none; }
footer a:hover { color: var(--accent); }

/* ── Scrollbar ── */
::-webkit-scrollbar { width: 6px; }
::-webkit-scrollbar-track { background: transparent; }
::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }
::-webkit-scrollbar-thumb:hover { background: var(--fg-muted); }

/* ── Tailscale URL ── */
.tailscale-url {
  text-align: center;
  padding: 4px 12px;
  font-size: 9px;
  color: var(--fg-muted);
  background: var(--bg-sec);
  border-bottom: 1px solid var(--border);
  font-family: var(--font);
}
.tailscale-url code {
  color: var(--accent-dim);
  font-size: 10px;
}

/* ── Responsive ── */
@media (max-width: 900px) {
  main { grid-template-columns: 1fr; grid-template-rows: auto 1fr auto; }
  .pane-project { max-height: 200px; }
  .pane-history { display: none; }
  header { padding: 6px 10px; flex-wrap: wrap; gap: 4px; }
  .logo-text { font-size: 12px; }
  .project-switcher { max-width: 140px; font-size: 11px; }
  .pane-header h2 { font-size: 10px; }
  .cue-text-input { font-size: 16px; } /* prevent iOS zoom */
  .cue-composer { flex-wrap: wrap; }
  .cue-position-input { width: 60px; }
  .cue-send-btn { padding: 8px 16px; font-size: 14px; }
  .cue-action-btn { padding: 6px 12px; font-size: 12px; }
  footer { font-size: 9px; padding: 3px 10px; flex-wrap: wrap; gap: 4px; }
}

@media (max-width: 480px) {
  .pane.project { max-height: 150px; }
  .project-info { grid-template-columns: 1fr; }
  .project-switcher { max-width: 100px; }
  .pane-filters { gap: 1px; }
  .filter-btn { padding: 4px 6px; font-size: 9px; }
}
"""


def _generate_app_js() -> str:
    return """// Clavus Web Companion — CRUX family UI
let currentFilter = 'all';
let currentProject = localStorage.getItem('clavus_project') || '';
let POLL_INTERVAL = 5000; // 5s auto-refresh

function $(id) { return document.getElementById(id); }

async function api(path, options = {}) {
  const url = '/api' + path;
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
}

async function loadProject() {
  const query = currentProject ? '?name=' + encodeURIComponent(currentProject) : '';
  const data = await api('/project' + query);
  if (data.error) {
    $('connStatus').textContent = '⚠ ' + data.error;
    $('connStatus').className = 'connection-status error';
    return;
  }
  $('connStatus').textContent = '⬤ connected';
  $('connStatus').className = 'connection-status';

  if (data.project) {
    const p = data.project;
    $('bpm').textContent = p.bpm || '—';
    $('timeSig').textContent = p.time_signature || '—';
    $('abletonVer').textContent = p.ableton_version || '—';
    $('trackCount').textContent = p.track_count || 0;

    // Tracks
    const trackList = $('trackList');
    if (p.tracks && p.tracks.length) {
      trackList.innerHTML = p.tracks.map(t => `
        <div class="track-item">
          <span class="track-dot" style="background:#${t.color.toString(16).padStart(6,'0')}"></span>
          <span class="track-name">${escapeHtml(t.name)}</span>
          <span class="track-type">${t.type}</span>
        </div>
      `).join('');
    } else {
      trackList.innerHTML = '<div class="empty-state">No tracks loaded</div>';
    }

    // Markers
    const markerList = $('markerList');
    if (p.markers && p.markers.length) {
      markerList.innerHTML = '<h3>Markers</h3>' + p.markers.map(m =>
        `<div class="marker-item"><span class="pos">${escapeHtml(m.time)}</span> ${escapeHtml(m.name)}</div>`
      ).join('');
    } else {
      markerList.innerHTML = '<h3>Markers</h3><div class="empty-state">No markers</div>';
    }
  }

  // History
  if (data.history && data.history.length) {
    $('snapshotCount').textContent = data.history.length;
    $('snapshotList').innerHTML = data.history.map(s => `
      <div class="snapshot-item ${s.is_head ? 'active' : 'noselect'}">
        <div>
          <span class="snap-hash">${s.is_head ? '➡ ' : ''}${s.hash}</span>
          <span class="snap-time">${s.time_str}</span>
        </div>
        <div class="snap-msg">${escapeHtml(s.message)}</div>
        <div class="snap-meta">${s.track_count} tracks @ ${s.bpm}bpm</div>
      </div>
    `).join('');
  } else {
    $('snapshotCount').textContent = '0';
    $('snapshotList').innerHTML = '<div class="empty-state">No snapshots</div>';
  }
}

async function loadCues() {
  let url = '/cues?pending_only=' + (currentFilter === 'pending' ? 'true' : 'false');
  if (currentProject) url += '&name=' + encodeURIComponent(currentProject);
  const data = await api(url);
  if (data.error) {
    $('cueList').innerHTML = '<div class="empty-state error">⚠ Failed to load cues</div>';
    return;
  }

  let cues = data.cues || [];
  if (currentFilter !== 'all' && currentFilter !== 'pending') {
    cues = cues.filter(c => c.status === currentFilter);
  }

  if (!cues.length) {
    $('cueList').innerHTML = '<div class="empty-state">No cues yet. Leave one above.</div>';
    return;
  }

  $('cueList').innerHTML = cues.map(c => `
    <div class="cue-card status-${c.status}">
      <div class="cue-card-header">
        <span class="cue-position">@${escapeHtml(c.position)}</span>
        <span class="cue-meta">${c.author} · ${c.time_str}</span>
      </div>
      <div class="cue-text">${escapeHtml(c.text)}</div>
      ${c.track_name ? `<div class="cue-meta" style="margin-top:2px">Track: ${escapeHtml(c.track_name)}</div>` : ''}
      ${(c.replies || []).map(r =>
        `<div class="cue-reply">
          <span class="reply-author">${escapeHtml(r.author)}:</span>
          <span class="reply-text">${escapeHtml(r.text)}</span>
        </div>`
      ).join('')}
      <div class="cue-actions">
        <span class="cue-status ${c.status}">${c.status}</span>
        ${c.status === 'pending' ? `
          <button class="cue-action-btn" onclick="showReply('${c.id}')">💬 Reply</button>
          <button class="cue-action-btn resolve" onclick="resolveCue('${c.id}')">✅ Resolve</button>
        ` : ''}
      </div>
      <div class="cue-reply-composer" id="reply-${c.id}" style="display:none">
        <input type="text" id="reply-text-${c.id}" placeholder="Type a reply..." onkeydown="if(event.key==='Enter')postReply('${c.id}')">
        <button onclick="postReply('${c.id}')">Send</button>
      </div>
    </div>
  `).join('');
}

function showReply(cueId) {
  const el = $('reply-' + cueId);
  el.style.display = el.style.display === 'none' ? 'flex' : 'none';
  if (el.style.display === 'flex') {
    $('reply-text-' + cueId).focus();
  }
}

async function postCue() {
  const text = $('cueText').value.trim();
  const position = $('cuePosition').value.trim() || '0.0.0';
  if (!text) return;

  $('cueSendBtn').textContent = '...';
  const result = await api('/cues', {
    method: 'POST',
    body: JSON.stringify({ text, position, project_name: currentProject }),
  });
  $('cueSendBtn').textContent = '+ Cue';
  if (!result.error) {
    $('cueText').value = '';
    $('cuePosition').value = '0.0.0';
    loadCues();
  }
}

async function postReply(cueId) {
  const text = $('reply-text-' + cueId).value.trim();
  if (!text) return;

  const query = currentProject ? '?name=' + encodeURIComponent(currentProject) : '';
  await api('/cues/' + cueId + '/reply' + query, {
    method: 'POST',
    body: JSON.stringify({ text }),
  });
  $('reply-text-' + cueId).value = '';
  $('reply-' + cueId).style.display = 'none';
  loadCues();
}

async function resolveCue(cueId) {
  const query = currentProject ? '?name=' + encodeURIComponent(currentProject) : '';
  await api('/cues/' + cueId + '/resolve' + query, { method: 'POST' });
  loadCues();
}

function setFilter(filter) {
  currentFilter = filter;
  document.querySelectorAll('.filter-btn').forEach(b => b.classList.remove('active'));
  document.querySelector(`.filter-btn[data-filter="${filter}"]`).classList.add('active');
  loadCues();
}

async function loadAll() {
  await Promise.all([loadProjectList(), loadProject(), loadCues()]);
}

async function loadProjectList() {
  const data = await api('/projects');
  if (data.error || !data.projects) return;
  const select = $('projectSwitcher');
  const currentVal = select.value || currentProject;
  select.innerHTML = '<option value="">Select project…</option>'
    + data.projects.map(p =>
      `<option value="${escapeHtml(p.name)}"${p.name === currentVal ? ' selected' : ''}>${escapeHtml(p.name)}</option>`
    ).join('');
}

function switchProject(name) {
  currentProject = name;
  if (name) {
    localStorage.setItem('clavus_project', name);
  } else {
    localStorage.removeItem('clavus_project');
  }
  loadAll();
}

function showServerInfo() {
  const url = window.location.href;
  const ts = document.getElementById('tailscaleUrl');
  const info = ts && ts.textContent ? ts.textContent : 'Local only';
  alert('Clavus Web Companion\nURL: ' + url + '\n' + info);
}

function escapeHtml(str) {
  if (!str) return '';
  return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

document.addEventListener('DOMContentLoaded', () => {
  $('cueText').addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      postCue();
    }
  });
  loadAll();
  setInterval(loadAll, POLL_INTERVAL);
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

    # Inject Tailscale URL into the HTML
    if tailscale_url:
        index_html = index_html.replace(
            '<div class="tailscale-url" id="tailscaleUrl"></div>',
            f'<div class="tailscale-url" id="tailscaleUrl">📡 Tailscale: <code>{tailscale_url}</code></div>'
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
    if tailscale_url:
        print(f"  Remote:  {tailscale_url}")
        print(f"  (via Tailscale — share this link)")
    else:
        print(f"  Share via Tailscale or Cloudflare tunnel.")
    print(f"  {'─' * 40}")
    print()
    print(f"  Press Ctrl+C to stop.")
    print()
    uvicorn.run(app, host=host, port=port, log_level="warning")
