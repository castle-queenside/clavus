"""
Clavus — peer-to-peer sync engine.

Self-contained, no cloud, no subscriptions.
Machines sync directly over LAN or Tailscale.

Usage:
  clavus remote add friend http://friend.local:7890
  clavus push                    # push to all remotes
  clavus pull                    # pull from all remotes
  clavus sync                    # daemon: auto-push on changes
"""

from __future__ import annotations

import json
import os
import time
import threading
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

import httpx

from clavus.store import (
    BlobStore, ClavusProject, Snapshot, DEFAULT_CLAVUS_DIR, StemStore,
)
from clavus.cues import CueStore, Cue, CueReply as CueReplyData, CueFilter


# ─── Remote Config ────────────────────────────────────────────────────

REMOTES_FILE = "remotes.json"


@dataclass
class Remote:
    """A remote clavus server."""
    name: str
    url: str  # e.g., "http://friend.local:7890"
    last_sync: float = 0.0


def load_remotes(store: BlobStore) -> list[Remote]:
    """Load remote configurations."""
    path = store.root / REMOTES_FILE
    if not path.exists():
        return []
    try:
        data = json.loads(path.read_text())
        return [Remote(**r) for r in data.get("remotes", [])]
    except (json.JSONDecodeError, TypeError):
        return []


def save_remotes(store: BlobStore, remotes: list[Remote]) -> None:
    """Save remote configurations."""
    path = store.root / REMOTES_FILE
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(
        {"remotes": [{"name": r.name, "url": r.url.rstrip("/"), "last_sync": r.last_sync}
                     for r in remotes]},
        indent=2,
    ))


# ─── Sync Client ──────────────────────────────────────────────────────

class SyncClient:
    """HTTP client for talking to remote clavus servers."""

    def __init__(self, remote_url: str):
        self.base_url = remote_url.rstrip("/")
        self.client = httpx.Client(timeout=30.0)

    def ping(self) -> bool:
        try:
            r = self.client.get(f"{self.base_url}/api/ping")
            return r.status_code == 200
        except Exception:
            return False

    def pull(self, project: str) -> Optional[dict]:
        """Pull cues and snapshots from remote."""
        try:
            r = self.client.get(
                f"{self.base_url}/api/sync/pull",
                params={"name": project},
                timeout=30,
            )
            if r.status_code == 200:
                return r.json()
        except Exception:
            pass
        return None

    def push_cues(self, project: str, cues: list[dict]) -> bool:
        """Push cues to remote."""
        try:
            r = self.client.post(
                f"{self.base_url}/api/sync/push",
                params={"name": project},
                json={"cues": cues},
                timeout=30,
            )
            return r.status_code == 200
        except Exception:
            return False

    def push_snapshots(self, project: str, snapshots: list[dict]) -> bool:
        """Push snapshot metadata to remote."""
        try:
            r = self.client.post(
                f"{self.base_url}/api/sync/push-snapshots",
                params={"name": project},
                json={"snapshots": snapshots},
                timeout=60,
            )
            return r.status_code == 200
        except Exception:
            return False

    def close(self):
        self.client.close()


# ─── Snapshot Blob Sync ──────────────────────────────────────────────


