"""Clavus — LAN peer discovery via mDNS/DNS-SD.

Find other Clavus servers on the local network and advertise
your own server so others can find you.

Uses zeroconf (pure Python mDNS). Cross-platform: macOS, Linux, Windows.

Service type: _clavus._tcp
TXT records:
  project  = Current project name
  user     = Author name from config
  version  = Clavus version string
"""

from __future__ import annotations

import json
import random
import socket
import time
from dataclasses import dataclass, field
from typing import Optional

import zeroconf

# ─── Service Definition ────────────────────────────────────────────────

SERVICE_TYPE = "_clavus._tcp.local."
SERVICE_NAME = "Clavus"

# ─── Models ────────────────────────────────────────────────────────────


@dataclass
class ClavusPeer:
    """A Clavus server discovered on the network."""

    name: str  # Machine hostname or friendly name
    host: str  # IP address
    port: int  # Server port
    project: str = ""  # Current project name
    user: str = ""  # Author name
    version: str = ""  # Clavus version
    last_seen: float = 0.0  # When discovered

    @property
    def url(self) -> str:
        return f"http://{self.host}:{self.port}"

    def __str__(self) -> str:
        proj = f"  {self.project}" if self.project else ""
        user = f"  [{self.user}]" if self.user else ""
        return f"{self.name:<20} {self.host:>15}:{self.port:<5}{proj}{user}"


# ─── Advertiser ────────────────────────────────────────────────────────


class ClavusAdvertiser:
    """Advertise this Clavus server on the LAN via mDNS.

    Call start(port, project, user) when the web server starts.
    Call stop() when shutting down.
    """

    def __init__(self):
        self._zc: Optional[zeroconf.Zeroconf] = None
        self._service: Optional[zeroconf.ServiceInfo] = None

    def start(self, port: int, project: str = "", user: str = "", version: str = "", share_code: str = ""):
        """Start advertising this Clavus server on the network.

        Args:
            port: The port Clavus web server is running on
            project: Name of the active project
            user: Author name from config
            version: Clavus version string
            share_code: Optional share code for clavus share/join flow
        """
        if self._zc:
            return  # Already advertising

        # Build TXT records
        txt = {
            "project": project or "none",
            "user": user or "anonymous",
            "version": version or "0.5.0",
        }
        if share_code:
            txt["share"] = share_code
        txt_bytes = {k: v.encode("utf-8") for k, v in txt.items()}

        # Get hostname
        hostname = socket.gethostname()

        # Build full service name: "Hostname._clavus._tcp.local."
        fqdn = f"{hostname}.{SERVICE_TYPE}"
        server_name = f"{hostname}.local."

        # Resolve our IP
        try:
            ip = socket.gethostbyname(hostname)
        except socket.gaierror:
            ip = "127.0.0.1"

        self._service = zeroconf.ServiceInfo(
            type_=SERVICE_TYPE,
            name=fqdn,
            addresses=[socket.inet_aton(ip)],
            port=port,
            properties=txt_bytes,
            server=server_name,
        )

        self._zc = zeroconf.Zeroconf()
        self._zc.register_service(self._service)
        print(f"📡 Advertising clavus on LAN: {hostname}:{port} ({project or 'no project'})")

    def update(self, project: str = "", user: str = "", version: str = "", share_code: str = ""):
        """Update TXT records (e.g., when project changes)."""
        if not self._zc or not self._service:
            return

        txt = {
            "project": project or "none",
            "user": user or "anonymous",
            "version": version or "0.5.0",
        }
        if share_code:
            txt["share"] = share_code
        self._service.properties = {k: v.encode("utf-8") for k, v in txt.items()}
        try:
            self._zc.unregister_service(self._service)
            self._zc.register_service(self._service)
        except Exception:
            pass

    def stop(self):
        """Stop advertising."""
        if self._zc and self._service:
            try:
                self._zc.unregister_service(self._service)
            except Exception:
                pass
            self._zc.close()
            self._zc = None
            self._service = None
            print("📡 Stopped LAN advertising")


# ─── Discoverer ────────────────────────────────────────────────────────


