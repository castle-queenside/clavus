"""
Clavus P2P Transport Layer

Provides:
  - TCPTransport: raw socket P2P, direct machine-to-machine sync
  - HTTPTransport: wraps existing SyncClient, for relay/hub mode

Frame format: [4-byte uint32 length][JSON payload]
Payload always has a "type" field:
  MANIFEST, CONFLICT, PING, WANT, GIVE, GOT, DONE, ERROR

Smoke test: python3 clavus/p2p_transport.py

Conflict Detection (git-style):
  Both sides track HEAD. On connect, the connector sends expected_head
  (what they think the listener has). The listener rejects if it doesn't
  match — CONFLICT frame — preventing silent overwrites. After successful
  sync, both update last_peer_head for the next session.
"""

from __future__ import annotations

import base64
import json
import socket
import struct
import threading
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Callable, Optional


# ─── Constants ────────────────────────────────────────────────────────────────

FRAME_HEADER = struct.Struct("!I")  # big-endian uint32
_MAX_RETRIES = 3
_RETRY_BACKOFF = (0.5, 1.5, 3.0)


# ─── Frame Primitives ─────────────────────────────────────────────────────────

def _send_frame(sock: socket.socket, payload: dict) -> bool:
    """Send a JSON frame with 4-byte length prefix."""
    try:
        data = json.dumps(payload, default=str).encode("utf-8")
        sock.sendall(FRAME_HEADER.pack(len(data)) + data)
        return True
    except Exception:
        return False


def _recv_frame(sock: socket.socket) -> Optional[dict]:
    """Receive a JSON frame. Returns None on disconnect."""
    try:
        header = sock.recv(FRAME_HEADER.size)
        if not header or len(header) < FRAME_HEADER.size:
            return None
        length, = FRAME_HEADER.unpack(header)
        # Reasonable cap: 50 MB
        if length > 50 * 1024 * 1024:
            return None
        body = b""
        while len(body) < length:
            chunk = sock.recv(length - len(body))
            if not chunk:
                return None
            body += chunk
        return json.loads(body.decode("utf-8"))
    except Exception:
        return None


# ─── Peer Manifest ────────────────────────────────────────────────────────────

@dataclass
class PeerManifest:
    """What a peer has at sync time."""
    project: str
    snapshots: list[str]
    blobs: list[str]
    head: Optional[str] = None          # peer's current HEAD hash
    expected_head: Optional[str] = None # what connector thinks listener has


# ─── Frame Helpers ────────────────────────────────────────────────────────────

def frame_manifest(
    project: str,
    snapshots: list[str],
    blobs: list[str],
    head: Optional[str] = None,
    expected_head: Optional[str] = None,
) -> dict:
    return {
        "type": "MANIFEST",
        "project": project,
        "snapshots": snapshots,
        "blobs": blobs,
        "head": head,
        "expected_head": expected_head,
    }


def frame_conflict(head: str, message: str) -> dict:
    return {"type": "CONFLICT", "head": head, "message": message}


def frame_want(sock: socket.socket, hashes: list[str]) -> None:
    _send_frame(sock, {"type": "WANT", "hashes": hashes})


def frame_give(sock: socket.socket, h: str, data: bytes) -> None:
    _send_frame(sock, {
        "type": "GIVE",
        "hash": h,
        "data": base64.b64encode(data).decode("ascii"),
    })


def frame_got(sock: socket.socket, h: str) -> None:
    _send_frame(sock, {"type": "GOT", "hash": h})


def frame_done(sock: socket.socket) -> None:
    _send_frame(sock, {"type": "DONE"})


def frame_error(sock: socket.socket, message: str) -> None:
    _send_frame(sock, {"type": "ERROR", "message": message})


# ─── Transport Protocol ───────────────────────────────────────────────────────

class SyncTransport(ABC):
    """Abstract sync transport."""

    @abstractmethod
    def connect(
        self, host: str, port: int,
    ) -> tuple[Optional[PeerManifest], Optional[socket.socket]]:
        """Connect and exchange manifests. Returns (peer_manifest, socket) or (None, None)."""
        ...

    @abstractmethod
    def listen(self, port: int, callback: Callable[[str, socket.socket], None]) -> None:
        """Start TCP server. callback(project, client_sock) called per connection."""
        ...

    @abstractmethod
    def close(self) -> None:
        ...


# ─── TCP Transport ────────────────────────────────────────────────────────────