def push_snapshot_blobs(
    store: BlobStore, proj: ClavusProject, remote: Remote,
) -> int:
    """Push snapshot content blobs + .als backup blobs to a remote.

    Walks the snapshot history and uploads any blobs (project JSON content
    and raw .als backups) that the remote doesn't already have.

    Returns the number of blobs uploaded.
    """
    import base64

    client = SyncClient(remote.url)
    count = 0
    BATCH_SIZE = 25  # Blobs per upload batch

    try:
        # Collect all blob hashes we need to check
        content_hashes: list[str] = []
        als_hashes: list[str] = []
        current = proj.head
        seen: set[str] = set()

        while current:
            if current in seen:
                break
            seen.add(current)
            snap = store.load_snapshot(current)
            if not snap:
                break

            # Content blob hash = the snapshot hash itself
            if snap.hash not in seen:
                content_hashes.append(snap.hash)

            # .als backup blob hash
            if snap.als_hash and snap.als_hash not in seen:
                als_hashes.append(snap.als_hash)

            if snap.parent == current:
                break
            current = snap.parent

        # Check which content blobs the remote is missing
        all_content_hashes = content_hashes
        try:
            r = client.client.post(
                f"{remote.url}/api/sync/check-blobs",
                json={"hashes": all_content_hashes},
                timeout=30,
            )
            if r.status_code == 200:
                missing = r.json().get("missing", [])
            else:
                missing = all_content_hashes
        except Exception:
            missing = all_content_hashes

        # Upload missing content blobs in batches
        if missing:
            print(f"    Uploading {len(missing)} content blob(s)...")
            for i in range(0, len(missing), BATCH_SIZE):
                batch = missing[i:i + BATCH_SIZE]
                upload_batch = []
                for h in batch:
                    data = store.get_object(h)
                    if data:
                        upload_batch.append({
                            "hash": h,
                            "data": base64.b64encode(data).decode("ascii"),
                        })

                if upload_batch:
                    try:
                        r = client.client.post(
                            f"{remote.url}/api/sync/push-blobs",
                            json=upload_batch,
                            timeout=120,
                        )
                        if r.status_code == 200:
                            count += len(upload_batch)
                    except Exception:
                        pass

        # Check which .als backup blobs are missing
        if als_hashes:
            try:
                r = client.client.post(
                    f"{remote.url}/api/sync/check-blobs",
                    json={"hashes": als_hashes},
                    timeout=30,
                )
                if r.status_code == 200:
                    missing_als = r.json().get("missing", [])
                else:
                    missing_als = als_hashes
            except Exception:
                missing_als = als_hashes

            if missing_als:
                print(f"    Uploading {len(missing_als)} .als backup(s)...")
                for i in range(0, len(missing_als), BATCH_SIZE):
                    batch = missing_als[i:i + BATCH_SIZE]
                    upload_batch = []
                    for h in batch:
                        data = store.get_object(h)
                        if data:
                            upload_batch.append({
                                "hash": h,
                                "data": base64.b64encode(data).decode("ascii"),
                            })

                    if upload_batch:
                        try:
                            r = client.client.post(
                                f"{remote.url}/api/sync/push-als-blobs",
                                json=upload_batch,
                                timeout=120,
                            )
                        except Exception:
                            pass

    finally:
        client.close()

    return count


def pull_snapshot_blobs(
    store: BlobStore, proj: ClavusProject, remote: Remote,
) -> int:
    """Pull missing snapshot content blobs + .als backups from a remote.

    Checks which blobs in our snapshot history are missing locally and
    fetches them from the remote. Returns number of blobs downloaded.
    """
    import base64
    import httpx

    try:
        # First, pull the remote's snapshot metadata so we know what exists
        pull_from_remote(store, proj, remote)
    except Exception:
        pass

    client = SyncClient(remote.url)
    count = 0

    try:
        # Re-read project from store (pull may have updated it)
        proj_ref = store.get_index(proj.name)
        if not proj_ref:
            return 0
        proj = proj_ref

        # Collect locally missing content hashes from our snapshot history
        missing_content: set[str] = set()
        missing_als: set[str] = set()
        current = proj.head
        seen: set[str] = set()

        while current:
            if current in seen:
                break
            seen.add(current)
            snap = store.load_snapshot(current)
            if not snap:
                break

            if not store.has_object(snap.hash):
                missing_content.add(snap.hash)
            if snap.als_hash and not store.has_object(snap.als_hash):
                missing_als.add(snap.als_hash)

            if snap.parent == current:
                break
            current = snap.parent

        # Also check what blobs the remote's snapshots reference
        try:
            r = client.client.get(
                f"{remote.url}/api/sync/pull",
                params={"name": proj.name},
                timeout=30,
            )
            if r.status_code == 200:
                data = r.json()
                for s in data.get("snapshots", []):
                    full_hash = s.get("full_hash", s.get("hash", ""))
                    if full_hash and not store.has_object(full_hash):
                        missing_content.add(full_hash)
        except Exception:
            pass

        # Download missing content blobs from the relay
        downloaded = set()
        for h in list(missing_content):
            if h in downloaded:
                continue
            try:
                r = client.client.get(
                    f"{remote.url}/api/blobs/{h}",
                    timeout=120,
                )
                if r.status_code == 200:
                    store.put_object(r.content, h)
                    count += 1
                    downloaded.add(h)
                    continue
            except Exception:
                pass
            print(f"    ⚠️  Could not fetch blob {h[:12]}")

        # Download missing .als backups
        for h in list(missing_als):
            if h in downloaded:
                continue
            try:
                r = client.client.get(
                    f"{remote.url}/api/stems/blob/{h}",
                    timeout=120,
                )
                if r.status_code == 200:
                    store.put_object(r.content, h)
                    count += 1
                    downloaded.add(h)
            except Exception:
                print(f"    ⚠️  Could not fetch .als backup {h[:12]}")

    finally:
        client.close()

    return count


