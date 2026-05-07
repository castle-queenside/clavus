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
    """Scan the LAN for Clavus servers.

    Args:
        timeout: Max seconds to wait for responses

    Returns:
        List of discovered ClavusPeer objects
    """
    listener = ClavusListener()
    zc = zeroconf.Zeroconf()

    try:
        browser = zeroconf.ServiceBrowser(zc, SERVICE_TYPE, listener)
        time.sleep(timeout)
        browser.cancel()
    finally:
        zc.close()

    return list(listener.peers.values())


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
    for base in [TAILSCALE_MAGICDNS, TAILSCALE_LOCALHOST]:
        try:
            import httpx
            r = httpx.get(f"{base}/localapi/v0/{path}", timeout=5)
            if r.status_code == 200:
                return r.json()
        except Exception:
            continue

    # Also try Unix socket
    for sock in ["/var/run/tailscale/tailscaled.sock",
                 "/run/tailscale/tailscaled.sock"]:
        try:
            import httpx
            transport = httpx.HTTPTransport(uds=sock)
            with httpx.Client(transport=transport) as client:
                r = client.get(f"http://localhost/localapi/v0/{path}", timeout=5)
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
        for port in [7890]:
            try:
                import httpx
                url = f"http://[{ts_ip}]:{port}" if ":" in ts_ip else f"http://{ts_ip}:{port}"
                r = httpx.get(f"{url}/api/ping", timeout=3)
                if r.status_code == 200:
                    # Get project/user info from share endpoint
                    try:
                        sr = httpx.get(f"{url}/api/share", timeout=3)
                        if sr.status_code == 200:
                            info = sr.json()
                            return ClavusPeer(
                                name=hostname,
                                host=ts_ip,
                                port=port,
                                project=info.get("project", {}).get("name", "") or "",
                                user=info.get("author", "") or "",
                                version=info.get("version", ""),
                                last_seen=_time.time(),
                            )
                    except Exception:
                        pass
                    return ClavusPeer(
                        name=hostname,
                        host=ts_ip,
                        port=port,
                        last_seen=_time.time(),
                    )
            except Exception:
                continue
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
