"""
Clavus — cue system.

Threaded, timeline-anchored comments for Ableton projects.

The review loop:
  1. Add a cue:  clavus cue "bridge feels long, try 4 bars" @3:45
  2. Reply:      clavus cue reply <cue-id> "got it, bumped 2dB"
  3. Resolve:    clavus cue resolve <cue-id>
  4. Skip:       clavus cue skip <cue-id> "already fixed this in arrangement"
  5. Render:     clavus cue render-als  — export cues as Ableton markers
  6. Stems:      clavus render --track "Kick"  — export a single stem

Cue lifecycle:
  ┌──────────┐
  │  Pending  │─── skip ──→│  Skipped  │
  │          │─── resolve ─→│ Resolved  │──¬
  └──────────┘             └───────────┘  │
       │                                  │
       └── replies keep it open ──────────┘
"""

from __future__ import annotations

import json
import os
import time
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Optional

from clavus.store import BlobStore, DEFAULT_CLAVUS_DIR


# ─── Data Model ─────────────────────────────────────────────────────────

@dataclass
class CueReply:
    """A single reply in a cue thread."""
    id: str
    text: str
    author: str
    timestamp: float
    snapshot_hash: str = ""


@dataclass
class Cue:
    """A timeline-anchored comment."""
    id: str
    position: str  # e.g., "4.1.1", "0:00:00", or "3:45"
    text: str
    author: str
    timestamp: float
    status: str = "pending"  # pending, resolved, skipped, deferred
    snapshot_hash: str = ""  # The snapshot this cue is attached to
    track_name: str = ""  # Optional: which track the cue is about
    replies: list[CueReply] = field(default_factory=list)

    @property
    def is_open(self) -> bool:
        return self.status == "pending"

    @property
    def thread_length(self) -> int:
        return len(self.replies)


@dataclass
class CueFilter:
    """Filter criteria for listing cues."""
    status: Optional[str] = None  # pending, resolved, skipped, deferred
    author: Optional[str] = None
    snapshot: Optional[str] = None
    track: Optional[str] = None


# ─── Cue Store ─────────────────────────────────────────────────────────

class CueStore:
    """Persistent storage for cues."""

    def __init__(self, project_name: str, store: Optional[BlobStore] = None):
        self.store = store or BlobStore()
        self.cues_dir = self.store.root / "cues" / project_name
        self.cues_dir.mkdir(parents=True, exist_ok=True)

    # ── CRUD ──

    def add_cue(self, text: str, position: str, author: str = "",
                snapshot_hash: str = "", track_name: str = "") -> Cue:
        """Add a new cue at a timeline position."""
        import hashlib
        seed = f"{text}{position}{time.time()}{author}"
        cue_id = hashlib.sha256(seed.encode()).hexdigest()[:12]

        cue = Cue(
            id=cue_id,
            position=position,
            text=text,
            author=author or os.environ.get("USER", "unknown"),
            timestamp=time.time(),
            status="pending",
            snapshot_hash=snapshot_hash,
            track_name=track_name,
        )
        self._save_cue(cue)
        return cue

    def reply(self, cue_id: str, text: str, author: str = "",
              snapshot_hash: str = "") -> Optional[CueReply]:
        """Add a reply to an existing cue."""
        cue = self.get_cue(cue_id)
        if cue is None:
            return None

        import hashlib
        reply_id = hashlib.sha256(f"{time.time()}{text}".encode()).hexdigest()[:10]
        reply = CueReply(
            id=reply_id,
            text=text,
            author=author or os.environ.get("USER", "unknown"),
            timestamp=time.time(),
            snapshot_hash=snapshot_hash,
        )
        cue.replies.append(reply)
        self._save_cue(cue)
        return reply

    def resolve(self, cue_id: str, note: str = "") -> Optional[Cue]:
        """Mark a cue as resolved."""
        cue = self.get_cue(cue_id)
        if cue is None:
            return None
        cue.status = "resolved"
        if note:
            cue.replies.append(CueReply(
                id="resolve",
                text=f"✅ Resolved: {note}",
                author="system",
                timestamp=time.time(),
            ))
        self._save_cue(cue)
        return cue

    def skip(self, cue_id: str, reason: str = "") -> Optional[Cue]:
        """Skip a cue (explicitly decide not to address it)."""
        cue = self.get_cue(cue_id)
        if cue is None:
            return None
        cue.status = "skipped"
        if reason:
            cue.replies.append(CueReply(
                id="skip",
                text=f"⏭ Skipped: {reason}",
                author="system",
                timestamp=time.time(),
            ))
        self._save_cue(cue)
        return cue

    def defer(self, cue_id: str, reason: str = "") -> Optional[Cue]:
        """Defer a cue to a later session."""
        cue = self.get_cue(cue_id)
        if cue is None:
            return None
        cue.status = "deferred"
        if reason:
            cue.replies.append(CueReply(
                id="defer",
                text=f"⏳ Deferred: {reason}",
                author="system",
                timestamp=time.time(),
            ))
        self._save_cue(cue)
        return cue

    def get_cue(self, cue_id: str) -> Optional[Cue]:
        """Load a cue by ID."""
        # Search both cue dirs (with and without prefix)
        for f in self.cues_dir.glob(f"{cue_id}*"):
            data = json.loads(f.read_text())
            return Cue(**data)
        
        # Full scan if prefix search failed
        for f in self.cues_dir.glob("*.json"):
            if f.stem == cue_id or f.stem.startswith(cue_id):
                data = json.loads(f.read_text())
                return Cue(**data)
        return None

    def list_cues(self, filter_: Optional[CueFilter] = None) -> list[Cue]:
        """List all cues, optionally filtered."""
        cues = []
        for f in sorted(self.cues_dir.glob("*.json")):
            data = json.loads(f.read_text())
            cue = Cue(**data)
            # Convert reply dicts to CueReply objects
            cue.replies = [CueReply(**r) if isinstance(r, dict) else r for r in cue.replies]

            if filter_:
                if filter_.status and cue.status != filter_.status:
                    continue
                if filter_.author and cue.author != filter_.author:
                    continue
                if filter_.snapshot and cue.snapshot_hash != filter_.snapshot:
                    continue
                if filter_.track and cue.track_name != filter_.track:
                    continue

            cues.append(cue)

        # Sort by timestamp (newest last)
        cues.sort(key=lambda c: c.timestamp)
        return cues

    def count_unresolved(self) -> int:
        """Count open/pending cues."""
        return len(self.list_cues(CueFilter(status="pending")))

    def _save_cue(self, cue: Cue) -> None:
        """Persist a cue to disk."""
        path = self.cues_dir / f"{cue.id}.json"
        path.write_text(json.dumps(asdict(cue), indent=2, default=str))