# ─── Push / Pull Logic ───────────────────────────────────────────────

def _cues_to_dicts(cues_store: CueStore) -> list[dict]:
    """Serialize all cues for transport."""
    all_cues = cues_store.list_cues(CueFilter())
    return [{
        "id": c.id, "position": c.position, "text": c.text,
        "author": c.author, "status": c.status, "timestamp": c.timestamp,
        "track_name": c.track_name, "snapshot_hash": c.snapshot_hash,
        "assignee": c.assignee, "in_progress": c.in_progress,
        "replies": [
            {"id": r.id, "text": r.text, "author": r.author,
             "timestamp": r.timestamp, "snapshot_hash": r.snapshot_hash}
            for r in (c.replies or [])
        ],
    } for c in all_cues]


def _snapshots_to_dicts(store: BlobStore, proj: ClavusProject) -> list[dict]:
    """Serialize snapshot history for transport."""
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
            "hash": snap.hash, "full_hash": snap.hash,
            "timestamp": snap.timestamp, "message": snap.message,
            "track_count": snap.track_count, "bpm": snap.bpm,
            "project_path": snap.project_path,
            "tags": snap.tags,
            "parent": snap.parent,
        })
        if snap.parent == current:
            break
        current = snap.parent
    return history


def push_to_remote(store: BlobStore, proj: ClavusProject, remote: Remote) -> dict:
    """Push all data to a remote. Returns summary."""
    result = {"cues": 0, "snapshots": 0, "error": ""}
    client = SyncClient(remote.url)

    try:
        if not client.ping():
            result["error"] = f"Cannot reach {remote.url}"
            return result

        # Push cues
        cues_store = CueStore(proj.name, store=store)
        cues_data = _cues_to_dicts(cues_store)
        if cues_data:
            ok = client.push_cues(proj.name, cues_data)
            result["cues"] = len(cues_data) if ok else 0
            if not ok:
                result["error"] = "Failed to push cues"

        # Push snapshots
        snap_data = _snapshots_to_dicts(store, proj)
        if snap_data:
            ok = client.push_snapshots(proj.name, snap_data)
            result["snapshots"] = len(snap_data) if ok else 0
            if not ok and not result["error"]:
                result["error"] = "Failed to push snapshots"

        remote.last_sync = time.time()
        save_remotes(store, load_remotes(store))
    finally:
        client.close()

    return result


