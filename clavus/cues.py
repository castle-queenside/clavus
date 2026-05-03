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
            cue = Cue(**data)
            cue.replies = [CueReply(**r) if isinstance(r, dict) else r for r in cue.replies]
            return cue

        # Full scan if prefix search failed
        for f in self.cues_dir.glob("*.json"):
            if f.stem == cue_id or f.stem.startswith(cue_id):
                data = json.loads(f.read_text())
                cue = Cue(**data)
                cue.replies = [CueReply(**r) if isinstance(r, dict) else r for r in cue.replies]
                return cue
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

    # ── Sync Import ──

    def import_cue(self, cue: Cue) -> None:
        """Import a cue from sync.

        If a cue with the same ID already exists and the existing one
        has a NEWER timestamp, skip (don't overwrite). Otherwise, overwrite.
        """
        existing = self.get_cue(cue.id)
        if existing is not None and existing.timestamp > cue.timestamp:
            return  # existing is newer — don't overwrite
        self._save_cue(cue)

    def import_reply(self, cue_id: str, reply: CueReply) -> bool:
        """Import a reply from sync.

        Only add the reply if it is not a duplicate — determined by
        matching both timestamp and author against existing replies.
        Returns True if the reply was added, False if duplicate or cue not found.
        """
        cue = self.get_cue(cue_id)
        if cue is None:
            return False

        # Check for duplicate (same timestamp + author)
        for existing_reply in cue.replies:
            if existing_reply.timestamp == reply.timestamp and existing_reply.author == reply.author:
                return False

        cue.replies.append(reply)
        self._save_cue(cue)
        return True

    def _save_cue(self, cue: Cue) -> None:
        """Persist a cue to disk."""
        path = self.cues_dir / f"{cue.id}.json"
        path.write_text(json.dumps(asdict(cue), indent=2, default=str))


# ─── Cue Rendering (Ableton Marker Export) ─────────────────────────────