# ─── Cue Rendering (Ableton Marker Export) ─────────────────────────────

def render_cues_as_markers(cues: list[Cue], output_path: str) -> str:
    """
    Render unresolved cues as an Ableton-compatible marker file.
    
    Produces a small XML snippet that can be merged into the .als file's
    <CuePoints> section. This is a one-way, safe operation — the original
    .als is never touched.
    """
    import xml.etree.ElementTree as ET
    
    unresolved = [c for c in cues if c.status in ("pending", "deferred")]
    if not unresolved:
        return ""

    root_als = ET.Element("ClavusCueExport")
    root_als.set("version", "1")
    root_als.set("count", str(len(unresolved)))

    for cue in unresolved:
        cue_elem = ET.SubElement(root_als, "CuePoint")
        ET.SubElement(cue_elem, "Time").set("Value", cue.position)
        ET.SubElement(cue_elem, "Name").set("Value", f"💬 {cue.text[:60]}")

    # Export to a file
    xml_str = '<?xml version="1.0" encoding="UTF-8"?>\n'
    xml_str += ET.tostring(root_als, encoding="unicode")

    output = Path(output_path)
    output.write_text(xml_str)
    return str(output)


# ─── CLI Integration ───────────────────────────────────────────────────

def add_cue_command(text: str, position: str, track: str = "",
                    store: Optional[BlobStore] = None) -> Cue:
    """CLI-level cue addition with store lookup."""
    blobs = store or BlobStore()
    projects = blobs.list_projects()
    if not projects:
        print("❌ No Clavus project found. Run 'clavus init' first.")
        return None

    proj = projects[0]
    cues = CueStore(proj.name, store=blobs)
    head = blobs.read_ref("HEAD")

    cue = cues.add_cue(
        text=text,
        position=position,
        snapshot_hash=head or "",
        track_name=track,
    )

    return cue


def format_cue(cue: Cue, verbose: bool = False) -> str:
    """Format a single cue for display."""
    time_str = time.strftime("%m/%d %H:%M", time.localtime(cue.timestamp))
    status_icon = {
        "pending": "⏺",
        "resolved": "✅",
        "skipped": "⏭",
        "deferred": "⏳",
    }.get(cue.status, "•")

    lines = []
    lines.append(f"  {status_icon} @{cue.position}  {time_str}  {cue.author}")
    if cue.track_name:
        lines.append(f"     Track: {cue.track_name}")
    lines.append(f"     \"{cue.text}\"")
    lines.append(f"     [{cue.status}]  id: {cue.id}")

    if verbose and cue.replies:
        for reply in cue.replies:
            rtime = time.strftime("%m/%d %H:%M", time.localtime(reply.timestamp))
            lines.append(f"       └─ {reply.author} ({rtime}): {reply.text[:80]}")

    return "\n".join(lines)


def format_cue_list(cues: list[Cue], verbose: bool = False) -> str:
    """Format a list of cues for display."""
    if not cues:
        return "  💬 No cues."

    unresolved = [c for c in cues if c.status == "pending"]
    resolved = [c for c in cues if c.status == "resolved"]
    skipped = [c for c in cues if c.status == "skipped"]
    deferred = [c for c in cues if c.status == "deferred"]

    parts = []
    parts.append(f"💬 Cues ({len(unresolved)} pending, {len(resolved)} resolved, "
                 f"{len(skipped)} skipped, {len(deferred)} deferred)")

    if unresolved:
        parts.append(f"\n  ⏺ Pending ({len(unresolved)}):")
        for cue in unresolved:
            parts.append("")
            parts.append(format_cue(cue, verbose))

    if verbose and resolved:
        parts.append(f"\n  ✅ Resolved ({len(resolved)}):")
        for cue in resolved:
            parts.append("")
            parts.append(format_cue(cue, verbose))

    if verbose and skipped:
        parts.append(f"\n  ⏭ Skipped ({len(skipped)}):")
        parts.append(f"    Use 'clavus cues --all' to see skipped cues.")

    return "\n".join(parts)
