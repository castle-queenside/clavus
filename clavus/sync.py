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
from clavus.helpers import get_desktop_path, get_projects_dir
from clavus.cues import CueStore, Cue, CueReply as CueReplyData, CueFilter


# ─── Remote Config ────────────────────────────────────────────────────

REMOTES_FILE = "remotes.json"


@dataclass
class Remote:
    """A remote clavus server."""
    name: str
    url: str  # e.g., "http://friend.local:7890"
    last_sync: float = 0.0
    last_head: str | None = None  # relay HEAD after last sync — for optimistic lock


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
        {"remotes": [{"name": r.name, "url": r.url.rstrip("/"), "last_sync": r.last_sync, "last_head": r.last_head}
                     for r in remotes]},
        indent=2,
    ))


# ─── Sync Client ──────────────────────────────────────────────────────

# Retryable exceptions: transient network failures, not semantic errors
_RETRYABLE = (
    httpx.ConnectError,
    httpx.TimeoutException,
    httpx.RemoteProtocolError,
    httpx.NetworkError,
    ConnectionError,
    TimeoutError,
    OSError,  # "Connection reset by peer" etc
)
_MAX_RETRIES = 3
_RETRY_BACKOFF = [1.0, 2.0, 4.0]  # seconds between attempts


class SyncClient:
    """HTTP client for talking to remote clavus servers.

    All HTTP calls get automatic retry on transient network failures
    (timeouts, connection resets, DNS blips). Semantic errors (409, 404)
    are never retried — they mean something that retrying won't fix.
    """

    def __init__(self, remote_url: str):
        self.base_url = remote_url.rstrip("/")
        self.client = httpx.Client(timeout=30.0)

    def _retry(self, fn, *args, **kwargs):
        """Call fn() up to _MAX_RETRIES times with backoff on transient errors.

        Returns (response, None) on success, (None, error_message) on failure.
        Semantic errors (4xx except 429) return immediately without retry.
        """
        last_error = None
        for attempt in range(_MAX_RETRIES):
            try:
                r = fn(*args, **kwargs)
                # 4xx errors (except 429 rate-limit) are semantic — don't retry
                if 400 <= r.status_code < 500 and r.status_code != 429:
                    return r, None
                if r.status_code < 500:
                    return r, None
                # 5xx — retryable
                last_error = f"HTTP {r.status_code}"
            except _RETRYABLE as e:
                last_error = str(e)[:120]
            except Exception as e:
                # Unexpected — don't retry
                return None, str(e)[:120]

            if attempt < _MAX_RETRIES - 1:
                time.sleep(_RETRY_BACKOFF[attempt])

        return None, last_error or "unknown"

    def ping(self) -> bool:
        r, _ = self._retry(
            lambda: self.client.get(f"{self.base_url}/api/ping", timeout=10)
        )
        return r is not None and r.status_code == 200

    def pull(self, project: str) -> Optional[dict]:
        r, err = self._retry(
            lambda: self.client.get(
                f"{self.base_url}/api/sync/pull",
                params={"name": project},
                timeout=30,
            )
        )
        if r is not None and r.status_code == 200:
            try:
                return r.json()
            except Exception:
                return None
        return None

    def push_cues(self, project: str, cues: list[dict]) -> bool:
        r, _ = self._retry(
            lambda: self.client.post(
                f"{self.base_url}/api/sync/push",
                params={"name": project},
                json={"cues": cues},
                timeout=30,
            )
        )
        return r is not None and r.status_code == 200

    def push_snapshots(self, project: str, snapshots: list[dict],
                        expected_parent: str | None = None) -> tuple[bool, str | None]:
        """Push snapshot metadata to remote.

        Args:
            expected_parent: Hash the peer expects to be HEAD on the relay.
                             If set and doesn't match, relay returns 409 Conflict.

        Returns:
            (success, conflict_message, relay_head) — conflict_message set on 409,
            relay_head is the current relay HEAD from the 409 response (for auto-recovery).
        """
        body: dict = {"snapshots": snapshots}
        if expected_parent:
            body["expected_parent"] = expected_parent

        r, _ = self._retry(
            lambda: self.client.post(
                f"{self.base_url}/api/sync/push-snapshots",
                params={"name": project},
                json=body,
                timeout=60,
            )
        )
        if r is None:
            return False, None, None
        if r.status_code == 200:
            return True, None, None
        if r.status_code == 409:
            try:
                resp_body = r.json()
                # FastAPI wraps detail in {"detail": ...}
                detail = resp_body.get("detail", resp_body) if isinstance(resp_body, dict) else resp_body
                if isinstance(detail, dict):
                    err = detail.get("message", detail.get("error", "Conflict — pull first"))
                    relay_head = detail.get("relay_head")
                else:
                    err = str(detail)
                    relay_head = None
            except Exception:
                err = "Conflict — pull first"
                relay_head = None
            return False, err, relay_head
        return False, None, None

    def close(self):
        self.client.close()

    def request_with_retry(self, method: str, path: str, **kwargs):
        """Make an HTTP request with retry. Use this for raw API calls
        that aren't covered by ping/pull/push_cues/push_snapshots."""
        return self._retry(
            lambda: self.client.request(
                method, f"{self.base_url}{path}", **kwargs
            )
        )


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
        sample_hashes_list: list[str] = []
        current = proj.head
        seen: set[str] = set()
        blob_seen: set[str] = set()  # dedup blobs across walk, not share with snap hashes

        while current:
            if current in seen:
                break
            seen.add(current)
            snap = store.load_snapshot(current)
            if not snap:
                break

            # Content blob hash = snapshot's content_hash (parsed JSON)
            if snap.content_hash and snap.content_hash not in blob_seen:
                content_hashes.append(snap.content_hash)
                blob_seen.add(snap.content_hash)

            # .als backup blob hash (raw .als = snapshot identity now)
            if snap.als_hash and snap.als_hash not in blob_seen:
                als_hashes.append(snap.als_hash)
                blob_seen.add(snap.als_hash)

            # Audio sample blob hashes
            for sh in (snap.sample_hashes or []):
                if sh not in blob_seen:
                    sample_hashes_list.append(sh)
                    blob_seen.add(sh)

            if snap.parent == current:
                break
            current = snap.parent

        # Check which content blobs the remote is missing
        all_content_hashes = content_hashes
        print(f"    Checking {len(all_content_hashes)} content blob(s)...")
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
            print(f"    Checking {len(als_hashes)} .als backup(s)...")
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
                        except Exception as e:
                            print(f"    ⚠️  .als upload failed ({len(upload_batch)} blob(s)): {e}")

        # Check which sample blobs the remote is missing
        if sample_hashes_list:
            print(f"    Checking {len(sample_hashes_list)} audio sample(s)...")
            try:
                r = client.client.post(
                    f"{remote.url}/api/sync/check-blobs",
                    json={"hashes": sample_hashes_list},
                    timeout=30,
                )
                if r.status_code == 200:
                    missing_samples = r.json().get("missing", [])
                else:
                    missing_samples = sample_hashes_list
            except Exception:
                missing_samples = sample_hashes_list

            if missing_samples:
                print(f"    Uploading {len(missing_samples)} audio sample(s)...")
                for i in range(0, len(missing_samples), BATCH_SIZE):
                    batch = missing_samples[i:i + BATCH_SIZE]
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

            # Also push sample filename metadata
            if missing_samples:
                name_batch = []
                for h in missing_samples:
                    meta_path = store.objects_dir / h[:2] / f"{h}.sample"
                    if meta_path.exists():
                        name_batch.append({
                            "hash": h,
                            "name": meta_path.read_text().strip(),
                        })
                if name_batch:
                    try:
                        client.client.post(
                            f"{remote.url}/api/sync/sample-names",
                            json=name_batch,
                            timeout=30,
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

    # pull_from_remote already called by cmd_pull before us;
    # we just handle blob downloading.

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
        missing_samples: set[str] = set()
        current = proj.head
        seen: set[str] = set()

        while current:
            if current in seen:
                break
            seen.add(current)
            snap = store.load_snapshot(current)
            if not snap:
                break

            # Content blob is stored under content_hash (parsed JSON)
            if snap.content_hash and not store.has_object(snap.content_hash):
                missing_content.add(snap.content_hash)
            # Raw .als blob = snapshot identity (also als_hash field for compat)
            if snap.als_hash and not store.has_object(snap.als_hash):
                missing_als.add(snap.als_hash)
            for sh in (snap.sample_hashes or []):
                if not store.has_object(sh):
                    missing_samples.add(sh)

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
                    for sh in s.get("sample_hashes", []):
                        if not store.has_object(sh):
                            missing_samples.add(sh)
        except Exception:
            pass

        # Download missing content blobs from the relay
        if missing_content:
            print(f"    ⬇️  Downloading {len(missing_content)} content blob(s)...")
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
        if missing_als:
            print(f"    ⬇️  Downloading {len(missing_als)} .als backup(s)...")
        for h in list(missing_als):
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
            except Exception as e:
                print(f"    ⚠️  Could not fetch .als backup {h[:12]}: {e}")

        # Download missing audio samples
        if missing_samples:
            print(f"    ⬇️  Downloading {len(missing_samples)} audio sample(s)...")
        for h in list(missing_samples):
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
            except Exception:
                pass

        # Fetch sample filenames for downloaded samples
        if missing_samples:
            try:
                hash_list = ",".join(list(missing_samples)[:100])
                r = client.client.get(
                    f"{remote.url}/api/sync/sample-names",
                    params={"hashes": hash_list},
                    timeout=30,
                )
                if r.status_code == 200:
                    names = r.json()
                    for h, name in names.items():
                        meta_path = store.objects_dir / h[:2] / f"{h}.sample"
                        meta_path.parent.mkdir(parents=True, exist_ok=True)
                        meta_path.write_text(name)
            except Exception:
                pass

    finally:
        client.close()

    # Always materialize the latest snapshot to project folder after any pull
    try:
        head = proj.head
        if head:
            snap = store.load_snapshot(head)
            if snap and snap.als_hash:
                raw = store.get_object(snap.als_hash)
                if raw:
                    project_name = proj.name.replace(" ", " ")
                    # Use Ableton project subfolder convention (matching TUI _run_open)
                    # Always from get_projects_dir — never nest based on prior root_als
                    base_dir = get_projects_dir() / project_name
                    als_dir = base_dir / f"{project_name} Project"
                    out = als_dir / f"{project_name}.als"
                    out.parent.mkdir(parents=True, exist_ok=True)
                    # Create Ableton project folder scaffolding so Ableton
                    # recognizes this as a valid project folder
                    (als_dir / "Samples").mkdir(exist_ok=True)
                    (als_dir / "Backup").mkdir(exist_ok=True)
                    (als_dir / "Ableton Project Info").mkdir(exist_ok=True)

                    # Materialize samples first — use .als RelativePath for correct dir structure
                    if snap.sample_hashes:
                        # Extract filename → RelativePath mapping from the original .als
                        import gzip as _gzip, re as _re
                        _xml = _gzip.decompress(raw).decode("utf-8", errors="replace")
                        _als_relpaths: dict[str, str] = {}
                        for _m in _re.finditer(r'<RelativePath\s+Value=\"([^\"]+)\"', _xml):
                            _rp = _m.group(1)
                            _fn = _rp.rsplit("/", 1)[-1].rsplit("\\", 1)[-1]  # filename only
                            if _fn not in _als_relpaths:
                                _als_relpaths[_fn] = _rp

                        print(f"    📁 Materializing {len(snap.sample_hashes)} sample(s)...")
                        for sh in snap.sample_hashes:
                            fname = store.get_sample_filename(sh)
                            # Prefer .als RelativePath (preserves subdirectory structure),
                            # fall back to relay's stored path
                            relpath = _als_relpaths.get(fname) or store.get_sample_relpath(sh) or ""
                            if fname and store.has_object(sh):
                                try:
                                    store.materialize_sample(sh, out.parent, fname, relpath)
                                except Exception:
                                    pass

                    # Path rewriting DISABLED (May 2026).
                    # rewrite_als_sample_paths() corrupts .als files, causing Ableton crashes.
                    # Samples are materialized alongside the .als; Ableton auto-resolves
                    # when you point it at any one sample in the project folder.
                    # Do NOT re-enable without validating output .als opens in Ableton
                    # on both macOS and Windows.
                    out.write_bytes(raw)
                    # Update project root_als so future snapshots find the .als
                    proj.root_als = str(out)
                    store.set_index(proj)
                    print(f"   📁 Project folder → {out.parent}")
    except Exception as e:
        print(f"    ⚠️  Materialization failed: {e}")

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
            "als_hash": snap.als_hash,
            "content_hash": snap.content_hash,
            "sample_hashes": snap.sample_hashes,
            "sample_paths": snap.sample_paths,
        })
        if snap.parent == current:
            break
        current = snap.parent
    return history


