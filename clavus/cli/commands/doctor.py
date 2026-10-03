from __future__ import annotations

"""Clavus CLI command: doctor — Diagnose Clavus store health."""
import argparse

def cmd_doctor(args: argparse.Namespace) -> None:
    """Diagnose Clavus store health — read-only check."""
    from clavus.store import BlobStore
    from clavus.sync import load_remotes, Remote
    from clavus.config import ClavusConfig
    from pathlib import Path
    import subprocess, json
    import socket
    import urllib.request
    import urllib.error
    import urllib.parse

    store = BlobStore()
    cfg = ClavusConfig.load()
    ok: list[str] = []
    warn: list[str] = []
    fail: list[str] = []
    info: list[str] = []

    def check(label: str, cond: bool, detail: str = "") -> None:
        sym = "[ok]" if cond else "--"
        msg = f"  {sym} {label}"
        if detail:
            msg += f" — {detail}"
        if cond:
            ok.append(msg)
        else:
            fail.append(msg)

    def check_info(label: str, detail: str = "") -> None:
        """Informational check — always shows a [ok] or [TIP], never --."""
        msg = f"  [TIP] {label}"
        if detail:
            msg += f": {detail}"
        info.append(msg)

    def warn_check(label: str, detail: str = "") -> None:
        msg = f"  [WARN]  {label}"
        if detail:
            msg += f" — {detail}"
        warn.append(msg)

    print(f"Clavus Doctor")
    print(f"   Store: {store.root}")
    print()

    # 1. Index JSON integrity
    print("  index.json:")
    if not store.index_path.exists():
        check("index.json exists", False, "file missing")
    else:
        try:
            data = json.loads(store.index_path.read_text())
            if not isinstance(data, dict):
                check("index.json valid", False, "not a dict")
            else:
                projects = [k for k in data if k != "_last_project"]
                last = data.get("_last_project", "(none)")
                check("index.json valid", True, f"{len(projects)} project(s), last: {last}")
                for name in projects:
                    p = data.get(name)
                    if not p or not isinstance(p, dict):
                        warn_check(f"  {name}", "corrupt entry (null)")
                        continue
                    als = p.get("root_als", "")
                    als_ok = Path(als).exists() if als else False
                    head = (p.get("head") or "")[:12] or "(none)"
                    sym = "[ok]" if als_ok else "[WARN]"
                    status = "file found" if als_ok else "file missing"
                    (ok if als_ok else warn).append(f"  {sym} {name}  @ {head}  {als[:50] if als else '(no path)'}  [{status}]")
        except (json.JSONDecodeError, OSError) as e:
            check("index.json valid", False, str(e))

    # 2. Backup files
    print("  backups:")
    backups = store.list_backups()
    if backups:
        check("backups available", True, f"{len(backups)} — latest: {backups[0].name}")
    else:
        warn_check("backups", "none available — run 'clavus backup'")

    # 3. HEAD ref
    print("  refs:")
    head = store.read_ref("HEAD")
    if head:
        check("HEAD ref", True, head[:16] + "...")
    else:
        warn_check("HEAD ref", "none set")

    # 4. Blob directory structure
    print("  blobs:")
    obj_count = sum(1 for f in store.objects_dir.rglob("*") if f.is_file())
    check("objects directory", store.objects_dir.exists(), f"{obj_count} blob(s)")
    # Check subdir structure (first 2 chars)
    subdirs = list(store.objects_dir.glob("*"))
    if subdirs:
        valid_sub = all(f.is_dir() and len(f.name) == 2 for f in subdirs)
        check("blob subdirectory scheme", valid_sub, f"{len(subdirs)} subdir(s)")
    elif obj_count > 0:
        warn_check("blob subdirs", "objects exist but no subdirs found")

    # 5. Cues
    print("  cues:")
    cues_root = store.root / "cues"
    if cues_root.exists():
        cue_count = sum(1 for f in cues_root.rglob("*.json"))
        check("cues directory", True, f"{cue_count} file(s)")
    else:
        warn_check("cues directory", "none")

    # 6. Remotes — connectivity
    print("  remotes:")
    try:
        remotes = load_remotes(store)
    except Exception as e:
        warn_check("remotes", f"could not load: {e}")
        remotes = []
    if not remotes:
        warn_check("remotes", "none configured")
    for remote in remotes:
        url = remote.url.rstrip("/")
        ping_ok = False
        try:
            req = urllib.request.Request(f"{url}/api/ping")
            urllib.request.urlopen(req, timeout=5)
            ping_ok = True
        except (urllib.error.URLError, socket.timeout, OSError):
            pass
        # Extract host for display
        try:
            host = urllib.parse.urlparse(url).netloc
        except Exception:
            host = url
        # localhost relay is optional — show as info, not failure
        is_localhost = host in ("localhost", "localhost:7890", "127.0.0.1", "127.0.0.1:7890")
        if ping_ok:
            check(f"remote '{remote.name}' ({host})", True, "reachable")
        elif is_localhost:
            check_info(f"relay '{remote.name}' ({host})", f"not running — start with 'clavus share' (optional for LAN collab)")
        else:
            check(f"remote '{remote.name}' ({host})", False, "unreachable")

    # 7. Tailscale MagicDNS — resolve each remote's hostname
    print("  network:")
    for remote in remotes:
        try:
            parsed = urllib.parse.urlparse(remote.url)
            if parsed.netloc:
                # Use parsed.hostname (without port) for DNS resolution
                host = parsed.hostname or parsed.netloc.split(":")[0]
                addr = socket.gethostbyname(host)
                ok.append(f"  [ok] '{host}' — {addr}")
        except socket.gaierror:
            try:
                parsed = urllib.parse.urlparse(remote.url)
                host = parsed.hostname or parsed.netloc.split(":")[0]
                warn.append(f"  [WARN]  '{host}' — could not resolve")
            except Exception:
                warn.append(f"  [WARN]  remote '{remote.name}' — could not parse URL")

    # 8. Tailscale serve — the relay's tailnet gateway (informational only)
    print("  tailnet relay proxy:")
    port = cfg.port or 7890
    ts_serve_ok = False
    try:
        r = subprocess.run(
            ["tailscale", "serve", "status"],
            capture_output=True, text=True, timeout=5,
        )
        if r.returncode == 0 and "No serve config" not in r.stdout:
            ts_serve_ok = True
            check("tailscale serve", True, f"proxy active on port {port}")
        else:
            # Not running — informational only, not a failure
            check_info("tailscale serve", f"not configured (run 'tailscale serve --bg --http {port} http://localhost:{port}' to enable MagicDNS relay)")
    except FileNotFoundError:
        check_info("tailscale", "not installed — LAN-only collab (Tailscale enables remote relay)")
    except Exception as e:
        check_info("tailscale serve", str(e))

    # 9. Relay process + end-to-end MagicDNS reachability
