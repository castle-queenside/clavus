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

from fastapi import FastAPI, HTTPException, Query
from fastapi.responses import HTMLResponse, JSONResponse
from fastapi.staticfiles import StaticFiles
from pydantic import BaseModel

# ─── Clavus core ─────────────────────────────────────────────────────────

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from clavus.helpers import get_store_and_project, find_als_file
from clavus.cues import CueStore, CueFilter, format_cue_list
from clavus.store import BlobStore, ClavusProject, diff_projects, DEFAULT_CLAVUS_DIR
from clavus import parse_als

# ─── App setup ──────────────────────────────────────────────────────────

app = FastAPI(title="Clavus Web", version="0.2.0")

# HTML template path
HERE = Path(__file__).resolve().parent
HTML_DIR = HERE / "web"
HTML_DIR.mkdir(exist_ok=True)

_HTML_CACHE: dict[str, str] = {}


def _get_project() -> tuple[BlobStore, ClavusProject]:
    """Get the active clavus project."""
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


class CueReply(BaseModel):
    text: str


# ─── API Routes ─────────────────────────────────────────────────────────

@app.get("/api/ping")
async def ping():
    return {"status": "ok", "app": "clavus-web", "version": "0.2.0"}


@app.get("/api/project")
def get_project_sync():
    """Get current project info + snapshot history."""
    try:
        store, proj = _get_project()
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    # Parse the .als if it exists
    als_path = Path(proj.root_als)
    project_data = None
    if als_path.exists():
        try:
            project_obj = parse_als(als_path)
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
async def get_cues(pending_only: bool = False):
    """List all cues."""
    try:
        store, proj = _get_project()
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
        store, proj = _get_project()
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
    return {
        "id": new_cue.id,
        "text": new_cue.text,
        "position": new_cue.position,
        "status": "created",
    }


@app.post("/api/cues/{cue_id}/reply")
async def reply_to_cue(cue_id: str, reply: CueReply):
    """Reply to a cue."""
    try:
        store, proj = _get_project()
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    head = store.read_ref("HEAD")
    result = cues_store.reply(cue_id, reply.text, snapshot_hash=head or "")
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")
    return {"status": "ok", "replies": len(result.replies) if result else 0}


@app.post("/api/cues/{cue_id}/resolve")
async def resolve_cue(cue_id: str):
    """Resolve a cue."""
    try:
        store, proj = _get_project()
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    result = cues_store.resolve(cue_id)
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")
    return {"status": "resolved"}


@app.post("/api/cues/{cue_id}/skip")
async def skip_cue(cue_id: str):
    """Skip a cue."""
    try:
        store, proj = _get_project()
    except HTTPException:
        return JSONResponse({"error": "No clavus project found"}, status_code=404)

    cues_store = CueStore(proj.name, store=store)
    result = cues_store.skip(cue_id)
    if not result:
        raise HTTPException(status_code=404, detail=f"Cue '{cue_id}' not found")
    return {"status": "skipped"}


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
      <span class="project-name" id="projectName">—</span>
    </div>
    <div class="header-actions">
      <span class="connection-status" id="connStatus">⬤ connecting...</span>
      <button id="refreshBtn" onclick="loadAll()" title="Refresh">⟳</button>
    </div>
  </header>

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

/* ── Responsive ── */
@media (max-width: 900px) {
  main { grid-template-columns: 1fr; grid-template-rows: auto 1fr auto; }
  .pane-project { max-height: 200px; }
  .pane-history { display: none; }
}
"""


def _generate_app_js() -> str:
    return """// Clavus Web Companion — CRUX family UI
let currentFilter = 'all';
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
  const data = await api('/project');
  if (data.error) {
    $('projectName').textContent = '⚠ ' + data.error;
    return;
  }
  $('projectName').textContent = data.name;
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
  const data = await api('/cues?pending_only=' + (currentFilter === 'pending' ? 'true' : 'false'));
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
    body: JSON.stringify({ text, position }),
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

  await api('/cues/' + cueId + '/reply', {
    method: 'POST',
    body: JSON.stringify({ text }),
  });
  $('reply-text-' + cueId).value = '';
  $('reply-' + cueId).style.display = 'none';
  loadCues();
}

async function resolveCue(cueId) {
  await api('/cues/' + cueId + '/resolve', { method: 'POST' });
  loadCues();
}

function setFilter(filter) {
  currentFilter = filter;
  document.querySelectorAll('.filter-btn').forEach(b => b.classList.remove('active'));
  document.querySelector(`.filter-btn[data-filter="${filter}"]`).classList.add('active');
  loadCues();
}

async function loadAll() {
  await Promise.all([loadProject(), loadCues()]);
}

function escapeHtml(str) {
  if (!str) return '';
  return str.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

// Keyboard shortcut: Enter to send cue
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


def run_web_server(host: str = "0.0.0.0", port: int = 7890) -> None:
    """Run the web server for the Clavus Web Companion."""
    # Templates must be generated before import since they're cached at module level
    _generate_index_html()
    _generate_app_css()
    _generate_app_js()

    import uvicorn
    print(f"🌐 Clavus Web Companion running at http://{host}:{port}")
    print(f"   Share via Tailscale or Cloudflare tunnel for remote access.")
    print(f"   Press Ctrl+C to stop.")
    uvicorn.run(app, host=host, port=port, log_level="warning")
    path = HTML_DIR / name
    path.write_text(content)
    _HTML_CACHE[name] = content