def push_to_remote(store: BlobStore, proj: ClavusProject, remote: Remote, force: bool = False) -> dict:
    """Push all data to a remote. Returns summary.

    ORDER MATTERS: snapshots → blobs → cues.
    Snapshots create the project on the relay and establish the history chain.
    If they fail (network or 409 conflict), nothing lands — clean stop.
    Cues go last because they reference snapshots; by the time they arrive,
    the project and its history already exist on the relay.
    """
    result = {"cues": 0, "snapshots": 0, "error": ""}
    client = SyncClient(remote.url)

    try:
        print(f"  🔗 Connecting...")
        if not client.ping():
            result["error"] = f"Cannot reach {remote.url}"
            print(f"  ❌ Cannot reach {remote.url}")
            return result
        print(f"  ✅ Connected")

        # ── Phase 1: Snapshots (establishes project + history on relay) ──
        snap_data = _snapshots_to_dicts(store, proj)
        print(f"  📸 Pushing {len(snap_data)} snapshot(s)...")
        ok, conflict, relay_head = client.push_snapshots(proj.name, snap_data,
                                              expected_parent=None if force else remote.last_head)
        if snap_data:
            result["snapshots"] = len(snap_data) if ok else 0
        if conflict:
            result["error"] = conflict
            # Auto-update last_head from relay's 409 response
            if relay_head:
                remote.last_head = relay_head
                remotes = load_remotes(store)
                for r in remotes:
                    if r.url.rstrip("/") == remote.url.rstrip("/"):
                        r.last_head = relay_head
                        r.last_sync = time.time()
                        break
                save_remotes(store, remotes)
        elif not ok and not result["error"]:
            result["error"] = "Failed to push snapshots"
        print(f"  {'✅' if ok else '❌'} Snapshots: {result['snapshots']} sent")

        # If snapshots failed (conflict or network), don't push blobs or cues
        if not ok:
            return result

        # ── Phase 3: Cues (safe to push now — project exists on relay) ──
        cues_store = CueStore(proj.name, store=store)
        cues_data = _cues_to_dicts(cues_store)
        print(f"  📋 Pushing {len(cues_data)} cue(s)...")
        ok_cues = client.push_cues(proj.name, cues_data)
        if cues_data:
            result["cues"] = len(cues_data) if ok_cues else 0
        if not ok_cues:
            # Cues failed but snapshots already landed — note it, don't overwrite error
            if not result["error"]:
                result["error"] = "Snapshots synced but cues failed — retry push"
        print(f"  {'✅' if ok_cues else '❌'} Cues: {result['cues']} sent")

        if ok:
            remote.last_head = proj.head
        remote.last_sync = time.time()
        # Save by reloading from disk and patching the matching remote
        remotes = load_remotes(store)
        for r in remotes:
            if r.url.rstrip("/") == remote.url.rstrip("/"):
                r.last_head = remote.last_head
                r.last_sync = remote.last_sync
                break
        save_remotes(store, remotes)
    finally:
        client.close()

    return result