# 9. Relay process — informational (optional for LAN-only collab)
    print("  relay:")
    relay_alive = False
    relay_port = port
    for check_port in (port, 7891) if port != 7891 else (7891,):
        try:
            req = urllib.request.Request(f"http://127.0.0.1:{check_port}/api/ping")
            urllib.request.urlopen(req, timeout=3)
            relay_alive = True
            relay_port = check_port
            check("relay process", True, f"running on port {check_port}")
            break
        except Exception:
            continue
    if not relay_alive:
        # Informational only — relay is optional for LAN-only usage
        check_info("relay process", f"not running (run 'clavus share' to start it — required for remote collab")

    # Check if MagicDNS URL is actually reachable via tailnet (not just DNS-resolved)
    ts_host: str | None = None
    if ts_serve_ok and relay_alive:
        try:
            r = subprocess.run(
                ["tailscale", "status", "--json"],
                capture_output=True, text=True, timeout=5,
            )
            if r.returncode == 0:
                dns = json.loads(r.stdout).get("Self", {}).get("DNSName", "")
                if dns:
                    ts_host = dns.rstrip(".")
            if ts_host:
                proxy_url = f"http://{ts_host}:{port}/api/ping"
                req2 = urllib.request.Request(proxy_url)
                urllib.request.urlopen(req2, timeout=5)
                check("MagicDNS proxy", True, f"http://{ts_host}:{port}/ — reachable via tailnet")
        except Exception:
            warn_check("MagicDNS proxy", f"http://{ts_host}:{port}/ — http request failed")
            warn.append(f"  [TIP] Verify from another tailnet machine: curl http://{ts_host or '?'}:{port}/api/ping")

    # Summary
    print()
    print(f"  Summary: {len(ok)} [ok]  {len(warn)} [WARN]  {len(fail)} --")
    for line in fail:
        print(line)
    for line in warn:
        print(line)
    for line in info:
        print(line)
    print()
    if fail:
        print(f"  [TIP] Run 'clavus repair' to recover from corruption")
        print(f"  [TIP] Run 'clavus restore-store' to restore from backup")
        print()
        print(f"  [WARN]  Some checks failed — collation may be limited until resolved.")
    elif warn:
        print(f"  [TIP] Run 'clavus backup' to create a full backup")
        print()
        print(f"  [ok] Core system healthy — ready to collab.")
    else:
        print(f"  [ok] Everything looks good!")
        print(f"  [ok] Ready to collab.")


