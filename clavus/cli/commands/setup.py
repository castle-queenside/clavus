from __future__ import annotations

"""Clavus CLI command: setup — Guided first-run setup."""
import argparse

def cmd_setup(args: argparse.Namespace) -> None:
    """Guided first-run setup — author, role, port, Tailscale, Ableton detection."""
    import json, platform, socket, subprocess
    from clavus.config import ClavusConfig
    from clavus.store import BlobStore

    print("Clavus Setup")
    print("--")
    print()

    cfg = ClavusConfig.load()

    # 1. Author name
    current = cfg.author or os.environ.get("USER") or os.environ.get("USERNAME") or ""
    print("Author name [{}]: ".format(current), end="")
    try:
        author = input().strip()
    except (EOFError, KeyboardInterrupt):
        author = ""
    cfg.set("author", author or current)

    # 2. Role — host, join, or LAN-only
    print()
    print("How will you use Clavus?")
    print("   [1] Host a session     -- start a relay, share your URL with collaborators")
    print("   [2] Join a session     -- you've received a relay URL from the host")
    print("   [3] LAN only           -- no internet, same network only")
    print()
    role = ""
    try:
        role_sel = input("   Select [1]: ").strip()
    except (EOFError, KeyboardInterrupt):
        role_sel = ""
    if role_sel == "2":
        role = "join"
    elif role_sel == "3":
        role = "lan"
    else:
        role = "host"

    # 3. Port
    port = cfg.port or 7890
    print(f"🔌 Relay port [{port}]: ", end="")
    try:
        pi = input().strip()
        if pi:
            port = int(pi)
    except (ValueError, EOFError, KeyboardInterrupt):
        pass
    cfg.set("port", port)

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    in_use = sock.connect_ex(("127.0.0.1", port)) == 0
    sock.close()
    if in_use:
        print(f"   [WARN]  Port {port} in use — choose another: clavus config set port <N>")

    # 4. Tailscale check
    print()
    print(f"Network discovery...")
    ts_ip = ""
    ts_magic_dns = ""
    ts_running = False
    try:
        r = subprocess.run(["tailscale", "status", "--json"], capture_output=True, text=True, timeout=10)
        if r.returncode == 0:
            import json
            status = json.loads(r.stdout)
            self_info = status.get("Self", {})
            ts_magic_dns = self_info.get("DNSName", "").rstrip(".")
            ts_ips = self_info.get("TailNetIPs", [])
            ts_ip = ts_ips[0] if ts_ips else ""
            ts_running = True
            if ts_magic_dns:
                print(f"   [ok] Tailscale found")
                print(f"MagicDNS: {ts_magic_dns}")
                if ts_ip:
                    print(f"Tailscale IP: {ts_ip}")
                cfg.set("tailscale_ip", ts_ip)
                cfg.set("tailscale_magic_dns", ts_magic_dns)
            else:
                print("   [ok] Tailscale running (MagicDNS not configured)")
        else:
            ts_running = False
    except (FileNotFoundError, subprocess.TimeoutExpired, json.JSONDecodeError):
        ts_running = False

    if not ts_running and role == "host":
        print()
        print("   [WARN]  Tailscale not found — needed to share your relay URL remotely.")
        print("   Install: https://tailscale.com/download")
        print("   After install, run: clavus setup")
        print()
        print("   For now you can use LAN-only mode (option [3] above).")
        role = "lan"
    elif not ts_running:
        print("   [info]  Tailscale not found — LAN-only collab")

    # Initialize store (needed for remotes)
    store = BlobStore()
    store.init()

    # 5. Collaborator URL (only for join role)
    print()
    from clavus.sync import load_remotes, save_remotes, Remote
    remotes = load_remotes(store)
    existing_urls = {r.url for r in remotes}
    collab_url = ""

    if role == "join":
        print(f"Enter the relay URL from your session host.")
        print("    They ran 'clavus share' and saw a URL like:")
        print("      http://machine-name.tailXXXX.ts.net:7890")
        print()
        try:
            collab_url = input("   URL: ").strip()
        except (EOFError, KeyboardInterrupt):
            collab_url = ""
        if collab_url:
            from urllib.parse import urlparse
            if not (collab_url.startswith("http://") or collab_url.startswith("https://")):
                if ":" in collab_url:
                    collab_url = f"http://{collab_url}"
                else:
                    collab_url = f"http://{collab_url}:7890"
            parsed = urlparse(collab_url)
            host = parsed.hostname or collab_url
            url = f"http://{host}:{parsed.port or port}"
            if url not in existing_urls:
                name = host.replace(".", "-")
                remotes.append(Remote(name=name, url=url))
                save_remotes(store, remotes)
                print(f"   [ok] Added remote '{name}' -> {url}")
                existing_urls.add(url)
        print()
        print("   [TIP] Run 'clavus tui' to open the UI, then press p to pull projects")
    elif role == "host":
        print()
        print(f"When you run 'clavus share', collaborators will connect to:")
        if ts_magic_dns:
            print(f"      http://{ts_magic_dns}:{port}")
        if ts_ip:
            print(f"      http://{ts_ip}:{port}")
        print(f"      http://<your-lan-ip>:{port}")
        print()
        print("   Share this URL with your collaborators after running 'clavus share'.")
    elif role == "lan":
        print()
        print(f"LAN-only mode — collaborators must be on the same network.")
        print("   They'll use your local IP address (e.g. 192.168.x.x)")

    # Also add localhost relay if port is in use (already running)
    if not collab_url and in_use:
        print(f"Relay detected on port {port} — good!")
        relay_url = f"http://localhost:{port}"
        if relay_url not in existing_urls:
            remotes.append(Remote(name="relay", url=relay_url))
            save_remotes(store, remotes)
            print(f"   [ok] Added remote 'relay' -> {relay_url}")
    elif collab_url:
        # Remove any stale localhost remotes — they won't work on this machine
        before = len(remotes)
        remotes = [r for r in remotes if "localhost" not in r.url and "127.0.0.1" not in r.url]
        if len(remotes) < before:
            save_remotes(store, remotes)
            print(f"   [sweep] Removed {before - len(remotes)} stale localhost remote(s)")

    # Summary
    remotes = load_remotes(store)
    if remotes:
        print(f"{len(remotes)} remote(s) configured")
    else:
        print("   [info]  No remotes — use 'clavus remote add' later or 'clavus join http://...'")

    # 6. Projects folder
    print()
    from clavus.config import DEFAULT_PROJECTS_DIR
    current_dir = cfg.projects_dir or DEFAULT_PROJECTS_DIR
    print(f"Projects folder [{current_dir}]: ", end="")
    try:
        pd = input().strip()
        if pd:
            cfg.set("projects_dir", pd)
            Path(pd).mkdir(parents=True, exist_ok=True)
    except (EOFError, KeyboardInterrupt):
        pass
    print(f"   All synced projects go here")

    # 7. Ableton detection
    print()
    ableton = None
    if platform.system() == "Darwin":
        for name in ["Ableton Live 12 Suite", "Ableton Live 12 Intro", "Ableton Live 12 Standard",
                      "Ableton Live 11 Suite", "Ableton Live 11 Intro"]:
            path = f"/Applications/{name}.app"
            if os.path.exists(path):
                ableton = path
                break
    elif platform.system() == "Windows":
        try:
            import winreg
            for ver in [12, 11]:
                try:
                    key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, rf"SOFTWARE\Ableton\Live {ver}")
                    ableton = winreg.QueryValueEx(key, "InstallDir")[0]
                    break
                except Exception:
                    pass
        except Exception:
            pass

    if ableton:
        print(f"   [ok] Ableton: {ableton}")
        cfg.set("ableton_path", ableton)
    else:
        print("   [info]  Ableton not auto-detected")
        print("      Set: clavus config set ableton_path /path/to/Ableton")

    # Save
    cfg.save()

    print()
    print("[ok] Setup complete!")
    print(f"   Config: {CONFIG_PATH}")
    print(f"   Data:   {store.root}")

    # Test connection to configured remotes
    remotes = load_remotes(store)
    if remotes:
        print()
        for remote in remotes:
            if "localhost" in remote.url:
                continue  # Skip localhost — only test external remotes
            try:
                from clavus.sync import SyncClient
                client = SyncClient(remote.url)
                r = client.client.get(f"{remote.url}/api/projects", timeout=5)
                client.close()
                if r.status_code == 200:
                    projects = r.json().get("projects", [])
                    print(f"   [ok] {remote.name} — {len(projects)} project(s) found")
                else:
                    print(f"   [WARN]  {remote.name} — relay responded but no projects (run 'clavus init' on host)")
            except Exception:
                print(f"   [WARN]  {remote.name} — unreachable (is 'clavus share' running?)")

    # Smart next-step based on role
    print()
    print("   Next steps:")
    if role == "host":
        print("     clavus share            <- start relay and get your share URL")
        print("     clavus init /path/to.als  <- track your first project")
    elif role == "join":
        print("     clavus pull             <- pull projects from the host")
        print("     clavus tui              <- open the dashboard (press p to pull)")
    else:
        print("     clavus init /path/to.als  <- track a local project")
        print("     clavus tui              <- open the dashboard")
    print()
    print("   [TIP] Quick start (host):  clavus share")
    print("   [TIP] Quick start (join):   clavus pull && clavus tui")