def pull_from_remote(store: BlobStore, proj: ClavusProject, remote: Remote) -> dict:
    """Pull all data from a remote. Returns summary."""
    result = {"cues": 0, "snapshots": 0, "error": ""}
    client = SyncClient(remote.url)

    try:
        if not client.ping():
            result["error"] = f"Cannot reach {remote.url}"
            return result

        data = client.pull(proj.name)
        if not data:
            result["error"] = "Pull returned no data"
            return result

        cues_store = CueStore(proj.name, store=store)

        # Import cues
        for c in data.get("cues", []):
            cue = Cue(
                id=c["id"], position=c.get("position", "0.0.0"),
                text=c.get("text", ""), author=c.get("author", ""),
                status=c.get("status", "pending"),
                timestamp=c.get("timestamp", 0.0),
                track_name=c.get("track_name", ""),
                snapshot_hash=c.get("snapshot_hash", ""),
                assignee=c.get("assignee", ""),
                in_progress=c.get("in_progress", False),
            )
            cues_store.import_cue(cue)

            # Import replies
            for r in c.get("replies", []):
                reply = CueReplyData(
                    id=r.get("id", ""), text=r.get("text", ""),
                    author=r.get("author", ""),
                    timestamp=r.get("timestamp", 0.0),
                    snapshot_hash=r.get("snapshot_hash", ""),
                )
                cues_store.import_reply(c["id"], reply)

        result["cues"] = len(data.get("cues", []))

        # Import snapshots
        for s in data.get("snapshots", []):
            snap = Snapshot(
                hash=s.get("full_hash", s["hash"]),
                timestamp=s.get("timestamp", 0.0),
                message=s.get("message", ""),
                parent=s.get("parent", None),
                project_path=s.get("project_path", ""),
                track_count=s.get("track_count", 0),
                bpm=s.get("bpm", 120.0),
                tags=s.get("tags", []),
            )
            # Store snapshot metadata
            meta_dir = store.objects_dir / snap.hash[:2]
            meta_dir.mkdir(parents=True, exist_ok=True)
            meta_path = meta_dir / f"{snap.hash}.meta"
            if not meta_path.exists():
                from dataclasses import asdict
                meta_path.write_text(json.dumps(asdict(snap), indent=2, default=str))

        result["snapshots"] = len(data.get("snapshots", []))

        # Update HEAD to latest snapshot if we have one and HEAD is empty
        if result["snapshots"] > 0:
            current_head = store.read_ref("HEAD")
            if not current_head:
                # Find the snapshot with no parent (root) or just use the first one
                snap_list = data.get("snapshots", [])
                for s in snap_list:
                    snap_hash = s.get("full_hash", s.get("hash", ""))
                    parent = s.get("parent", None)
                    if not parent or parent == snap_hash:
                        store.update_ref("HEAD", snap_hash)
                        proj.head = snap_hash
                        store.set_index(proj)
                        break
                else:
                    # Fallback: use last snapshot in list
                    last = snap_list[-1]
                    last_hash = last.get("full_hash", last.get("hash", ""))
                    store.update_ref("HEAD", last_hash)
                    proj.head = last_hash
                    store.set_index(proj)

        remote.last_sync = time.time()
        save_remotes(store, load_remotes(store))
    finally:
        client.close()

    return result


# ─── WebSocket Sync Daemon ──────────────────────────────────────────

def _apply_cue_event(store: BlobStore, proj: ClavusProject, event: str, data: dict):
    """Apply a cue event received from websocket to local store."""
    cues_store = CueStore(proj.name, store=store)

    if event == "cue_new":
        cue = Cue(
            id=data.get("id", ""),
            position=data.get("position", "0.0.0"),
            text=data.get("text", ""),
            author=data.get("author", "remote"),
            status=data.get("status", "pending"),
            timestamp=data.get("timestamp", 0.0),
            track_name=data.get("track_name", ""),
            snapshot_hash=data.get("snapshot_hash", ""),
            assignee=data.get("assignee", ""),
            in_progress=data.get("in_progress", False),
        )
        cues_store.import_cue(cue)
        print(f"  📥 Incoming cue: {cue.text[:40]} @ {cue.position}")

    elif event == "cue_reply":
        cue_id = data.get("cue_id", "")
        reply_text = data.get("reply", "")
        reply = CueReplyData(
            id=f"ws_{int(time.time())}",
            text=reply_text,
            author="remote",
            timestamp=data.get("timestamp", time.time()),
        )
        if cues_store.import_reply(cue_id, reply):
            print(f"  📥 Incoming reply to {cue_id[:8]}: {reply_text[:40]}")

    elif event == "cue_update":
        cue_id = data.get("cue_id", "")
        status = data.get("status", "")
        if status == "resolved":
            cues_store.resolve(cue_id)
            print(f"  📥 Incoming resolve: {cue_id[:8]}")
        elif status == "skipped":
            cues_store.skip(cue_id)
            print(f"  📥 Incoming skip: {cue_id[:8]}")


