from __future__ import annotations

"""Clavus CLI commands: relay, share."""
import argparse
from clavus.config import ClavusConfig

def cmd_relay(args: argparse.Namespace) -> None:
    """Start the Clavus relay server for collaboration.

    Provides the HTTP API and WebSocket hub that peers (TUI or CLI)
    connect to for sync, cues, snapshots, and stem transfer.
    Designed to run on a VPS, Raspberry Pi, or always-on machine.

    Other Clavus clients connect using:
      clavus remote add <name> http://<relay-ip>:7890

    The relay prints its LAN and Tailscale IPs on startup."""
    try:
        from clavus.web import run_relay_server
    except ImportError:
        print("-- Relay server requires fastapi and uvicorn.")
        print("   Install with: pip install fastapi uvicorn")
        sys.exit(1)
    cfg = ClavusConfig.load()
    host = args.host or cfg.host
    port = args.port or cfg.port

    # Always do clean-slate check — kill any stale relay before trying to bind
    pid_path = Path.home() / ".clavus" / "relay.pid"
    if pid_path.exists():
        try:
            old_pid = int(pid_path.read_text().strip())
            os.kill(old_pid, 0)
            # Process still alive — kill it
            os.kill(old_pid, 9)
            time.sleep(0.5)
        except (ProcessLookupError, ValueError, OSError):
            pass
        pid_path.unlink()

    # Check for --bg / --background flag
    if getattr(args, 'background', False) or getattr(args, 'bg', False):
        import subprocess
        pid_path = Path.home() / '.clavus' / 'relay.pid'
        pid_path.parent.mkdir(parents=True, exist_ok=True)
        
        # Check if already running
        if pid_path.exists():
            try:
                old_pid = int(pid_path.read_text().strip())
                os.kill(old_pid, 0)
                print(f'[WARN]  Relay already running (PID {old_pid}). Use --kill to stop.')
                return
            except (ProcessLookupError, ValueError, OSError):
                pid_path.unlink()
        
        if platform.system() == 'Windows':
            DETACHED = 0x00000008
            relay_args = [sys.executable, '-m', 'clavus', 'relay', '--port', str(port)]
            if args.project:
                relay_args.extend(['--project', args.project])
            proc = subprocess.Popen(
                relay_args,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                creationflags=DETACHED,
            )
            pid = proc.pid
        else:
            pid = os.fork()
            if pid == 0:
                # Child: become daemon
                os.setsid()
                devnull = os.open('/dev/null', os.O_RDWR)
                os.dup2(devnull, 0)
                os.dup2(devnull, 1)
                os.dup2(devnull, 2)
                os.close(devnull)
                relay_args = [sys.executable, '-m', 'clavus', 'relay', '--port', str(port)]
                if args.project:
                    relay_args.extend(['--project', args.project])
                os.execv(sys.executable, relay_args)
            # Parent: pid is the child
        
        pid_path.write_text(str(pid))
        time.sleep(0.5)
        # Verify it started
        try:
            from clavus.http_client import http_get
            data, err, status = http_get(f'http://127.0.0.1:{port}/api/ping', timeout=3.0)
            if status == 200:
                print(f'[ok] Relay running in background (PID {pid})')
                print(f'   Share URL: http://127.0.0.1:{port}')
                return
        except Exception:
            pass
        print(f'[WARN]  Relay started (PID {pid}) but not responding — logs: ~/.clavus/relay.log')
        return
    
    # Check for --kill flag
    if getattr(args, 'kill', False):
        pid_path = Path.home() / '.clavus' / 'relay.pid'
        if pid_path.exists():
            try:
                pid = int(pid_path.read_text().strip())
                os.kill(pid, 9)
                pid_path.unlink()
                print(f'[ok] Relay (PID {pid}) stopped.')
            except ProcessLookupError:
                pid_path.unlink()
                print('Relay was not running.')
            except Exception as e:
                print(f'Failed to stop relay: {e}')
        else:
            print('No relay PID file found.')
        return
    
    allowed_projects = [args.project] if getattr(args, 'project', None) else None
    run_relay_server(host=host, port=port, allowed_projects=allowed_projects)