class TCPTransport(SyncTransport):
    """Raw-socket P2P transport. No HTTP, no relay.

    Conflict detection via expected_head:
      - connector sends head + expected_head in MANIFEST
      - listener checks: expected_head != current head → CONFLICT, aborts
      - after successful sync, caller updates last_peer_head
    """

    def __init__(
        self,
        project: str,
        snapshots: list[str],
        blobs: list[str],
        head: Optional[str] = None,
        last_peer_head: Optional[str] = None,
    ):
        self.project = project
        self.snapshots = snapshots
        self.blobs = blobs
        self.head = head                              # our current HEAD
        self.last_peer_head = last_peer_head          # what peer had last sync
        self._server_sock: Optional[socket.socket] = None
        self._server_thread: Optional[threading.Thread] = None
        self._running = False
        self._callback: Optional[Callable[[str, socket.socket], None]] = None

    def connect(
        self, host: str, port: int,
    ) -> tuple[Optional[PeerManifest], Optional[socket.socket]]:
        """Connect to a peer, exchange manifests. Caller closes returned socket.

        Sends MANIFEST with our head and expected_head (what we think peer has).
        If listener sends CONFLICT, returns (None, None) with error in manifest.project.
        """
        sock = None
        for attempt in range(_MAX_RETRIES):
            try:
                sock = socket.create_connection((host, port), timeout=10)
                break
            except (socket.timeout, ConnectionRefusedError, OSError):
                if attempt < _MAX_RETRIES - 1:
                    time.sleep(_RETRY_BACKOFF[attempt])
                continue

        if sock is None:
            return (None, None)

        try:
            # Send our manifest with HEAD context
            if not _send_frame(sock, frame_manifest(
                self.project, self.snapshots, self.blobs,
                head=self.head,
                expected_head=self.last_peer_head,
            )):
                sock.close()
                return (None, None)

            peer_frame = _recv_frame(sock)
            if not peer_frame:
                sock.close()
                return (None, None)

            # Handle CONFLICT from listener
            if peer_frame.get("type") == "CONFLICT":
                sock.close()
                # Return a manifest with project as error message for caller
                return (
                    PeerManifest(
                        project=f"CONFLICT: {peer_frame.get('message', 'head divergence')} "
                                f"(peer head: {peer_frame.get('head', '?')[:8]})",
                        snapshots=[],
                        blobs=[],
                        head=peer_frame.get("head"),
                    ),
                    None,
                )

            if peer_frame.get("type") != "MANIFEST":
                sock.close()
                return (None, None)

            return (
                PeerManifest(
                    project=peer_frame.get("project", ""),
                    snapshots=peer_frame.get("snapshots", []),
                    blobs=peer_frame.get("blobs", []),
                    head=peer_frame.get("head"),
                ),
                sock,
            )
        except Exception:
            try:
                sock.close()
            except Exception:
                pass
            return (None, None)

    def listen(self, port: int, callback: Callable[[str, socket.socket], None]) -> None:
        """Start TCP server."""
        self._running = True
        self._callback = callback
        self._server_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server_sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server_sock.bind(("0.0.0.0", port))
        self._server_sock.listen(4)
        self._server_sock.settimeout(2.0)
        self._server_thread = threading.Thread(target=self._accept_loop, daemon=True)
        self._server_thread.start()

    def listen_with_peer_manifest(
        self,
        port: int,
        callback: Callable[[str, socket.socket, "PeerManifest"], None],
    ) -> None:
        """Start TCP server. callback receives (project, sock, peer_manifest).

        Unlike listen(), the callback gets the already-received PeerManifest
        so it can run p2p_sync on the socket without reconnecting.
        """
        self._running = True
        self._callback_with_peer = callback  # type: ignore[assignment]
        self._server_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server_sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server_sock.bind(("0.0.0.0", port))
        self._server_sock.listen(4)
        self._server_sock.settimeout(2.0)
        self._server_thread = threading.Thread(target=self._accept_loop_with_peer, daemon=True)
        self._server_thread.start()

    def _accept_loop(self) -> None:
        while self._running:
            try:
                client_sock, _ = self._server_sock.accept()  # type: ignore[union-attr]
                t = threading.Thread(target=self._handle, args=(client_sock,), daemon=True)
                t.start()
            except socket.timeout:
                continue
            except OSError:
                break

    def _accept_loop_with_peer(self) -> None:
        """Accept loop for listen_with_peer_manifest — passes peer_manifest to callback."""
        while self._running:
            try:
                client_sock, _ = self._server_sock.accept()  # type: ignore[union-attr]
                t = threading.Thread(
                    target=self._handle_with_peer, args=(client_sock,), daemon=True
                )
                t.start()
            except socket.timeout:
                continue
            except OSError:
                break

    def _handle_with_peer(self, sock: socket.socket) -> None:
        """Handle one incoming connection. Passes PeerManifest to callback."""
        try:
            peer_frame = _recv_frame(sock)
            if not peer_frame or peer_frame.get("type") != "MANIFEST":
                return

            peer_head = peer_frame.get("head")
            expected_head = peer_frame.get("expected_head")

            # Conflict detection
            if expected_head is not None and self.head is not None:
                if expected_head != self.head:
                    msg = (
                        f"Head mismatch: you have {expected_head[:8]}, "
                        f"peer has {self.head[:8]} — sync both to same state first"
                    )
                    _send_frame(sock, frame_conflict(self.head, msg))
                    sock.close()
                    return

            peer_manifest = PeerManifest(
                project=peer_frame.get("project", ""),
                snapshots=peer_frame.get("snapshots", []),
                blobs=peer_frame.get("blobs", []),
                head=peer_head,
            )

            # Send our manifest back
            if not _send_frame(sock, frame_manifest(
                self.project, self.snapshots, self.blobs,
                head=self.head,
            )):
                return

            # Call the callback with project, socket, AND peer_manifest
            cb = getattr(self, "_callback_with_peer", None)
            if cb:
                cb(peer_manifest.project, sock, peer_manifest)
        except Exception:
            pass
        finally:
            try:
                sock.close()
            except Exception:
                pass

    def _handle(self, sock: socket.socket) -> None:
        """Handle one incoming connection.

        Checks expected_head from connector against our current HEAD.
        If mismatch → send CONFLICT, close. This prevents silent overwrites.
        """
        try:
            peer_frame = _recv_frame(sock)
            if not peer_frame or peer_frame.get("type") != "MANIFEST":
                return

            peer_head = peer_frame.get("head")
            expected_head = peer_frame.get("expected_head")

            # ── Conflict detection (git-style expected_parent) ──
            if expected_head is not None and self.head is not None:
                if expected_head != self.head:
                    ts = time.strftime("%H:%M", time.localtime())
                    msg = (
                        f"Head mismatch: you have {expected_head[:8]}, "
                        f"peer has {self.head[:8]} — sync both to same state first"
                    )
                    _send_frame(sock, frame_conflict(self.head, msg))
                    sock.close()
                    return

            peer = PeerManifest(
                project=peer_frame.get("project", ""),
                snapshots=peer_frame.get("snapshots", []),
                blobs=peer_frame.get("blobs", []),
                head=peer_head,
            )

            # Send our manifest back
            if not _send_frame(sock, frame_manifest(
                self.project, self.snapshots, self.blobs,
                head=self.head,
            )):
                return

            if self._callback:
                self._callback(peer.project, sock)
        except Exception:
            pass
        finally:
            try:
                sock.close()
            except Exception:
                pass

    def close(self) -> None:
        self._running = False
        if self._server_sock:
            try:
                self._server_sock.close()
            except Exception:
                pass
        if self._server_thread:
            self._server_thread.join(timeout=3)


