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

DEFAULT_CLAVUS_DIR = Path(os.environ.get("CLAVUS_DIR", str(Path.home() / ".clavus")))
OBJECTS_DIR = "objects"
REFS_DIR = "refs"
INDEX_FILE = "index.json"
CONFIG_FILE = "config.json"


# ─── Data Model ─────────────────────────────────────────────────────────

@dataclass
class Snapshot:
    """A single snapshot of an Ableton project state."""
    hash: str  # SHA256 of the raw .als file (snapshot identity)
    timestamp: float  # Unix timestamp
    message: str  # User-provided message (e.g., "arrangement pass 3")
    parent: Optional[str] = None  # Parent snapshot hash (None for first)
    project_path: str = ""  # Path to the .als file at snapshot time
    track_count: int = 0
    bpm: float = 120.0
    tags: list[str] = field(default_factory=list)  # User tags
    als_hash: Optional[str] = None  # DEPRECATED: now same as .hash (kept for compat)
    content_hash: Optional[str] = None  # SHA256 of serialized project JSON (for diff)
    sample_hashes: list[str] = field(default_factory=list)  # SHA256 of referenced audio samples
    sample_paths: dict[str, str] = field(default_factory=dict)  # hash → relative path from project root
    conflict_message: Optional[str] = None  # Remote message that conflicts with local

    def short_hash(self, length: int = 8) -> str:
        return self.hash[:length]


@dataclass
class ClavusProject:
    """Metadata about a tracked Ableton project."""
    name: str
    root_als: str  # Path to the .als file
    created_at: float
    head: Optional[str] = None  # Current snapshot hash
    description: str = ""  # Optional human-readable notes
    branch: str = "main"
    sync_url: str = ""  # Legacy — kept for backward compat with old indexes
    active_remote: str = ""  # Selected remote for push/pull (by name)
    last_remote_head: str = ""  # Relay HEAD last seen for THIS project (optimistic lock)


@dataclass
class StemEntry:
    """A single stem file tracked in a snapshot."""
    track_name: str           # e.g., "Kick", "Bass", "Vocal"
    file_name: str            # e.g., "Kick.wav"
    hash: str                 # SHA256 of the audio file content
    size: int = 0             # File size in bytes
    format: str = "wav"       # Audio format
    sample_rate: int = 44100
    bit_depth: int = 24
    channels: int = 2
    duration_seconds: float = 0.0
    bounced_at: float = 0.0   # Timestamp when the stem was bounced


