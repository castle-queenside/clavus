from __future__ import annotations

"""Clavus CLI command: join — Connect to a share session."""
import argparse
from clavus.discovery import discover_peers
from clavus.helpers import get_store_and_project

def cmd_join(args: argparse.Namespace) -> None:
    """Guided onboarding — connect to a Clavus collaboration session.

    Handles bare hostnames, MagicDNS, IPs, and full URLs.
    Checks prerequisites (Tailscale, Ableton) and provides clear
    diagnostic messages with specific fix instructions.
    """
    import platform, socket, subprocess, time, os
    from clavus.sync import save_remotes, Remote, load_remotes, pull_from_remote, pull_snapshot_blobs, SyncClient
    from clavus.store import BlobStore, ClavusProject
    from urllib.parse import urlparse

    # ── Welcome ──────────────────────────────────────────────────────
    print()
    print("[KEYBOARD]  Clavus — Join a Collaboration Session")
    print("═" * 48)
    print()

    # ── Phase 0: No URL? Show how to get one ─────────────────────────
    if not args.code:
        print("👋  Welcome! To join a session, ask the session host for their relay URL.")
        print("    They'll see it when they run 'clavus share'. It looks like:")
        print()
        print("      http://machine-name.tailXXXX.ts.net:7890   (Tailscale)")
        print("      http://192.168.1.50:7890                   (LAN)")
        print()
        print("    Then run:")
        print("      clavus join http://machine-name.tailXXXX.ts.net:7890")
        print()
        print("    [TIP]  You can also scan your network for active relays:")
        print("      clavus find             — LAN discovery")
        print("      clavus find --tailscale — Tailscale discovery")
        print()
        return

    # ── Phase 1: Parse URL (auto-wrap bare hostnames) ─────────────────
    raw = args.code.rstrip("/")
    if not (raw.startswith("http://") or raw.startswith("https://")):
        # Bare hostname, IP, or MagicDNS — auto-wrap
        if ":" in raw:
            raw = f"http://{raw}"  # already has port
        else:
            raw = f"http://{raw}:7890"

    parsed = urlparse(raw)
    host = parsed.hostname or "localhost"
    port = parsed.port or 7890
    base = f"http://{host}:{port}"

    # ── Phase 2: Prerequisites Check ──────────────────────────────────
    print("🔍  Checking prerequisites...")
    print()

    # 2a. Clavus version
    try:
        from importlib.metadata import version
        v = version("clavus")
    except ImportError:
        v = "dev"
    print(f"    [ok]  Clavus {v}")

    # 2b. Ableton detection (for :open in TUI)
    ableton = None
    system = platform.system()
    if system == "Darwin":
        for name in ["Ableton Live 12 Suite", "Ableton Live 12 Intro",
                      "Ableton Live 11 Suite", "Ableton Live 11 Intro"]:
            p = f"/Applications/{name}.app"
            if os.path.exists(p):
                ableton = p
                break
    elif system == "Windows":
        try:
            import winreg
            for ver in [12, 11]:
                try:
                    key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE,
                                        rf"SOFTWARE\Ableton\Live {ver}")
                    ableton = winreg.QueryValueEx(key, "InstallDir")[0]
                    break
                except Exception:
                    pass
        except Exception:
            pass

    if ableton:
        print(f"    [ok]  Ableton detected")
    else:
        print(f"    [WARN]  Ableton not auto-detected — sync still works,")
        print(f"        but :open in the TUI won't launch Ableton.")

    # 2c. Tailscale check
    is_tailscale_addr = ".ts.net" in host or (host.startswith("100.") and "." in host[4:])

    ts_running = False
    ts_ip = ""
    try:
        r = subprocess.run(["tailscale", "version"], capture_output=True,
                          text=True, timeout=5)
        if r.returncode == 0:
            ts_running = True
            r2 = subprocess.run(["tailscale", "ip", "-4"], capture_output=True,
                              text=True, timeout=5)
            ts_ip = r2.stdout.strip()
    except (FileNotFoundError, subprocess.TimeoutExpired):
        pass

    if ts_running:
        print(f"    [ok]  Tailscale running ({ts_ip})")
    elif is_tailscale_addr:
        print(f"    --  Tailscale required — not installed or not running")
        print(f"        Download: https://tailscale.com/download")
        print(f"        Start it: tailscale up")
        print()
        print(f"    [WARN]  Fix this, then re-run: clavus join {raw}")
        return
    else:
        print(f"    [info]  Tailscale not detected — LAN-only collaboration")

    # 2d. Storage init
    store = BlobStore()
    store.init()
    print(f"    [ok]  Storage ready")
    print()

    # ── Phase 3: Test Connectivity ───────────────────────────────────
    print(f"Connecting to {host}:{port}...")
    print()

    # 3a. TCP-level reachability test (fast)
    tcp_ok = False
    dns_error = ""
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(4)
        tcp_ok = sock.connect_ex((host, port)) == 0
        sock.close()
    except socket.gaierror as e:
        dns_error = str(e)
    except OSError as e:
        dns_error = str(e)

    if dns_error or not tcp_ok:
        if dns_error and not is_tailscale_addr:
            print(f"    --  Cannot resolve '{host}' — hostname not found on your network")
            print()
            print(f"    Check the hostname and try again. If this is a Tailscale")
            print(f"    MagicDNS address, make sure Tailscale is running:")
            print(f"      tailscale status")
        elif is_tailscale_addr:
            print(f"    --  Cannot reach {host}:{port}")
            print()
            print(f"    This address uses Tailscale MagicDNS. Check:")
            print(f"    1. Is Tailscale running?        -> tailscale status")
            print(f"    2. On the same tailnet?         -> tailscale status")
            print(f"    3. Host shared their machine?   -> ask the host to check")
            print(f"       tailscale.com -> Machines -> (their machine) -> Share")
            print(f"    4. Is the relay running?        -> host: clavus share")
            if ts_running:
                print(f"    5. Try the Tailscale IP direct:  -> clavus join <100.x.x.x>:{port}")
        else:
            print(f"    --  Cannot reach {host}:{port} (TCP connection refused)")
            print()
            print(f"    Check:")
            print(f"    • Both machines on the same network or Tailscale?")
            print(f"    • 'clavus share' running on the host machine?")
            print(f"    • Firewall not blocking port {port}?")
            print(f"    • Correct IP? Try 'clavus find' on the same network.")
        return

    # 3b. HTTP-level: is it actually a Clavus relay?
    info: dict = {}
    client = None
    try:
        client = SyncClient(base)
        r = client.client.get(f"{base}/api/ping", timeout=8)
        if r.status_code != 200:
            print(f"    [WARN]  Port {port} is open, but it's not a Clavus relay.")
            print(f"       Make sure the host is running 'clavus share', not another service.")
            return

        projects_resp = client.client.get(f"{base}/api/projects", timeout=8)
        if projects_resp.status_code == 200:
            info = projects_resp.json()
        print(f"    [ok]  Connected — relay is online")
    except Exception as e:
        err = str(e)
        if "timed out" in err.lower() or "timeout" in err.lower():
            print(f"    --  Connection timed out")
            print(f"       Port {port} is reachable but the relay isn't responding.")
            print(f"       The host may have stopped 'clavus share' or crashed.")
        else:
            print(f"    --  Connection error: {e}")
        return
    finally:
        if client:
            try:
                client.close()
            except Exception:
                pass

    print()

    # ── Phase 4: Save Remote ─────────────────────────────────────────
    remotes = load_remotes(store)
    existing_urls = {r.url: r for r in remotes}
    remote_name = host.replace(".", "-")

    if base in existing_urls:
        existing = existing_urls[base]
        if existing.name == remote_name:
            print(f"Already connected to {remote_name}")
        else:
            # Same URL, different name — update name (relay may have rebooted)
            remotes = [r for r in remotes if r.url != base]
            remotes.append(Remote(name=remote_name, url=base))
            save_remotes(store, remotes)
            print(f"    [ok]  Updated remote name: {remote_name}")
    else:
        if remotes:
            # Already have a different remote — warn before replacing
            current = remotes[0]
            print(f"    [WARN]  You already have a remote configured: {current.name}")
            print(f"       Replacing it with {remote_name} (single remote at a time).")
            print()
            remotes = [Remote(name=remote_name, url=base)]
            save_remotes(store, remotes)
            print(f"    [ok]  Switched to: {remote_name}")
        else:
            remotes.append(Remote(name=remote_name, url=base))
            save_remotes(store, remotes)
            print(f"    [ok]  Saved remote: {remote_name}")

    # ── Phase 5: Pull Existing Projects ──────────────────────────────
    projects = info.get("projects", [])

    if not projects:
        print()
        print(f"    [info]  No projects on relay yet")
        print(f"       The host hasn't pushed any projects. You can share yours:")
        print(f"         clavus init /path/to/your-project.als")
        print(f"         clavus push")
    else:
        print()
        print(f"    📥  {len(projects)} project(s) on relay — pulling...")
        print()

        for pdata in projects:
            pname = pdata["name"]
            proj_data = store.get_index(pname)
            if not proj_data:
                proj_data = ClavusProject(
                    name=pname, root_als="", head=None,
                    created_at=time.time(),
                    description=f"Joined from {host}",
                )
                store.set_index(proj_data)

            print(f"      📥 {pname}...", end=" ", flush=True)
            result = pull_from_remote(store, proj_data, Remote(name=remote_name, url=base))
            if result.get("error"):
                print(f"-- {result['error']}")
            else:
                s = result.get("snapshots", 0)
                c = result.get("cues", 0)
                print(f"{s} snapshots, {c} cues")
                blob_count, failed = pull_snapshot_blobs(store, proj_data, Remote(name=remote_name, url=base))
                if blob_count:
                    print(f"{blob_count} audio blob(s)")
                if failed:
                    print(f"        [WARN]  {len(failed)} blob(s) failed — check disk space or network")

    # ── Phase 6: Next Steps ──────────────────────────────────────────
    print()
    print("═" * 48)
    print("[ok]  You're all set!")
    print()
    print("    Next steps:")
    print(f"      clavus tui       — Open the dashboard (recommended)")
    if not projects:
        print(f"      clavus init ...  — Share one of YOUR projects with the group")
        print(f"      clavus push      — Push it to the relay")
        print()
        print(f"    [TIP]  Quick start:  clavus init /path/to/song.als && clavus push && clavus tui")
    else:
        print()
        print(f"    [TIP]  Run 'clavus tui' — you'll see your projects, cues, and history.")
    print()