def render_cues_as_markers(cues: list[Cue], output_path: str,
                            inject_into_als: str = "") -> str:
    """
    Render unresolved cues as Ableton-compatible markers.
    
    Two modes:
    1. Export to XML file — produce a <ClavusCueExport> wrapper file
    2. Inject into .als — merge cue markers directly into the project's <CuePoints>
    
    Injection mode creates a backup (.als.bak) before modifying if one doesn't exist.
    Only inserts new, unique markers — never duplicates existing ones.
    """
    import xml.etree.ElementTree as ET
    
    unresolved = [c for c in cues if c.status in ("pending", "deferred")]
    if not unresolved:
        return ""
    
    # ── Injection Mode ──
    if inject_into_als:
        import gzip
        import shutil
        
        als_path = Path(inject_into_als)
        if not als_path.exists():
            print(f"❌ .als file not found: {als_path}")
            return ""
        
        # Create backup on first injection (never overwrite existing backup)
        backup = als_path.with_suffix(".als.bak")
        if not backup.exists():
            shutil.copy2(als_path, backup)
        
        # Parse existing .als
        with gzip.open(als_path, "rb") as f:
            raw = f.read()
        root = ET.fromstring(raw)
        
        # Find the LiveSet
        if root.tag == "Ableton":
            live_set = root.find("LiveSet")
        else:
            live_set = root  # Live 9 format
        
        if live_set is None:
            print("❌ Could not find <LiveSet> in the .als file.")
            return ""
        
        live_set_parent = root if root.tag != "Ableton" else root
        
        # Detect Live 12+ (uses Locators) vs older (uses CuePoints)
        locators_container = live_set_parent.find(".//Locators")
        cue_wrapper = live_set_parent.find(".//CuePointsListWrapper")
        
        # Determine which format to use
        is_live_12 = locators_container is not None
        
        if is_live_12:
            # Live 12+ format: write into <Locators><Locators> as <Locator> elements
            # Find the inner Locators container (Locators > Locators)
            inner_locators = None
            for child in locators_container:
                if child.tag == "Locators":
                    inner_locators = child
                    break
            if inner_locators is None:
                inner_locators = ET.SubElement(locators_container, "Locators")
            
            target_container = inner_locators
            elem_tag = "Locator"
            existing_names = set()
            next_id = 0
            for existing in target_container.findall(elem_tag):
                existing_id = int(existing.get("Id", "0"))
                if existing_id >= next_id:
                    next_id = existing_id + 1
                name_elem = existing.find("Name")
                if name_elem is not None and name_elem.get("Value"):
                    existing_names.add(name_elem.get("Value"))
        else:
            # Live 10/11 format: use CuePoints
            cue_points = live_set_parent.find(".//CuePoints")
            if cue_points is None:
                cue_points = ET.SubElement(live_set, "CuePoints")
            
            target_container = cue_points
            elem_tag = "CuePoint"
            existing_names = set()
            for existing in target_container.findall(elem_tag):
                name_elem = existing.find("Name")
                if name_elem is not None and name_elem.get("Value"):
                    existing_names.add(name_elem.get("Value"))
        
        # Helper: convert bar.beat.sixteenth (e.g. "16.1.1") to quarter-note beats
        def position_to_beats(pos: str) -> int:
            parts = pos.replace("@", "").strip().split(".")
            if len(parts) >= 2:
                try:
                    bar = int(parts[0])
                    beat = int(parts[1])
                    return (bar - 1) * 4 + (beat - 1)
                except ValueError:
                    pass
            try:
                return int(float(pos.replace("@", "").strip()))
            except ValueError:
                return 0
        
        # Insert new markers
        inserted = 0
        for cue in unresolved:
            marker_name = f"💬 {cue.text[:60]}"
            if marker_name in existing_names:
                continue  # skip duplicates
            
            if is_live_12:
                # Live 12: Locator with Id, Time (quarter-note beats), Name, Annotation, IsSongStart
                locator = ET.SubElement(target_container, "Locator")
                locator.set("Id", str(next_id))
                next_id += 1
                ET.SubElement(locator, "LomId").set("Value", "0")
                beats = position_to_beats(cue.position)
                ET.SubElement(locator, "Time").set("Value", str(beats))
                ET.SubElement(locator, "Name").set("Value", marker_name)
                ET.SubElement(locator, "Annotation").set("Value", "")
                ET.SubElement(locator, "IsSongStart").set("Value", "false")
            else:
                # Live 10/11: CuePoint with Time (bar.beat notation) and Name
                cue_elem = ET.SubElement(target_container, elem_tag)
                ET.SubElement(cue_elem, "Time").set("Value", cue.position)
                ET.SubElement(cue_elem, "Name").set("Value", marker_name)
            
            existing_names.add(marker_name)
            inserted += 1
        
        if inserted == 0:
            print("📍 All cues already present in the project — nothing to inject.")
            return ""
        
        # Write back the modified .als preserving original XML declaration format.
        # Ableton is picky about the XML header format (double quotes vs single quotes).
        # Extract the original declaration from the raw bytes to preserve it exactly.
        original_bytes = raw
        xml_start = original_bytes.find(b"<?xml")
        decl_end = original_bytes.find(b"?>", xml_start) + 2 if xml_start >= 0 else 0
        
        # Serialize modified tree without xml_declaration (we'll use the original)
        tree_xml = ET.tostring(root, encoding="unicode")
        
        if decl_end > 0:
            # Keep original declaration, discard the tree's auto-generated one (if any)
            original_header = original_bytes[:decl_end].decode("utf-8", errors="replace")
            # If tree_xml has its own declaration, skip it
            if tree_xml.startswith("<?xml"):
                body_start = tree_xml.find("?>") + 2
                tree_body = tree_xml[body_start:]
            else:
                tree_body = tree_xml
            new_content = original_header + tree_body
        else:
            new_content = tree_xml
        
        with gzip.open(als_path, "wt", encoding="utf-8") as f:
            f.write(new_content)
        
        print(f"📍 Injected {inserted} cue(s) as markers into {als_path.name}")
        if backup.exists():
            print(f"   Backup saved as {backup.name}")
        return str(als_path)
    
    # ── Export Mode (original behavior) ──
    root_als = ET.Element("ClavusCueExport")
    root_als.set("version", "1")
    root_als.set("count", str(len(unresolved)))

    for cue in unresolved:
        cue_elem = ET.SubElement(root_als, "CuePoint")
        ET.SubElement(cue_elem, "Time").set("Value", cue.position)
        ET.SubElement(cue_elem, "Name").set("Value", f"💬 {cue.text[:60]}")

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