def pull_from_remote(store: BlobStore, proj: ClavusProject, remote: Remote, output_dir: Optional[str] = None) -> dict:
    """Pull all data from a remote. Returns summary.
    
    Args:
        output_dir: Override projects directory for materialization.
                    Defaults to config's projects_dir (~/Clavus/Projects/).
    """
    result = {"cues": 0, "snapshots": 0, "error": ""}
    client = SyncClient(remote.url)

    try:
        print(f"  🔗 Connecting...")
        if not client.ping():
            result["error"] = f"Cannot reach {remote.url}"
            print(f"  ❌ Cannot reach {remote.url}")
            return result
        print(f"  ✅ Connected")

        print(f"  📥 Fetching data...")
        data = client.pull(proj.name)
        if not data:
            result["error"] = "Pull returned no data"
            print(f"  ❌ Pull returned no data")
            return result

        cues_count = len(data.get("cues", []))
        snap_count = len(data.get("snapshots", []))
        print(f"  📋 Received {cues_count} cue(s), {snap_count} snapshot(s)")

        cues_store = CueStore(proj.name, store=store)

        # Import cues
        conflict_count = 0
        if cues_count:
            print(f"  📝 Importing {cues_count} cue(s)...")
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
            status = cues_store.import_cue(cue)
            if status == "conflict":
                conflict_count += 1

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
        if result["cues"]:
            msg = f"  ✅ Imported {result['cues']} cue(s)"
            if conflict_count:
                msg += f" — ⚠ {conflict_count} conflict(s)"
            print(msg)
        result["conflicts"] = conflict_count

        # Import snapshots
        snapshots_data = data.get("snapshots", [])
        snap_conflicts = 0
        if snapshots_data:
            print(f"  📸 Importing {len(snapshots_data)} snapshot(s)...")
        for s in snapshots_data:
            remote_msg = s.get("message", "")
            # Check for message conflict before overwriting
            meta_dir = store.objects_dir / s.get("full_hash", s["hash"])[:2]
            meta_dir.mkdir(parents=True, exist_ok=True)
            meta_path = meta_dir / f"{s.get('full_hash', s['hash'])}.meta"
            local_msg = ""
            if meta_path.exists():
                local_meta = json.loads(meta_path.read_text())
                local_msg = local_meta.get("message", "")
            snap = Snapshot(
                hash=s.get("full_hash", s["hash"]),
                timestamp=s.get("timestamp", 0.0),
                message=s.get("message", ""),
                parent=s.get("parent", None),
                project_path=s.get("project_path", ""),
                track_count=s.get("track_count", 0),
                bpm=s.get("bpm", 120.0),
                tags=s.get("tags", []),
                als_hash=s.get("als_hash", None),
                content_hash=s.get("content_hash", None),
                sample_hashes=s.get("sample_hashes", []),
                sample_paths=s.get("sample_paths", {}),
            )
            # Message conflict detection — same pattern as cue conflicts
            if local_msg and remote_msg and local_msg != remote_msg:
                snap.message = local_msg            # keep local
                snap.conflict_message = remote_msg  # store remote as conflict
                snap_conflicts += 1
            from dataclasses import asdict
            meta_path.write_text(json.dumps(asdict(snap), indent=2, default=str))

            # Always update .sample files from sample_paths (ensures relative paths)
            for sh, rel in snap.sample_paths.items():
                spath = store.objects_dir / sh[:2] / f"{sh}.sample"
                spath.parent.mkdir(parents=True, exist_ok=True)
                fname = Path(rel).name
                spath.write_text(f"{fname}\n{rel}")

        result["snapshots"] = len(snapshots_data)
        result["snap_conflicts"] = snap_conflicts
        if result["snapshots"]:
            msg = f"  ✅ Imported {result['snapshots']} snapshot(s)"
            if snap_conflicts:
                msg += f" — ⚠ {snap_conflicts} message conflict(s)"
            print(msg)

        # Update HEAD to the newest pulled snapshot
        relay_head = None
        if result["snapshots"] > 0:
            snap_list = data.get("snapshots", [])
            # Pick the snapshot with the newest timestamp
            newest = max(snap_list, key=lambda s: s.get("timestamp", 0))
            relay_head = newest.get("full_hash", newest.get("hash", ""))
            if relay_head:
                current_head = store.read_ref("HEAD")
                current_time = 0
                if current_head:
                    old_snap = store.load_snapshot(current_head)
                    if old_snap:
                        current_time = old_snap.timestamp
                newest_time = newest.get("timestamp", 0)
                if newest_time > current_time or not current_head:
                    store.update_ref("HEAD", relay_head)
                    proj.head = relay_head
                    store.set_index(proj)

        # last_head MUST track relay HEAD for optimistic lock, not local HEAD
        remote.last_head = relay_head if relay_head else proj.head
        remote.last_sync = time.time()
        # Save by reloading from disk and patching the matching remote
        remotes = load_remotes(store)
        for r in remotes:
            if r.url.rstrip("/") == remote.url.rstrip("/"):
                r.last_head = remote.last_head
                r.last_sync = remote.last_sync
                break
        save_remotes(store, remotes)
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