# ─── P2P Blob Sync ────────────────────────────────────────────────────────────

def p2p_sync(
    sock: socket.socket,
    store,                    # BlobStore
    local_snapshots: list[str],
    local_blobs: list[str],
    local_has: Callable[[str], bool],
    peer_snapshots: list[str],
    peer_blobs: list[str],
    peer_has: Callable[[str], bool],
    on_progress: Callable[[str, str], None] | None = None,
) -> dict:
    """
    Bidirectional blob sync over a live TCP socket.

    Both sides run this identically. Protocol:
      1. MANIFEST exchange (transport layer, before this call)
      2. Both compute diff: what they need
      3. Both send WANT for blobs they need
      4. Both respond to peer's WANT with GIVE
      5. DONE when exchange complete

    Returns {"downloaded": [...], "uploaded": [...], "error": ""}
    """
    result = {"downloaded": [], "uploaded": [], "error": ""}

    # What we need from peer (they have, we don't)
    we_want = [h for h in peer_blobs if not local_has(h)]
    # What peer needs from us (we have, they don't)
    they_want = [h for h in local_blobs if not peer_has(h)]

    print(f"  [P2P] need from peer: {len(we_want)}  |  peer needs from us: {len(they_want)}")

    pending_uploads = set(they_want)
    pending_downloads = set(we_want)

    # Send our wants first
    if we_want:
        frame_want(sock, we_want)

    # Exchange loop
    while pending_uploads or pending_downloads:
        frame = _recv_frame(sock)
        if not frame:
            result["error"] = "Connection closed"
            break

        t = frame.get("type")

        if t == "WANT":
            for h in (frame.get("hashes") or []):
                if h in pending_uploads and local_has(h):
                    data = store.get_object(h)
                    if data:
                        frame_give(sock, h, data)
                        pending_uploads.discard(h)
                        result["uploaded"].append(h)
                        if on_progress:
                            on_progress("upload", h)

        elif t == "GIVE":
            h = frame.get("hash", "")
            data_b64 = frame.get("data", "")
            if h in pending_downloads and data_b64:
                try:
                    data = base64.b64decode(data_b64)
                    store.put_object(data, h)
                    pending_downloads.discard(h)
                    result["downloaded"].append(h)
                    frame_got(sock, h)
                    if on_progress:
                        on_progress("download", h)
                except Exception:
                    pass

        elif t == "GOT":
            pending_uploads.discard(frame.get("hash", ""))

        elif t == "DONE":
            break

        elif t == "ERROR":
            result["error"] = frame.get("message", "peer error")
            break

    frame_done(sock)
    return result


