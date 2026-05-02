"""
Clavus — snapshot engine.

Content-addressed blob storage for Ableton project states.

Snapshots work like Git commits:
  - Each snapshot has a content hash (SHA256 of serialized project state)
  - Snapshots form a DAG via parent references
  - Tags reference snapshots by name (like "arrangement pass 3")
  - The manifest stores the complete history

Storage layout (~/.clavus/):
  objects/  ── content-addressed blobs (prefix/sha256)
  refs/     ── named references (tags, branches, HEAD)
  index     ── current working state
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import time
from dataclasses import dataclass, field, asdict
from pathlib import Path
from typing import Optional, Any

from clavus.parser import Project

# ─── Config ──────────────────────────────────────────────────────────────

DEFAULT_CLAVUS_DIR = Path.home() / ".clavus"
OBJECTS_DIR = "objects"
REFS_DIR = "refs"
INDEX_FILE = "index.json"
CONFIG_FILE = "config.json"


# ─── Data Model ─────────────────────────────────────────────────────────

@dataclass
class Snapshot:
    """A single snapshot of an Ableton project state."""
    hash: str  # SHA256 of the serialized project
    timestamp: float  # Unix timestamp
    message: str  # User-provided message (e.g., "arrangement pass 3")
    parent: Optional[str] = None  # Parent snapshot hash (None for first)
    project_path: str = ""  # Path to the .als file at snapshot time
    track_count: int = 0
    bpm: float = 120.0
    tags: list[str] = field(default_factory=list)  # User tags

    def short_hash(self, length: int = 8) -> str:
        return self.hash[:length]


@dataclass
class ClavusProject:
    """Metadata about a tracked Ableton project."""
    name: str
    root_als: str  # Path to the .als file
    created_at: float
    head: Optional[str] = None  # Current snapshot hash
    branch: str = "main"
    sync_url: str = ""  # Remote sync address (for Phase 5+)


# ─── Storage ────────────────────────────────────────────────────────────

class BlobStore:
    """Content-addressed storage for serialized project snapshots."""

    def __init__(self, clavus_dir: Path = DEFAULT_CLAVUS_DIR):
        self.root = clavus_dir
        self.objects_dir = self.root / OBJECTS_DIR
        self.refs_dir = self.root / REFS_DIR
        self.index_path = self.root / INDEX_FILE
        self.config_path = self.root / CONFIG_FILE

    def init(self) -> None:
        """Initialize the Clavus storage directory."""
        self.root.mkdir(parents=True, exist_ok=True)
        self.objects_dir.mkdir(exist_ok=True)
        self.refs_dir.mkdir(exist_ok=True)
        if not self.index_path.exists():
            self._write_json(self.index_path, {})
        if not self.config_path.exists():
            self._write_json(self.config_path, {
                "version": 1,
                "created_at": time.time(),
            })
        print(f"📁 Initialized Clavus repository at {self.root}")

    # ── Object Storage ──

    def put_object(self, data: bytes, hash_str: Optional[str] = None) -> str:
        """Store content-addressed blob. Returns hash."""
        if hash_str is None:
            hash_str = hashlib.sha256(data).hexdigest()

        obj_dir = self.objects_dir / hash_str[:2]
        obj_path = obj_dir / hash_str

        if not obj_path.exists():
            obj_dir.mkdir(parents=True, exist_ok=True)
            obj_path.write_bytes(data)

        return hash_str

    def get_object(self, hash_str: str) -> Optional[bytes]:
        """Retrieve blob by SHA256 hash."""
        obj_path = self.objects_dir / hash_str[:2] / hash_str
        if obj_path.exists():
            return obj_path.read_bytes()
        return None

    def has_object(self, hash_str: str) -> bool:
        """Check if a blob exists."""
        obj_path = self.objects_dir / hash_str[:2] / hash_str
        return obj_path.exists()

    # ── Snapshot Storage ──

    def save_snapshot(self, project: Project, message: str,
                      parent: Optional[str] = None, tags: list[str] | None = None) -> Snapshot:
        """Serialize a project, hash it, and store as a snapshot."""
        # Serialize the project to JSON
        project_data = self._project_to_dict(project)
        serialized = json.dumps(project_data, sort_keys=True, default=str).encode("utf-8")

        # Content-address: hash the serialized project
        content_hash = hashlib.sha256(serialized).hexdigest()

        # Store the content
        self.put_object(serialized, content_hash)

        # Create the snapshot metadata
        snapshot = Snapshot(
            hash=content_hash,
            timestamp=time.time(),
            message=message,
            parent=parent,
            project_path=str(project.file_path) if project.file_path else "",
            track_count=project.track_count,
            bpm=project.bpm,
            tags=tags or [],
        )

        # Store snapshot metadata (indexed by hash)
        self._write_snapshot_meta(snapshot)

        return snapshot

    def load_snapshot(self, hash_str: str) -> Optional[Snapshot]:
        """Load snapshot metadata by hash."""
        meta_path = self.objects_dir / hash_str[:2] / f"{hash_str}.meta"
        if not meta_path.exists():
            return None
        data = json.loads(meta_path.read_text())
        return Snapshot(**data)

    def load_project(self, hash_str: str) -> Optional[Project]:
        """Deserialize a Project from a snapshot hash."""
        data = self.get_object(hash_str)
        if data is None:
            return None
        project_dict = json.loads(data)
        return self._dict_to_project(project_dict)

    def _write_snapshot_meta(self, snapshot: Snapshot) -> None:
        """Write snapshot metadata alongside the content blob."""
        meta_dir = self.objects_dir / snapshot.hash[:2]
        meta_path = meta_dir / f"{snapshot.hash}.meta"
        meta_path.write_text(json.dumps(asdict(snapshot), indent=2, default=str))

    # ── Reference Management (tags / branches / HEAD) ──

    def update_ref(self, ref_name: str, hash_str: str) -> None:
        """Update a named reference to point to a snapshot."""
        ref_path = self.refs_dir / ref_name
        ref_path.parent.mkdir(parents=True, exist_ok=True)
        ref_path.write_text(hash_str)

    def read_ref(self, ref_name: str) -> Optional[str]:
        """Read what a named reference points to."""
        ref_path = self.refs_dir / ref_name
        if ref_path.exists():
            return ref_path.read_text().strip()
        return None

    def delete_ref(self, ref_name: str) -> None:
        """Delete a named reference."""
        ref_path = self.refs_dir / ref_name
        if ref_path.exists():
            ref_path.unlink()

    # ── Index (Active Project) ──

    def set_index(self, project: ClavusProject) -> None:
        """Set the active tracked project in the index."""
        index = json.loads(self.index_path.read_text()) if self.index_path.exists() else {}
        index[project.name] = asdict(project)
        self._write_json(self.index_path, index)

    def get_index(self, name: str) -> Optional[ClavusProject]:
        """Get an active project from the index."""
        if not self.index_path.exists():
            return None
        index = json.loads(self.index_path.read_text())
        data = index.get(name)
        if data:
            return ClavusProject(**data)
        return None

    def list_projects(self) -> list[ClavusProject]:
        """List all tracked projects."""
        if not self.index_path.exists():
            return []
        index = json.loads(self.index_path.read_text())
        return [ClavusProject(**data) for data in index.values()]

    @staticmethod
    def _write_json(path: Path, data: dict) -> None:
        path.write_text(json.dumps(data, indent=2, default=str))

    # ── Serialization Helpers ──

    @staticmethod
    def _project_to_dict(project: Project) -> dict:
        """Convert a Project to a JSON-serializable dict."""
        return {
            "ableton_version": project.ableton_version,
            "schema_version": project.schema_version,
            "session_id": project.session_id,
            "bpm": project.bpm,
            "time_signature": project.time_signature,
            "tracks": [
                {
                    "name": t.name,
                    "track_type": t.track_type,
                    "index": t.index,
                    "color": t.color,
                    "is_frozen": t.is_frozen,
                    "is_muted": t.is_muted,
                    "is_solo": t.is_solo,
                    "devices": [
                        {"name": d.name, "device_type": d.device_type}
                        for d in t.devices
                    ],
                    "sends": dict(t.sends),
                }
                for t in project.tracks
            ],
            "return_tracks": [
                {
                    "name": t.name,
                    "track_type": "Return",
                    "devices": [
                        {"name": d.name, "device_type": d.device_type}
                        for d in t.devices
                    ],
                }
                for t in project.return_tracks
            ],
            "master_track": {
                "name": project.master_track.name if project.master_track else "",
                "devices": [
                    {"name": d.name, "device_type": d.device_type}
                    for d in (project.master_track.devices if project.master_track else [])
                ],
            } if project.master_track else None,
            "markers": [
                {"time": m.time, "name": m.name}
                for m in project.markers
            ],
        }

    @staticmethod
    def _dict_to_project(data: dict) -> Project:
        """Reconstruct a Project from a serialized dict."""
        from clavus.parser import Project, Track, Marker, Device, TempoEvent

        project = Project(
            ableton_version=data.get("ableton_version", ""),
            schema_version=data.get("schema_version", ""),
            session_id=data.get("session_id", ""),
            bpm=data.get("bpm", 120.0),
            time_signature=data.get("time_signature", "4/4"),
        )

        for td in data.get("tracks", []):
            track = Track(
                name=td.get("name", "Unnamed"),
                track_type=td.get("track_type", "Audio"),
                index=td.get("index", 0),
                color=td.get("color", 16777215),
                is_frozen=td.get("is_frozen", False),
                is_muted=td.get("is_muted", False),
                is_solo=td.get("is_solo", False),
                devices=[Device(**d) for d in td.get("devices", [])],
                sends=td.get("sends", {}),
            )
            project.tracks.append(track)

        for td in data.get("return_tracks", []):
            track = Track(
                name=td.get("name", "Return"),
                track_type="Return",
                devices=[Device(**d) for d in td.get("devices", [])],
            )
            project.return_tracks.append(track)

        mt = data.get("master_track")
        if mt:
            project.master_track = Track(
                name=mt.get("name", "Master"),
                track_type="Master",
                devices=[Device(**d) for d in mt.get("devices", [])],
            )

        for md in data.get("markers", []):
            project.markers.append(Marker(time=md.get("time", "0"), name=md.get("name", "")))

        return project


# ─── Diff Engine ────────────────────────────────────────────────────────

@dataclass
class TrackDiff:
    """What changed between two snapshot states for a single track."""
    name: str
    status: str  # "added", "removed", "modified", "unchanged"
    devices_added: list[str] = field(default_factory=list)
    devices_removed: list[str] = field(default_factory=list)
    frozen_changed: Optional[bool] = None
    mute_changed: Optional[bool] = None
    solo_changed: Optional[bool] = None
    name_changed: Optional[str] = None
    color_changed: Optional[int] = None


@dataclass
class ProjectDiff:
    """Full diff between two project snapshots."""
    before_hash: str
    after_hash: str
    bpm_changed: Optional[tuple[float, float]] = None
    track_count_changed: Optional[tuple[int, int]] = None
    tracks: list[TrackDiff] = field(default_factory=list)
    markers_added: list[str] = field(default_factory=list)
    markers_removed: list[str] = field(default_factory=list)
    summary: str = ""


def diff_projects(before: Project, after: Project) -> ProjectDiff:
    """Compare two project states and generate a structured diff."""
    diff = ProjectDiff(
        before_hash="",
        after_hash="",
    )

    # BPM change
    if before.bpm != after.bpm:
        diff.bpm_changed = (before.bpm, after.bpm)

    # Track count
    if before.track_count != after.track_count:
        diff.track_count_changed = (before.track_count, after.track_count)

    # Track-level diffs
    before_tracks = {t.name: t for t in before.tracks}
    after_tracks = {t.name: t for t in after.tracks}
    all_track_names = set(list(before_tracks.keys()) + list(after_tracks.keys()))

    for name in sorted(all_track_names):
        bt = before_tracks.get(name)
        at = after_tracks.get(name)

        if bt and not at:
            diff.tracks.append(TrackDiff(name=name, status="removed"))
        elif at and not bt:
            diff.tracks.append(TrackDiff(name=name, status="added"))
        elif bt and at:
            changes = []
            devices_added = []
            devices_removed = []

            # Device diff
            before_devs = {d.device_type: d for d in bt.devices}
            after_devs = {d.device_type: d for d in at.devices}
            all_devs = set(list(before_devs.keys()) + list(after_devs.keys()))

            for d in all_devs:
                if d in after_devs and d not in before_devs:
                    devices_added.append(d)
                elif d in before_devs and d not in after_devs:
                    devices_removed.append(d)

            frozen_changed = None
            if bt.is_frozen != at.is_frozen:
                frozen_changed = at.is_frozen

            mute_changed = None
            if bt.is_muted != at.is_muted:
                mute_changed = at.is_muted

            if devices_added or devices_removed or frozen_changed is not None or mute_changed is not None:
                diff.tracks.append(TrackDiff(
                    name=name,
                    status="modified",
                    devices_added=devices_added,
                    devices_removed=devices_removed,
                    frozen_changed=frozen_changed,
                    mute_changed=mute_changed,
                ))
            else:
                diff.tracks.append(TrackDiff(name=name, status="unchanged"))
        else:
            diff.tracks.append(TrackDiff(name=name, status="unchanged"))

    # Marker diff
    before_markers = {m.name: m for m in before.markers}
    after_markers = {m.name: m for m in after.markers}
    diff.markers_added = [m for m in after_markers if m not in before_markers]
    diff.markers_removed = [m for m in before_markers if m not in after_markers]

    # Generate summary
    summary_parts = []
    if diff.bpm_changed:
        summary_parts.append(f"BPM: {diff.bpm_changed[0]}→{diff.bpm_changed[1]}")
    if diff.track_count_changed:
        summary_parts.append(f"Tracks: {diff.track_count_changed[0]}→{diff.track_count_changed[1]}")

    modified = [t for t in diff.tracks if t.status == "modified"]
    added = [t for t in diff.tracks if t.status == "added"]
    removed = [t for t in diff.tracks if t.status == "removed"]

    if modified:
        summary_parts.append(f"Modified: {', '.join(t.name for t in modified[:5])}")
        if len(modified) > 5:
            summary_parts[-1] += f" (+{len(modified) - 5} more)"
    if added:
        summary_parts.append(f"Added: {', '.join(t.name for t in added[:3])}")
    if removed:
        summary_parts.append(f"Removed: {', '.join(t.name for t in removed[:3])}")
    if diff.markers_added:
        summary_parts.append(f"Markers: +{len(diff.markers_added)}")
    if diff.markers_removed:
        summary_parts.append(f"Markers: -{len(diff.markers_removed)}")

    diff.summary = " · ".join(summary_parts) if summary_parts else "No changes"
    return diff


def format_diff(diff: ProjectDiff, verbose: bool = False) -> str:
    """Format a ProjectDiff as a human-readable string."""
    lines = []
    lines.append(f"  {diff.summary}")
    lines.append("")

    for td in diff.tracks:
        if td.status == "unchanged" and not verbose:
            continue

        icon = {"added": "➕", "removed": "✂️", "modified": "🔄", "unchanged": "  "}.get(td.status, "•")
        lines.append(f"  {icon} {td.name}")

        if td.devices_added:
            lines.append(f"       Added: {', '.join(td.devices_added)}")
        if td.devices_removed:
            lines.append(f"       Removed: {', '.join(td.devices_removed)}")
        if td.frozen_changed is not None:
            lines.append(f"       Frozen: {'yes' if td.frozen_changed else 'no'}")
        if td.mute_changed is not None:
            lines.append(f"       Muted: {'yes' if td.mute_changed else 'no'}")

    if diff.markers_added:
        lines.append(f"\n  📍 Markers added: {', '.join(diff.markers_added[:5])}")
    if diff.markers_removed:
        lines.append(f"  🗑 Markers removed: {', '.join(diff.markers_removed[:3])}")

    return "\n".join(lines)