@dataclass
class StemManifest:
    """Mapping of which stem blobs belong to a snapshot."""
    snapshot_hash: str
    stems: list[StemEntry] = field(default_factory=list)
    created_at: float = 0.0


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
        """Initialize the Clavus storage directory.

        Safe to call multiple times — will not overwrite existing data.
        """
        self.root.mkdir(parents=True, exist_ok=True)
        self.objects_dir.mkdir(exist_ok=True)
        self.refs_dir.mkdir(exist_ok=True)

        # Never overwrite an existing healthy index — only restore from backup if missing
        if not self.index_path.exists():
            if not self._try_restore_index():
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

    def _resolve_object_hash(self, hash_str: str) -> Optional[str]:
        """Resolve a short (8-char) hash to the full 64-char filename."""
        if not hash_str:
            return None
        if len(hash_str) >= 64:
            return hash_str
        prefix_dir = self.objects_dir / hash_str[:2]
        if not prefix_dir.exists():
            return None
        for f in prefix_dir.iterdir():
            # Skip .meta files
            if f.suffix == ".meta":
                continue
            if f.name.startswith(hash_str):
                return f.name
        return None

    def get_object(self, hash_str: str) -> Optional[bytes]:
        """Retrieve blob by SHA256 hash (full or short)."""
        full = self._resolve_object_hash(hash_str)
        if not full:
            return None
        obj_path = self.objects_dir / full[:2] / full
        if obj_path.exists():
            return obj_path.read_bytes()
        return None

    def has_object(self, hash_str: str) -> bool:
        """Check if a blob exists (full or short hash)."""
        full = self._resolve_object_hash(hash_str)
        if not full:
            return False
        obj_path = self.objects_dir / full[:2] / full
        return obj_path.exists()

    # ── Sample Storage ──

    def store_sample(self, file_path: str | Path, relative_path: str = "") -> tuple[str, str]:
        """Hash an audio sample and store with filename metadata.
        Returns (sha256_hash, original_filename)."""
        import hashlib
        fp = Path(file_path)
        data = fp.read_bytes()
        h = hashlib.sha256(data).hexdigest()
        self.put_object(data, h)
        # Store filename + optional relative path alongside blob
        meta_path = self.objects_dir / h[:2] / f"{h}.sample"
        # Normalize backslashes to forward slashes for cross-OS compat
        relative_path = relative_path.replace("\\", "/")
        if relative_path:
            meta_path.write_text(f"{fp.name}\n{relative_path}")
        else:
            meta_path.write_text(fp.name)
        return h, fp.name

    def materialize_sample(self, sample_hash: str, out_dir: Path, filename: str, relpath: str = "") -> Path:
        """Write a sample blob to a directory, preserving subdirectory structure.
        Returns the output path."""
        data = self.get_object(sample_hash)
        if not data:
            raise FileNotFoundError(f"Sample blob not found: {sample_hash[:12]}")
        # If relpath is provided, preserve the subdirectory structure
        # e.g. relpath="Samples/Processed/Freeze/file.wav" → out_dir/Samples/Processed/Freeze/file.wav
        # Normalize backslashes (Windows paths) to forward slashes for cross-OS compat
        if relpath:
            relpath = relpath.replace("\\", "/")
            parent = Path(relpath).parent
            full_dir = out_dir / parent
        else:
            full_dir = out_dir
        full_dir.mkdir(parents=True, exist_ok=True)
        out_path = full_dir / filename
        out_path.write_bytes(data)
        return out_path

    def get_sample_filename(self, sample_hash: str) -> Optional[str]:
        """Get the original filename for a stored sample."""
        meta_path = self.objects_dir / sample_hash[:2] / f"{sample_hash}.sample"
        if meta_path.exists():
            content = meta_path.read_text().strip()
            return content.split("\n")[0]  # First line is filename
        return None

    def get_sample_relpath(self, sample_hash: str) -> Optional[str]:
        """Get the relative path (from project root) for a stored sample."""
        meta_path = self.objects_dir / sample_hash[:2] / f"{sample_hash}.sample"
        if meta_path.exists():
            content = meta_path.read_text().strip()
            lines = content.split("\n")
            if len(lines) > 1:
                # Normalize backslashes to forward slashes for cross-OS compat
                return lines[1].replace("\\", "/")  # Second line is relative path
        return None

    # ── Helpers ──

    @staticmethod
    def _write_json(path: Path, data: dict) -> None:
        """Write JSON data to a file atomically via rename.

        Writes to a .tmp file first, then atomically renames to the
        target. This prevents corruption if the process is killed or
        crashes mid-write. The .tmp is created in the same directory
        as the target so the rename is always on the same filesystem.
        """
        tmp = path.with_suffix(path.suffix + ".tmp")
        tmp.write_text(json.dumps(data, indent=2, default=str))
        tmp.rename(path)

    # ── Snapshot Storage ──

    def save_snapshot(self, project: Project, message: str,
                      parent: Optional[str] = None, tags: list[str] | None = None) -> Snapshot:
        """Serialize a project, hash it, and store as a snapshot.

        Snapshot identity = SHA256 of raw .als bytes. ANY save in Ableton
        that changes the file produces a new snapshot. Parsed project JSON
        is still stored (content-addressed) for diff/comparison."""
        # Serialize the project to JSON (for diff display)
        project_data = _project_to_dict(project)
        serialized = json.dumps(project_data, sort_keys=True, default=str).encode("utf-8")
        content_hash = hashlib.sha256(serialized).hexdigest()
        self.put_object(serialized, content_hash)

        # Raw .als bytes — THIS is the snapshot identity
        als_hash = None
        if project.file_path and Path(project.file_path).exists():
            raw_als = Path(project.file_path).read_bytes()
            als_hash = hashlib.sha256(raw_als).hexdigest()
            self.put_object(raw_als, als_hash)

        # Store referenced audio samples
        sample_hashes: list[str] = []
        sample_paths: dict[str, str] = {}
        try:
            from clavus.parser import extract_sample_paths
            sample_path_list = extract_sample_paths(project.file_path)
            project_root = Path(project.file_path).parent
            for sp in sample_path_list:
                if Path(sp).exists():
                    try:
                        rel = str(Path(sp).relative_to(project_root))
                    except ValueError:
                        rel = Path(sp).name
                    sh, _ = self.store_sample(sp, relative_path=rel)
                    sample_hashes.append(sh)
                    sample_paths[sh] = rel
        except Exception:
            pass  # Sample extraction is best-effort

        # Create the snapshot metadata — identity = raw .als hash
        snapshot = Snapshot(
            hash=als_hash or content_hash,  # fallback for edge cases
            timestamp=time.time(),
            message=message,
            parent=parent,
            project_path=str(project.file_path) if project.file_path else "",
            track_count=project.track_count,
            bpm=project.bpm,
            tags=tags or [],
            als_hash=als_hash,
            content_hash=content_hash,
            sample_hashes=sample_hashes,
            sample_paths=sample_paths,
        )

        # Store snapshot metadata (indexed by hash).
        # Only write if meta doesn't exist yet — with als_hash as identity,
        # duplicate snapshots have the same hash, and overwriting would
        # trigger self-referencing parent protection (parent == hash).
        meta_path = self.objects_dir / snapshot.hash[:2] / f"{snapshot.hash}.meta"
        if not meta_path.exists():
            self._write_snapshot_meta(snapshot)

        return snapshot

    def load_snapshot(self, hash_str: str) -> Optional[Snapshot]:
        """Load snapshot metadata by hash."""
        meta_path = self.objects_dir / hash_str[:2] / f"{hash_str}.meta"
        if not meta_path.exists():
            return None
        try:
            data = json.loads(meta_path.read_text())
            return Snapshot(**data)
        except (TypeError, json.JSONDecodeError):
            return None  # corrupted or partial meta file

    def load_project(self, hash_str: str) -> Optional[Project]:
        """Deserialize a Project from a snapshot hash.

        Snapshot hash is now the raw .als SHA256. The parsed JSON is stored
        under content_hash (found in snapshot metadata). For backward compat,
        also try loading the hash directly as a content blob."""
        # Try loading via snapshot metadata first (new format: hash = als_hash)
        snap = self.load_snapshot(hash_str)
        if snap and snap.content_hash:
            data = self.get_object(snap.content_hash)
            if data:
                project_dict = json.loads(data)
                return _dict_to_project(project_dict)
        # Fallback: old format (hash was content_hash directly)
        data = self.get_object(hash_str)
        if data:
            project_dict = json.loads(data)
            return _dict_to_project(project_dict)
        return None

    def _write_snapshot_meta(self, snapshot: Snapshot) -> None:
        """Write snapshot metadata alongside the content blob."""
        # Safety: prevent self-referencing parent (causes infinite history walks)
        if snapshot.parent == snapshot.hash:
            snapshot.parent = None
        meta_dir = self.objects_dir / snapshot.hash[:2]
        meta_path = meta_dir / f"{snapshot.hash}.meta"
        meta_path.write_text(json.dumps(asdict(snapshot), indent=2, default=str))

    def repair_snapshot(self, hash_str: str) -> bool:
        """Check a snapshot for self-referencing parent and fix it.
        Returns True if the snapshot was repaired."""
        import json
        meta_path = self.objects_dir / hash_str[:2] / f"{hash_str}.meta"
        if not meta_path.exists():
            return False
        data = json.loads(meta_path.read_text())
        if data.get("parent") == hash_str:
            data["parent"] = None
            meta_path.write_text(json.dumps(data, indent=2, default=str))
            return True
        return False

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

    # ── Index Backup & Recovery ──

    def _backup_index(self) -> None:
        """Rotating backup of index.json before each write + daily full store backup.

        Keeps the last 3 index backups as index.json.bak, .bak2, .bak3.
        Also creates a full store backup (tar.gz) once per day.
        """
        if not self.index_path.exists():
            return
        import shutil
        # Rotate: .bak2 → .bak3, .bak → .bak2, current → .bak
        for dst, src in [(".bak3", ".bak2"), (".bak2", ".bak")]:
            src_path = self.index_path.with_suffix(self.index_path.suffix + src)
            dst_path = self.index_path.with_suffix(self.index_path.suffix + dst)
            if src_path.exists():
                shutil.copy2(src_path, dst_path)
        bak_path = self.index_path.with_suffix(self.index_path.suffix + ".bak")
        shutil.copy2(self.index_path, bak_path)

        # Daily full store backup (only if none exists for today)
        day_str = time.strftime("%Y%m%d")
        daily_backup = self.root / "backups" / f"clavus-auto-{day_str}.tar.gz"
        if not daily_backup.exists():
            try:
                self.backup_store(daily_backup)
            except Exception:
                pass  # Non-critical — don't crash writes on backup failure

    def backup_store(self, archive_path: Path | None = None) -> Path:
        """Create a full backup of the entire Clavus store.

        Archives all of: index.json, cues/, objects/, refs/, config.json
        into a single .tar.gz file. Returns the path to the archive.

        Args:
            archive_path: Target path for the backup (default: ~/.clavus/backups/clavus-<date>.tar.gz)
        """
        import tarfile

        backup_dir = self.root / "backups"
        backup_dir.mkdir(parents=True, exist_ok=True)

        if archive_path is None:
            date_str = time.strftime("%Y%m%d_%H%M%S")
            archive_path = backup_dir / f"clavus-{date_str}.tar.gz"

        with tarfile.open(archive_path, "w:gz") as tar:
            # Add index.json
            if self.index_path.exists():
                tar.add(self.index_path, arcname="index.json")
            # Add config
            config_path = self.root / "config.json"
            if config_path.exists():
                tar.add(config_path, arcname="config.json")
            # Add cues
            cues_path = self.root / "cues"
            if cues_path.exists():
                for cue_file in cues_path.rglob("*.json"):
                    tar.add(cue_file, arcname=str(cue_file.relative_to(self.root)))
            # Add objects
            obj_path = self.root / "objects"
            if obj_path.exists():
                for obj_file in obj_path.rglob("*"):
                    if obj_file.is_file():
                        tar.add(obj_file, arcname=str(obj_file.relative_to(self.root)))
            # Add refs
            refs_path = self.root / "refs"
            if refs_path.exists():
                for ref_file in refs_path.rglob("*"):
                    if ref_file.is_file():
                        tar.add(ref_file, arcname=str(ref_file.relative_to(self.root)))

        return archive_path

    def restore_store(self, archive_path: Path) -> bool:
        """Restore the entire Clavus store from a backup archive.

        Extracts all files from a .tar.gz backup into the store root.
        Does NOT clear existing data — newer files overwrite older ones.

        Args:
            archive_path: Path to a .tar.gz backup file

        Returns:
            True if restore succeeded
        """
        import tarfile

        if not archive_path.exists():
            print(f"❌ Backup not found: {archive_path}")
            return False

        if not archive_path.suffix == ".gz" and not archive_path.name.endswith(".tar.gz"):
            print(f"❌ Not a valid backup archive: {archive_path}")
            return False

        extracted = 0
        with tarfile.open(archive_path, "r:gz") as tar:
            for member in tar.getmembers():
                if member.isfile():
                    tar.extract(member, path=self.root)
                    extracted += 1

        print(f"📦 Restored {extracted} file(s) from {archive_path.name}")
        return True

    def list_backups(self) -> list[Path]:
        """List all available backup archives."""
        backup_dir = self.root / "backups"
        if not backup_dir.exists():
            return []
        return sorted(backup_dir.glob("*.tar.gz"), reverse=True)

    def _try_restore_index(self) -> bool:
        """Try to restore index.json from backup or scan refs/cues dirs.
        Returns True if restored, False if nothing to recover.
        """
        import json, time

        # Level 1: restore from .bak file (also handles corrupt/truncated index.json)
        for bak_suffix in [".bak", ".bak2", ".bak3"]:
            bak_path = self.index_path.with_suffix(self.index_path.suffix + bak_suffix)
            if bak_path.exists():
                try:
                    data = json.loads(bak_path.read_text())
                    if isinstance(data, dict) and any(
                        isinstance(v, dict) and "root_als" in v for v in data.values()
                    ):
                        self._write_json(self.index_path, data)
                        print(f"⚠️  index.json restored from {bak_path.name}")
                        return True
                except (json.JSONDecodeError, OSError):
                    continue

        # Level 1b: index.json exists but is corrupt — overwrite from .bak if valid
        if self.index_path.exists():
            try:
                data = json.loads(self.index_path.read_text())
                # Valid if it has the expected structure
                if isinstance(data, dict) and any(
                    isinstance(v, dict) and "root_als" in v for v in data.values()
                ):
                    pass  # index is valid, no restore needed
                else:
                    raise json.JSONDecodeError("index.json missing project data", "", 0)
            except (json.JSONDecodeError, OSError):
                # Corrupt — try .bak
                for bak_suffix in [".bak", ".bak2", ".bak3"]:
                    bak_path = self.index_path.with_suffix(self.index_path.suffix + bak_suffix)
                    if bak_path.exists():
                        try:
                            data = json.loads(bak_path.read_text())
                            if isinstance(data, dict) and any(
                                isinstance(v, dict) and "root_als" in v for v in data.values()
                            ):
                                self._write_json(self.index_path, data)
                                print(f"⚠️  index.json was corrupt — restored from {bak_path.name}")
                                return True
                        except (json.JSONDecodeError, OSError):
                            continue
                # No valid backup — Level 2 will rebuild from refs
                print(f"⚠️  index.json is corrupt and no backup found — rebuilding from refs")

        # Level 2: reconstruct from refs/ directory
        ref_files = list(self.refs_dir.glob("**/*"))
        if ref_files:
            projects = {}
            head_hash = self.read_ref("HEAD")
            # Scan for refs/heads/* and refs/tags/*
            for ref_file in ref_files:
                if ref_file.is_file():
                    ref_name = str(ref_file.relative_to(self.refs_dir))
                    ref_value = ref_file.read_text().strip()

            # Try to find cues dirs as project names
            cues_root = self.root / "cues"
            if cues_root.exists():
                for proj_dir in sorted(cues_root.iterdir()):
                    if proj_dir.is_dir() and not proj_dir.name.startswith("."):
                        cue_files = list(proj_dir.glob("*.json"))
                        if cue_files:
                            proj = ClavusProject(
                                name=proj_dir.name,
                                root_als="",
                                created_at=time.time(),
                                head=head_hash,
                                description="(recovered — run 'clavus repair' to set .als path)",
                            )
                            projects[proj.name] = asdict(proj)

            if projects:
                projects["_last_project"] = list(projects.keys())[0]
                self._write_json(self.index_path, projects)
                print(f"⚠️  index.json was missing — recovered {len(projects)} project(s) from cues/refs")
                return True

        return False

    # ── Index (Active Project) ──

    def set_index(self, project: ClavusProject) -> None:
        """Set the active tracked project in the index."""
        self._backup_index()
        index = json.loads(self.index_path.read_text()) if self.index_path.exists() else {}
        index[project.name] = asdict(project)
        index["_last_project"] = project.name
        self._write_json(self.index_path, index)
        # Also persist as a ref for recovery safety
        self.update_ref("_last_project", project.name)

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
        return [ClavusProject(**data) for data in index.values()
                if isinstance(data, dict)]