# ─── Peer Discovery ───────────────────────────────────────────────────────────

def discover_peers() -> list[dict]:
    """
    Discover Clavus peers on the tailnet via `tailscale status --json`.
    Returns [{"name", "dns", "ip", "online"}, ...]. Excludes self.
    """
    import subprocess

    try:
        r = subprocess.run(
            ["tailscale", "status", "--json"],
            capture_output=True, text=True, timeout=10,
        )
        if r.returncode != 0:
            return []
        data = json.loads(r.stdout)
    except Exception:
        return []

    peers = []
    self_name = data.get("Self", {}).get("HostName", "")
    for node in data.get("Peer", {}).values():
        dns = node.get("DNSName", "").rstrip(".")
        ip = node.get("TailscaleIPs", [None])[0] or ""
        peers.append({
            "name": node.get("HostName", dns),
            "dns": dns,
            "ip": ip,
            "online": node.get("Online", False),
        })

    return [p for p in peers if p["name"] != self_name]


# ─── Mock BlobStore for Testing ───────────────────────────────────────────────

class MockStore:
    def __init__(self, blobs: dict[str, bytes]):
        self._d = blobs

    def get_object(self, h: str) -> Optional[bytes]:
        return self._d.get(h)

    def put_object(self, data: bytes, h: str) -> None:
        self._d[h] = data

    def has_object(self, h: str) -> bool:
        return h in self._d


# ─── Smoke Tests ──────────────────────────────────────────────────────────────

def _smoke_manifest():
    """Test: two TCPTransport instances exchange manifests."""
    print("\n=== P2P Manifest Exchange ===\n")

    host = TCPTransport("test-project", ["snap_a", "snap_b"], ["blob_1", "blob_2"],
                        head="head_a")
    client = TCPTransport("test-project", ["snap_b", "snap_c"], ["blob_2", "blob_3"],
                          head="head_c")

    connected = {}

    def handler(project: str, sock: socket.socket):
        connected["project"] = project
        sock.close()

    host.listen(7950, handler)
    time.sleep(0.2)

    peer_manifest, sock = client.connect("127.0.0.1", 7950)
    time.sleep(0.3)

    host.close()
    client.close()

    ok = True
    checks = {
        "peer_manifest_received": peer_manifest is not None,
        "host_got_project": connected.get("project") == "test-project",
        "peer_snapshots_correct": list(peer_manifest.snapshots) == ["snap_a", "snap_b"] if peer_manifest else False,
        "peer_blobs_correct": list(peer_manifest.blobs) == ["blob_1", "blob_2"] if peer_manifest else False,
    }
    for name, result in checks.items():
        print(f"  {'PASS' if result else 'FAIL'} {name}")
        if not result:
            ok = False

    print(f"\n  {'ALL PASS' if ok else 'FAILURES'}\n")
    return ok