def cmd_share(args: argparse.Namespace) -> None:
    """Start a relay and print the URL collaborators connect to.

    Simple: starts the relay, shows the direct URL.
    Collaborators connect with: clavus join http://IP:PORT
    """
    try:
        from clavus.web import run_relay_server
    except ImportError:
        print("-- Relay server requires fastapi and uvicorn.")
        print("   Install with: pip install fastapi uvicorn")
        sys.exit(1)

    cfg = ClavusConfig.load()
    host = args.host or cfg.host
    port = args.port or cfg.port

    # Always do clean-slate check — kill any stale relay before trying to bind
    pid_path = Path.home() / ".clavus" / "relay.pid"
    if pid_path.exists():
        try:
            old_pid = int(pid_path.read_text().strip())
            os.kill(old_pid, 0)
            os.kill(old_pid, 9)
            time.sleep(0.5)
        except (ProcessLookupError, ValueError, OSError):
            pass
        pid_path.unlink()

    # ── Tailscale serve detection (must run BEFORE socket check) ─────────────
    # Tailscale serve is a userspace proxy — it won't be detected by socket.connect_ex()
    # to the Tailscale IP (connection routes through Tailscale's own stack, not standard TCP).
    # We always check tailscale serve status for port 7890 on Mac.
    if port == 7890:
        import subprocess as sp
        try:
            ts_check = sp.run(
                ["tailscale", "serve", "status"], capture_output=True, text=True, timeout=5
            )
            ts_output = ts_check.stdout
            if ts_check.returncode == 0:
                import re
                # Match "http://...:7890 (tailnet only)" and "localhost:7891" in output
                port_match = re.search(rf"https?://[^:]+:{port}\s*\(tailnet only\)", ts_output)
                proxy_match = re.search(rf"proxy\s+http://localhost:(\d+)", ts_output)
                if port_match and proxy_match:
                    ts_proxy_port = int(proxy_match.group(1))
                    print(f"   [info]  Port {port} is proxied by tailscale serve -> localhost:{ts_proxy_port}")
                    port = 7891
                    print(f"Switched to port 7891 (tailscale serve handles external traffic on 7890)")
        except Exception:
            pass
    # ── End Tailscale serve detection ─────────────────────────────────────────

    # Check if port is in use — try to kill stale Clavus relay
    import socket, platform, signal, subprocess as sp, time

    # Get Tailscale IP to check if Tailscale serve owns the port
    ts_ip = None
    try:
        ts_result = sp.run(
            ["tailscale", "status", "--self", "--json"], capture_output=True, text=True, timeout=5
        )
        import json as json_mod
        ts_data = json_mod.loads(ts_result.stdout)
        ts_ips = ts_data.get("Self", {}).get("TailNetIPs", [])
        if ts_ips:
            ts_ip = ts_ips[0]
    except Exception:
        pass

    # Check 127.0.0.1 AND Tailscale IP — Tailscale serve binds to Tailscale IP, not localhost
    port_open = False
    for check_host in ["127.0.0.1"] + ([ts_ip] if ts_ip else []):
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        if sock.connect_ex((check_host, port)) == 0:
            sock.close()
            port_open = True
            break

    if port_open:
        killed = False
        try:
            from clavus.http_client import http_get
            data, err, status = http_get(f"http://127.0.0.1:{port}/api/sync/pull", params={"name": "_"}, timeout=2.0)
            if status in (200, 404):
                print(f"Restarting stale Clavus relay on port {port}...")
                if platform.system() == "Windows":
                    # Find PID using port and kill it
                    r2 = sp.run(["netstat", "-ano"], capture_output=True, text=True)
                    for line in r2.stdout.split("\n"):
                        if f":{port}" in line and "LISTENING" in line:
                            pid = line.strip().split()[-1]
                            sp.run(["taskkill", "/f", "/pid", pid], capture_output=True)
                            killed = True
                            break
                else:
                    result = sp.run(["lsof", "-ti", f":{port}"], capture_output=True, text=True)
                    for pid in result.stdout.strip().split("\n"):
                        if pid:
                            try:
                                os.kill(int(pid), signal.SIGTERM)
                                killed = True
                            except Exception:
                                pass
                if killed:
                    time.sleep(1)
        except Exception:
            pass
        
        # Re-check with retry — port may not release instantly on Windows
        for attempt in range(6):
            sock2 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            if sock2.connect_ex(("127.0.0.1", port)) != 0:
                sock2.close()
                break  # Port free
            sock2.close()
            if attempt < 5:
                time.sleep(0.5)
        else:
            print(f"   -- Port {port} still in use. Stop the other relay first:")
            print(f"      clavus share --kill" if platform.system() != "Windows" else f"      taskkill /f /im python.exe")
            sys.exit(1)
    else:
        sock.close()

    # Show the URL(s) collaborators should use
    # Detect Tailscale MagicDNS hostname (works cross-account when node is shared)
    import subprocess, json
    ts_url = ""
    try:
        r = subprocess.run(
            ["tailscale", "status", "--json"],
            capture_output=True, text=True, timeout=5,
        )
        if r.returncode == 0:
            dns = json.loads(r.stdout).get("Self", {}).get("DNSName", "")
            if dns:
                ts_host = dns.rstrip(".")
                ts_url = f"http://{ts_host}:{port}"
    except Exception:
        pass

    # Fallback: raw Tailscale IP (same-tailnet only)
    if not ts_url:
        try:
            result = subprocess.run(
                ["tailscale", "ip", "-4"],
                capture_output=True, text=True, timeout=5,
            )
            ts_ip = result.stdout.strip()
            if ts_ip:
                ts_url = f"http://{ts_ip}:{port}"
        except Exception:
            pass

    lan_url = f"http://{socket.gethostbyname(socket.gethostname())}:{port}"

    # Auto-setup tailscale serve if missing — this is the #1 failure point for cross-account sharing
    try:
        r = subprocess.run(
            ["tailscale", "serve", "status"],
            capture_output=True, text=True, timeout=5,
        )
        serve_ok = r.returncode == 0 and "No serve config" not in r.stdout
    except Exception:
        serve_ok = False

    if not serve_ok:
        try:
            subprocess.run(
                ["tailscale", "serve", "--bg", "--http", str(port), f"http://localhost:{port}"],
                capture_output=True, text=True, timeout=10,
            )
            print(f"  [ok] MagicDNS relay configured -- collaborators can use the hostname URL")
        except Exception:
            print(f"  [WARN] Could not configure MagicDNS relay -- collaborators may not reach you via hostname")

    print(f"  >> CLAVUS SHARE")
    print(f"  {'─' * 45}")
    if ts_url:
        print()
        print(f"  Collaborators connect:")
        print(f"    clavus join {ts_url}")
    print(f"  LAN:")
    print(f"    clavus join {lan_url}")
    print()
    print(f"  Press Ctrl+C to stop.")
    print(f"  {'─' * 45}")
    print()

    # ── Auto-configure localhost remote so you can pull your own relay ──
    from clavus.store import BlobStore
    from clavus.sync import load_remotes, save_remotes, Remote as RemoteConfig
    store = BlobStore()
    remotes = load_remotes(store)
    localhost_url = f"http://localhost:{port}"
    if not any(r.url.rstrip("/") == localhost_url for r in remotes):
        remotes.append(RemoteConfig(name="localhost", url=localhost_url))
        save_remotes(store, remotes)

    if args.project:
        print(f"  scoped to project: {args.project}")
        print()

    # Check for --bg / --background flag
    if getattr(args, 'background', False) or getattr(args, 'bg', False):
        import subprocess
        pid_path = Path.home() / '.clavus' / 'relay.pid'
        pid_path.parent.mkdir(parents=True, exist_ok=True)
        
        # Check if already running
        if pid_path.exists():
            try:
                old_pid = int(pid_path.read_text().strip())
                os.kill(old_pid, 0)
                print(f'[WARN]  Relay already running (PID {old_pid}). Use --kill to stop.')
                return
            except (ProcessLookupError, ValueError, OSError):
                pid_path.unlink()
        
        if platform.system() == 'Windows':
            DETACHED = 0x00000008
            relay_args = [sys.executable, '-m', 'clavus', 'relay', '--port', str(port)]
            if args.project:
                relay_args.extend(['--project', args.project])
            proc = subprocess.Popen(
                relay_args,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                creationflags=DETACHED,
            )
            pid = proc.pid
        else:
            pid = os.fork()
            if pid == 0:
                # Child: become daemon
                os.setsid()
                devnull = os.open('/dev/null', os.O_RDWR)
                os.dup2(devnull, 0)
                os.dup2(devnull, 1)
                os.dup2(devnull, 2)
                os.close(devnull)
                relay_args = [sys.executable, '-m', 'clavus', 'relay', '--port', str(port)]
                if args.project:
                    relay_args.extend(['--project', args.project])
                os.execv(sys.executable, relay_args)
            # Parent: pid is the child
        
        pid_path.write_text(str(pid))
        time.sleep(0.5)
        # Verify it started
        try:
            from clavus.http_client import http_get
            data, err, status = http_get(f'http://127.0.0.1:{port}/api/ping', timeout=3.0)
            if status == 200:
                print(f'[ok] Relay running in background (PID {pid})')
                print(f'   Share URL: http://127.0.0.1:{port}')
                return
        except Exception:
            pass
        print(f'[WARN]  Relay started (PID {pid}) but not responding — logs: ~/.clavus/relay.log')
        return
    
    # Check for --kill flag
    if getattr(args, 'kill', False):
        pid_path = Path.home() / '.clavus' / 'relay.pid'
        if pid_path.exists():
            try:
                pid = int(pid_path.read_text().strip())
                os.kill(pid, 9)
                pid_path.unlink()
                print(f'[ok] Relay (PID {pid}) stopped.')
            except ProcessLookupError:
                pid_path.unlink()
                print('Relay was not running.')
            except Exception as e:
                print(f'Failed to stop relay: {e}')
        else:
            print('No relay PID file found.')
        return
    
    allowed_projects = [args.project] if getattr(args, 'project', None) else None
    run_relay_server(host=host, port=port, allowed_projects=allowed_projects)