class SyncDaemon:
    """Background daemon that syncs changes to remotes in real-time.

    Connects to each remote via websocket and listens for incoming events.
    Also pushes local changes when they happen (via REST as fallback).
    """

    def __init__(self, store: BlobStore, proj: ClavusProject, interval: int = 30):
        self.store = store
        self.proj = proj
        self.interval = interval
        self._running = False
        self._thread: Optional[threading.Thread] = None
        self._last_cue_count = 0
        self._last_event_times: set[str] = set()

    def start(self):
        self._running = True
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()
        print(f"🔄 Sync daemon started")
        print(f"   Project: '{self.proj.name}'")

        remotes = load_remotes(self.store)
        for r in remotes:
            print(f"   Remote:  {r.name} ({r.url})")
        if not remotes:
            print(f"   No remotes configured — use 'clavus remote add'")

    def stop(self):
        self._running = False
        print("🛑 Sync daemon stopped")

    def _run(self):
        while self._running:
            remotes = load_remotes(self.store)
            for remote in remotes:
                if not self._running:
                    break
                try:
                    self._listen_to_remote(remote)
                except Exception as e:
                    print(f"  ⚠️  Connection to '{remote.name}' lost: {e}")
                    print(f"     Reconnecting in {self.interval}s...")
            # Wait before reconnecting
            for _ in range(self.interval):
                if not self._running:
                    return
                time.sleep(1)

    def _listen_to_remote(self, remote: Remote):
        """Connect to a remote's websocket and listen for events."""
        ws_url = remote.url.replace("http://", "ws://").replace("https://", "wss://")
        ws_url = f"{ws_url}/ws?project={self.proj.name}"

        import httpx

        with httpx.Client() as client:
            try:
                # Initial sync: pull all cues
                resp = client.get(
                    f"{remote.url}/api/sync/pull",
                    params={"name": self.proj.name},
                    timeout=15,
                )
                if resp.status_code == 200:
                    data = resp.json()
                    for c in data.get("cues", []):
                        _apply_cue_event(self.store, self.proj, "cue_new", c)
                    cue_count = len(data.get("cues", []))
                    snap_count = len(data.get("snapshots", []))
                    if cue_count or snap_count:
                        print(f"  📥 Initial sync from '{remote.name}': {cue_count} cues, {snap_count} snapshots")
            except Exception as e:
                print(f"  ⚠️  Initial sync from '{remote.name}' failed: {e}")
                return

        # Now listen for real-time events via websocket
        try:
            import websockets.sync.client

            print(f"  🔌 Listening to '{remote.name}' ({ws_url})...")
            with websockets.sync.client.connect(ws_url) as ws:
                # Push our current state to the remote
                cues_store = CueStore(self.proj.name, store=self.store)
                all_cues = cues_store.list_cues(CueFilter())
                for cue in all_cues:
                    ws.send(json.dumps({
                        "event": "cue_new",
                        "data": {
                            "id": cue.id, "position": cue.position,
                            "text": cue.text, "author": cue.author,
                            "status": cue.status, "timestamp": cue.timestamp,
                            "track_name": cue.track_name,
                        }
                    }))

                # Listen loop
                while self._running:
                    message = ws.recv()
                    data = json.loads(message)
                    event = data.get("event")
                    payload = data.get("data", {})

                    if event == "ping":
                        ws.send(json.dumps({"event": "pong"}))
                    elif event == "pong":
                        pass
                    elif event in ("cue_new", "cue_reply", "cue_update"):
                        _apply_cue_event(self.store, self.proj, event, payload)
        except ImportError:
            print(f"  ⚠️  websockets library not installed. Run: pip install websockets")
        except Exception as e:
            raise e


# ─── Stem Sync ───────────────────────────────────────────────────────