class ClavusListener(zeroconf.ServiceListener):
    """Collects discovered Clavus peers from mDNS responses."""

    def __init__(self):
        self.peers: dict[str, ClavusPeer] = {}
        self._done = False

    def add_service(self, zc: zeroconf.Zeroconf, type_: str, name: str):
        info = zc.get_service_info(type_, name)
        if info:
            peer = self._info_to_peer(info)
            self.peers[peer.name] = peer
            self.peers[peer.name].last_seen = time.time()

    def update_service(self, zc: zeroconf.Zeroconf, type_: str, name: str):
        self.add_service(zc, type_, name)

    def remove_service(self, zc: zeroconf.Zeroconf, type_: str, name: str):
        # Extract hostname from service name
        host_part = name.split(".")[0]
        if host_part in self.peers:
            del self.peers[host_part]

    def _info_to_peer(self, info: zeroconf.ServiceInfo) -> ClavusPeer:
        txt = {}
        if info.properties:
            txt = {
                k.decode("utf-8") if isinstance(k, bytes) else k: (
                    v.decode("utf-8") if isinstance(v, bytes) else v
                )
                for k, v in info.properties.items()
            }

        # Get IP address
        ip = "0.0.0.0"
        if info.addresses:
            ip = socket.inet_ntoa(info.addresses[0])

        hostname = info.name.split(".")[0]
        return ClavusPeer(
            name=hostname,
            host=ip,
            port=info.port,
            project=txt.get("project", ""),
            user=txt.get("user", ""),
            version=txt.get("version", ""),
        )

    def done(self):
        self._done = True


def discover_peers(timeout: float = 3.0) -> list[ClavusPeer]:
    """Scan for Clavus servers on Tailscale tailnet + LAN.

    Tries Tailscale first (faster, no broadcast needed on tailnet).
    Falls back to LAN (zeroconf/mDNS) if Tailscale is unavailable.
    Results are merged and deduplicated by hostname.

    Args:
        timeout: Max seconds for the full scan

    Returns:
        List of discovered ClavusPeer objects
    """
    # 1. Try Tailscale peer discovery (faster on tailnet)
    ts_peers: list[ClavusPeer] = []
    try:
        ts_peers = discover_tailscale_peers(timeout=min(timeout, 5.0))
    except Exception:
        pass

    # 2. Fall back to LAN discovery via zeroconf
    lan_peers: list[ClavusPeer] = []
    try:
        listener = ClavusListener()
        zc = zeroconf.Zeroconf()
        try:
            browser = zeroconf.ServiceBrowser(zc, SERVICE_TYPE, listener)
            time.sleep(timeout)
            browser.cancel()
        finally:
            zc.close()
        lan_peers = list(listener.peers.values())
    except Exception:
        pass

    # 3. Merge results, dedup by hostname (Tailscale takes priority)
    seen: dict[str, ClavusPeer] = {}
    for peer in ts_peers:
        seen[peer.name] = peer
    for peer in lan_peers:
        if peer.name not in seen:
            seen[peer.name] = peer

    return list(seen.values())


# ─── Share Codes ───────────────────────────────────────────────────────


SHARE_ADJECTIVES = [
    "BRIGHT", "RAPID", "DEEP", "WILD", "COOL", "FAST", "SOFT", "HOT",
    "CALM", "DARK", "LIGHT", "FRESH", "BOLD", "WARM", "COLD", "PURE",
    "GLOW", "ZEN", "RICH", "LOUD", "FLAT", "JAZZ", "FUNK", "GROOVE",
]

SHARE_NOUNS = [
    "DUCK", "FISH", "WOLF", "BEAR", "OWL", "FOX", "ELK", "HARE",
    "MOON", "STAR", "WAVE", "TREE", "ROCK", "FIRE", "MIST", "SUN",
    "NOTE", "BEAT", "RIFT", "LOOP", "BASS", "DRUM", "HORN", "KEY",
]

SHARE_COLORS = [
    "RED", "BLUE", "TEA", "GOLD", "LIME", "ROSE", "ICE", "TEAL",
]


def generate_share_code() -> str:
    """Generate a human-friendly 3-word share code.

    Format: ADJ-NOUN-NUM (e.g., BRIGHT-DUCK-7, WARM-LOOP-3)
    The words are chosen so they're easy to say over voice/chat.
    The number adds enough entropy to avoid collisions in small groups.
    """
    adj = random.choice(SHARE_ADJECTIVES)
    noun = random.choice(SHARE_NOUNS)
    num = random.randint(1, 9)
    return f"{adj}-{noun}-{num}"