# ─── Stem Registry ──────────────────────────────────────────────────


STEMS_DIR = "stems"  # Working tree: reconstructed stem directories per snapshot


class StemStore:
    """Manages stem file registry — mapping blobs to snapshots.

    Stems are stored as content-addressed blobs in objects/ (dedup'd by hash).
    A StemManifest per snapshot records which stems belong to it.
    The stems/ working tree is reconstructed on demand (e.g., after pull).
    """

    def __init__(self, project_name: str, store: BlobStore):
        self.project = project_name
        self.store = store
        self.stems_root = store.root / STEMS_DIR / project_name

    # ── Manifest CRUD ──

    def get_manifest(self, snapshot_hash: str) -> Optional[StemManifest]:
        """Load the StemManifest for a given snapshot."""
        path = self._manifest_path(snapshot_hash)
        if not path.exists():
            return None
        try:
            data = json.loads(path.read_text())
            stems = [StemEntry(**s) for s in data.get("stems", [])]
            return StemManifest(
                snapshot_hash=data["snapshot_hash"],
                stems=stems,
                created_at=data.get("created_at", 0.0),
            )
        except (json.JSONDecodeError, KeyError):
            return None

    def save_manifest(self, manifest: StemManifest) -> None:
        """Save or update a StemManifest."""
        path = self._manifest_path(manifest.snapshot_hash)
        path.parent.mkdir(parents=True, exist_ok=True)
        data = {
            "snapshot_hash": manifest.snapshot_hash,
            "created_at": manifest.created_at or time.time(),
            "stems": [asdict(s) for s in manifest.stems],
        }
        path.write_text(json.dumps(data, indent=2, default=str))

    def list_manifests(self) -> list[str]:
        """List all snapshot hashes that have stem manifests."""
        base = self.stems_root
        if not base.exists():
            return []
        return sorted([d.name for d in base.iterdir() if d.is_dir()])

    # ── Stem Blob Operations ──

    def store_stem_file(self, file_path: str, track_name: str) -> StemEntry:
        """Ingest a stem audio file into the blob store. Returns a StemEntry."""
        path = Path(file_path)
        data = path.read_bytes()
        content_hash = self.store.put_object(data)

        # Gather file metadata
        import wave
        import struct
        duration = 0.0
        sample_rate = 44100
        bit_depth = 24
        channels = 2
        try:
            with wave.open(str(path), 'rb') as wf:
                channels = wf.getnchannels()
                sample_rate = wf.getframerate()
                bit_depth = wf.getsampwidth() * 8
                frames = wf.getnframes()
                duration = frames / sample_rate if sample_rate > 0 else 0.0
        except Exception:
            pass

        return StemEntry(
            track_name=track_name,
            file_name=path.name,
            hash=content_hash,
            size=len(data),
            format=path.suffix.lstrip(".").lower(),
            sample_rate=sample_rate,
            bit_depth=bit_depth,
            channels=channels,
            duration_seconds=duration,
            bounced_at=time.time(),
        )

    def get_stem_data(self, stem_hash: str) -> Optional[bytes]:
        """Retrieve stem audio data by content hash."""
        return self.store.get_object(stem_hash)

    def has_stem(self, stem_hash: str) -> bool:
        """Check if a stem blob exists locally."""
        return self.store.has_object(stem_hash)

    # ── Working Tree (reconstruct stems/ on disk) ──

    def has_working_tree(self, snapshot_hash: str) -> bool:
        """Check if the stems directory is already materialized."""
        return self._manifest_path(snapshot_hash).exists()

    def materialize_stems(self, snapshot_hash: str, output_dir: str = "") -> list[Path]:
        """Reconstruct stem files from blobs into a directory. Returns list of created paths."""
        manifest = self.get_manifest(snapshot_hash)
        if not manifest:
            return []

        out = Path(output_dir) if output_dir else self._snap_dir(snapshot_hash)
        out.mkdir(parents=True, exist_ok=True)

        created = []
        for entry in manifest.stems:
            data = self.store.get_object(entry.hash)
            if not data:
                continue
            stem_path = out / entry.file_name
            stem_path.write_bytes(data)
            created.append(stem_path)

        return created

    # ── Path Helpers ──

    def _manifest_path(self, snapshot_hash: str) -> Path:
        return self._snap_dir(snapshot_hash) / "StemManifest.json"

    def _snap_dir(self, snapshot_hash: str) -> Path:
        return self.stems_root / snapshot_hash[:12]

    @staticmethod
    def _write_json(path: Path, data: dict) -> None:
        path.write_text(json.dumps(data, indent=2, default=str))