def push_stems_to_remote(
    store: BlobStore, proj: ClavusProject,
    remote: Remote, stem_store: StemStore, snapshot_hash: str,
) -> int:
    """Push stem blobs for a snapshot to a remote. Returns number of stems pushed."""
    manifest = stem_store.get_manifest(snapshot_hash)
    if not manifest or not manifest.stems:
        return 0

    client = SyncClient(remote.url)
    count = 0

    try:
        # Ask remote which hashes it's missing
        all_hashes = [s.hash for s in manifest.stems]
        r = client.client.post(
            f"{remote.url}/api/stems/check",
            json={"hashes": all_hashes},
            timeout=30,
        )
        if r.status_code != 200:
            print(f"  ⚠️  Stem check failed: {r.status_code}")
            return 0

        missing = r.json().get("missing", [])
        if not missing:
            return len(all_hashes)  # All already present on remote

        # Upload each missing stem blob
        for stem_hash in missing:
            data = store.get_object(stem_hash)
            if not data:
                print(f"  ⚠️  Stem blob {stem_hash[:12]} not found locally, skipping")
                continue

            # Find the entry for reporting
            entry = next((s for s in manifest.stems if s.hash == stem_hash), None)
            track = entry.track_name if entry else "?"

            r = client.client.post(
                f"{remote.url}/api/stems/blob/{stem_hash}",
                content=data,
                timeout=120,
            )
            if r.status_code == 200:
                print(f"    Uploaded {track} ({stem_hash[:12]}) — {len(data) / (1024*1024):.1f} MB")
                count += 1
            else:
                print(f"    ⚠️  Upload failed for {stem_hash[:12]}: {r.status_code}")

        # Push the manifest too
        manifest_data = {
            "snapshot_hash": manifest.snapshot_hash,
            "stems": [{
                "track_name": s.track_name, "file_name": s.file_name,
                "hash": s.hash, "size": s.size, "format": s.format,
                "sample_rate": s.sample_rate, "bit_depth": s.bit_depth,
                "channels": s.channels, "duration_seconds": s.duration_seconds,
            } for s in manifest.stems],
        }
        r = client.client.post(
            f"{remote.url}/api/stems/{proj.name}/manifest/{snapshot_hash}",
            json=manifest_data,
            timeout=30,
        )

    finally:
        client.close()

    return count


def pull_stems_from_remote(
    store: BlobStore, proj: ClavusProject, remote: Remote,
) -> int:
    """Pull stem files from a remote for the current HEAD. Returns count downloaded."""
    head = store.read_ref("HEAD")
    if not head:
        return 0

    client = SyncClient(remote.url)
    stem_store = StemStore(proj.name, store)
    count = 0

    try:
        # Get remote's manifest for this snapshot
        r = client.client.get(
            f"{remote.url}/api/stems/{proj.name}/manifest/{head}",
            timeout=30,
        )
        if r.status_code != 200:
            return 0

        manifest_data = r.json()
        stems = manifest_data.get("stems", [])
        if not stems:
            return 0

        # Check which we need locally
        needed = [s for s in stems if not stem_store.has_stem(s["hash"])]

        if not needed:
            return len(stems)  # All already present

        # Download each missing stem
        for entry in needed:
            r = client.client.get(
                f"{remote.url}/api/stems/blob/{entry['hash']}",
                timeout=120,
            )
            if r.status_code != 200:
                print(f"  ⚠️  Download failed for {entry['hash'][:12]}: {r.status_code}")
                continue

            # Store the blob
            store.put_object(r.content, entry["hash"])
            size_mb = len(r.content) / (1024 * 1024)
            print(f"    Downloaded {entry['track_name']} ({entry['hash'][:12]}) — {size_mb:.1f} MB")
            count += 1

        # Also save the manifest locally
        local_manifest = stem_store.get_manifest(head)
        if local_manifest:
            for entry in stems:
                local_entry = next(
                    (s for s in local_manifest.stems if s.hash == entry["hash"]), None
                )
                if not local_entry:
                    from clavus.store import StemEntry
                    local_manifest.stems.append(StemEntry(
                        track_name=entry["track_name"],
                        file_name=entry["file_name"],
                        hash=entry["hash"],
                        size=entry.get("size", 0),
                        format=entry.get("format", "wav"),
                        sample_rate=entry.get("sample_rate", 44100),
                        bit_depth=entry.get("bit_depth", 24),
                        channels=entry.get("channels", 2),
                        duration_seconds=entry.get("duration_seconds", 0),
                        bounced_at=0,
                    ))
            stem_store.save_manifest(local_manifest)
        else:
            from clavus.store import StemManifest, StemEntry
            new_manifest = StemManifest(snapshot_hash=head, created_at=time.time())
            for entry in stems:
                new_manifest.stems.append(StemEntry(
                    track_name=entry["track_name"],
                    file_name=entry["file_name"],
                    hash=entry["hash"],
                    size=entry.get("size", 0),
                    format=entry.get("format", "wav"),
                    sample_rate=entry.get("sample_rate", 44100),
                    bit_depth=entry.get("bit_depth", 24),
                    channels=entry.get("channels", 2),
                    duration_seconds=entry.get("duration_seconds", 0),
                    bounced_at=0,
                ))
            stem_store.save_manifest(new_manifest)

    finally:
        client.close()

    return count