def scan_for_share_codes(
    timeout: float = 5.0,
    scan_tailscale: bool = True,
    scan_lan: bool = True,
) -> list[ClavusPeer]:
    """Scan LAN and/or Tailscale for Clavus relays with share codes.

    Each peer's project field may contain a share code in the format
    ADJ-NOUN-NUM if it's in share mode. The user can match visually.

    Args:
        timeout: Max seconds for the full scan
        scan_tailscale: Whether to scan Tailscale tailnet
        scan_lan: Whether to scan LAN via mDNS

    Returns:
        List of discovered ClavusPeer objects
    """
    import concurrent.futures
    all_peers: list[ClavusPeer] = []

    def _scan_lan():
        try:
            return discover_peers(timeout=timeout)
        except Exception:
            return []

    def _scan_tailscale():
        try:
            return discover_tailscale_peers(timeout=timeout)
        except Exception:
            return []

    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures = []
        if scan_lan:
            futures.append(pool.submit(_scan_lan))
        if scan_tailscale:
            futures.append(pool.submit(_scan_tailscale))

        for fut in concurrent.futures.as_completed(futures, timeout=timeout + 2):
            try:
                all_peers.extend(fut.result())
            except Exception:
                pass

    return all_peers


# ─── Tailscale Discovery ───────────────────────────────────────────────


TAILSCALE_LOCALHOST = "http://100.100.100.100:8080"
TAILSCALE_MAGICDNS = "http://tailscale"


def _tailscale_api(path: str) -> Optional[dict]:
    """Call the Tailscale local API.

    Tries MagicDNS first, then the localhost IP.
    Returns parsed JSON or None on failure.
    """
    from clavus.http_client import http_get
    for base in [TAILSCALE_MAGICDNS, TAILSCALE_LOCALHOST]:
        try:
            data, err, status = http_get(f"{base}/localapi/v0/{path}", timeout=5.0)
            if status == 200 and data:
                return data
        except Exception:
            continue

    # Also try Unix socket (httpx required for UDS transport — no stdlib equivalent)
    for sock in ["/var/run/tailscale/tailscaled.sock",
                 "/run/tailscale/tailscaled.sock"]:
        try:
            import httpx
            transport = httpx.HTTPTransport(uds=sock)
            with httpx.Client(transport=transport, timeout=5.0) as client:
                r = client.get(f"http://localhost/localapi/v0/{path}")
                if r.status_code == 200:
                    return r.json()
        except Exception:
            continue

    return None


def discover_tailscale_peers(timeout: float = 5.0) -> list[ClavusPeer]:
    """Scan the Tailscale tailnet for Clavus servers.

    Uses Tailscale's local API to list all devices in the tailnet,
    then pings each one's clavus server to check if it's running.

    Args:
        timeout: Max seconds for the full scan

    Returns:
        List of discovered ClavusPeer objects (only online Clavus servers)
    """
    import time as _time
    import concurrent.futures

    status = _tailscale_api("status")
    if not status:
        return []  # Tailscale not running

    peers: list[ClavusPeer] = []
    device_list = []

    # Collect all devices (peers only — skip self since we're scanning for OTHERS)
    for peer_id, peer_data in status.get("Peer", {}).items():
        if isinstance(peer_data, dict):
            device_list.append((peer_id, peer_data))

    def _check_device(device_info: tuple) -> Optional[ClavusPeer]:
        peer_id, data = device_info
        hostname = data.get("DNSName", "").split(".")[0] or data.get("HostName", peer_id)
        ips = data.get("TailscaleIPs", [])
        if not ips:
            return None

        ts_ip = ips[0]
        # Try default Clavus port
        from clavus.http_client import http_get
        for port in [7890]:
            url = f"http://[{ts_ip}]:{port}" if ":" in ts_ip else f"http://{ts_ip}:{port}"
            data, err, status = http_get(f"{url}/api/ping", timeout=3.0)
            if status == 200 and data:
                # Get project/user info from share endpoint
                share_data, _, share_status = http_get(f"{url}/api/share", timeout=3.0)
                if share_status == 200 and share_data:
                    return ClavusPeer(
                        name=hostname,
                        host=ts_ip,
                        port=port,
                        project=share_data.get("project", {}).get("name", "") or "",
                        user=share_data.get("author", "") or "",
                        version=share_data.get("version", ""),
                        last_seen=_time.time(),
                    )
                return ClavusPeer(
                    name=hostname,
                    host=ts_ip,
                    port=port,
                    last_seen=_time.time(),
                )
        return None
        return None

    start = _time.time()
    remaining = max(1.0, timeout)
    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as pool:
        fut_map = {pool.submit(_check_device, d): d for d in device_list}
        for fut in concurrent.futures.as_completed(fut_map, timeout=remaining):
            try:
                result = fut.result()
                if result:
                    peers.append(result)
            except Exception:
                pass

    return peers