# ── Serialization Helpers ──


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
                "clips": [
                    {
                        "start_beats": c.start_beats,
                        "end_beats": c.end_beats,
                        "name": c.name,
                        "color": c.color,
                        "clip_type": c.clip_type,
                    }
                    for c in t.clips
                ],
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


def _dict_to_project(data: dict) -> Project:
    """Reconstruct a Project from a serialized dict."""
    from clavus.parser import Project, Track, Marker, Device, TempoEvent, Clip

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
            clips=[Clip(**c) for c in td.get("clips", [])],
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
    clips_changed: Optional[bool] = None  # True if clip positions/counts differ
    clips_before: int = 0  # Number of clips before
    clips_after: int = 0   # Number of clips after


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

            # Clip diff
            clips_changed = None
            clips_before = len(bt.clips)
            clips_after = len(at.clips)
            if clips_before != clips_after:
                clips_changed = True
            else:
                # Check if any clip positions differ
                for bc, ac in zip(bt.clips, at.clips):
                    if (bc.start_beats != ac.start_beats or
                        bc.end_beats != ac.end_beats or
                        bc.name != ac.name):
                        clips_changed = True
                        break

            if (devices_added or devices_removed or frozen_changed is not None or
                mute_changed is not None or clips_changed):
                diff.tracks.append(TrackDiff(
                    name=name,
                    status="modified",
                    devices_added=devices_added,
                    devices_removed=devices_removed,
                    frozen_changed=frozen_changed,
                    mute_changed=mute_changed,
                    clips_changed=clips_changed,
                    clips_before=clips_before,
                    clips_after=clips_after,
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
        clip_modified = [t for t in modified if t.clips_changed]
        other_modified = [t for t in modified if not t.clips_changed]
        if other_modified:
            summary_parts.append(f"Modified: {', '.join(t.name for t in other_modified[:5])}")
            if len(other_modified) > 5:
                summary_parts[-1] += f" (+{len(other_modified) - 5} more)"
        if clip_modified:
            for cm in clip_modified[:3]:
                diff_count = cm.clips_after - cm.clips_before
                if diff_count > 0:
                    summary_parts.append(f"{cm.name}: +{diff_count} clips")
                else:
                    summary_parts.append(f"{cm.name}: {diff_count} clips")
            if len(clip_modified) > 3:
                summary_parts[-1] += f" (+{len(clip_modified) - 3} more clipped)"
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
        if td.clips_changed:
            diff_count = td.clips_after - td.clips_before
            if diff_count > 0:
                lines.append(f"       +{diff_count} clips")
            else:
                lines.append(f"       {diff_count} clips")

    if diff.markers_added:
        lines.append(f"\n  📍 Markers added: {', '.join(diff.markers_added[:5])}")
    if diff.markers_removed:
        lines.append(f"  🗑 Markers removed: {', '.join(diff.markers_removed[:3])}")

    return "\n".join(lines)