def _smoke_full_sync():
    """Test: bidirectional blob sync between two peers."""
    print("\n=== P2P Full Blob Sync ===\n")

    # Host has blobs 1,2  |  Client has blobs 2,3
    host_blobs = {"blob_1": b"content1", "blob_2": b"content2"}
    client_blobs = {"blob_2": b"content2", "blob_3": b"content3"}

    host_store = MockStore(host_blobs)
    client_store = MockStore(client_blobs)

    host_snapshots = ["snap_a", "snap_b"]
    client_snapshots = ["snap_b", "snap_c"]

    connected = {}
    peer_manifest_ref = {}

    def handler(project: str, sock: socket.socket, peer_mf: "PeerManifest"):
        """Server-side handler. sock is already connected; peer_mf is already received."""
        connected["project"] = project
        peer_manifest_ref["manifest"] = peer_mf
        # sock is the already-accepted client socket.
        # We already received peer's MANIFEST (peer_mf) — no reconnect needed.
        # Just run p2p_sync directly on this socket.
        from clavus.p2p_transport import p2p_sync
        p2p_sync(
            sock=sock,
            store=host_store,
            local_snapshots=host_snapshots,
            local_blobs=list(host_store._d.keys()),
            local_has=host_store.has_object,
            peer_snapshots=peer_mf.snapshots,
            peer_blobs=peer_mf.blobs,
            peer_has=client_store.has_object,
        )

    host = TCPTransport("test-project", host_snapshots, list(host_store._d.keys()), head="head_ab")
    host.listen_with_peer_manifest(7951, handler)
    time.sleep(0.5)

    client = TCPTransport("test-project", client_snapshots, list(client_store._d.keys()), head="head_bc")
    peer_manifest, sock = client.connect("127.0.0.1", 7951)

    if peer_manifest and sock:
        r = p2p_sync(
            sock=sock,
            store=client_store,
            local_snapshots=client_snapshots,
            local_blobs=list(client_store._d.keys()),
            local_has=client_store.has_object,
            peer_snapshots=peer_manifest.snapshots,
            peer_blobs=peer_manifest.blobs,
            peer_has=host_store.has_object,
        )
        sock.close()
        print(f"  Sync result: {r}")

    time.sleep(0.3)
    host.close()
    client.close()

    ok = True
    checks = {
        "host_got_blob3": "blob_3" in host_store._d,
        "client_got_blob1": "blob_1" in client_store._d,
        "blob2_unchanged_host": host_store._d.get("blob_2") == b"content2",
        "blob2_unchanged_client": client_store._d.get("blob_2") == b"content2",
    }
    for name, result in checks.items():
        print(f"  {'PASS' if result else 'FAIL'} {name}")
        if not result:
            ok = False

    print(f"\n  {'ALL PASS' if ok else 'FAILURES'}\n")
    return ok


def _smoke_conflict():
    """Test: CONFLICT frame when expected_head doesn't match listener's head."""
    print("\n=== P2P Conflict Detection ===\n")

    # Client thinks listener has head_A, but listener actually has head_B
    client = TCPTransport("test-project", ["snap_c"], ["blob_3"],
                          head="head_c",
                          last_peer_head="head_A")   # stale — listener has head_B

    def handler(project: str, sock: socket.socket):
        # Listener with head_B — client expects head_A → CONFLICT
        listener = TCPTransport("test-project", ["snap_a", "snap_b"], ["blob_1", "blob_2"],
                                head="head_B")        # actual head
        listener._callback = lambda p, s: s.close()
        listener._running = True
        # Simulate handle: receive manifest, detect conflict, send CONFLICT
        peer_frame = _recv_frame(sock)
        if peer_frame:
            expected = peer_frame.get("expected_head")
            if expected and expected != "head_B":
                _send_frame(sock, frame_conflict("head_B",
                    f"Head mismatch: you have {expected[:8]}, peer has head_B"))
        sock.close()

    import threading
    server_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server_sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server_sock.bind(("127.0.0.1", 7952))
    server_sock.listen(1)
    server_sock.settimeout(2.0)

    def accept_and_handle():
        try:
            client_sock, _ = server_sock.accept()
            peer_frame = _recv_frame(client_sock)
            if peer_frame:
                expected = peer_frame.get("expected_head")
                if expected and expected != "head_B":
                    _send_frame(client_sock, frame_conflict("head_B",
                        f"Head mismatch: you have {expected[:8]}, peer has head_B"))
            client_sock.close()
        except Exception:
            pass
        finally:
            server_sock.close()

    t = threading.Thread(target=accept_and_handle, daemon=True)
    t.start()

    peer_manifest, sock = client.connect("127.0.0.1", 7952)
    t.join(timeout=3)

    ok = True
    checks = {
        "conflict_detected": peer_manifest is not None and peer_manifest.project.startswith("CONFLICT:"),
        "no_socket_returned": sock is None,
        "peer_head_returned": peer_manifest.head == "head_B" if peer_manifest else False,
    }
    for name, result in checks.items():
        print(f"  {'PASS' if result else 'FAIL'} {name}")
        if not result:
            ok = False

    print(f"\n  {'ALL PASS' if ok else 'FAILURES'}\n")
    return ok


if __name__ == "__main__":
    results = [_smoke_manifest(), _smoke_full_sync(), _smoke_conflict()]
    print("=" * 40)
    print(f"  {'ALL SMOKE TESTS PASS' if all(results) else 'SOME TESTS FAILED'}")
    print("=" * 40)
