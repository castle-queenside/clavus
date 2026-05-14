"""
clavus CLI — the primary user interface.

Commands:
  clavus init [path]          Initialize + git init
  clavus projects             List all tracked projects
  clavus project <name>       Switch active project
  clavus snapshot "message"   Snapshot + git commit .als
  clavus log                  Show clavus + git history
  clavus log --graph          Show branch topology
  clavus branch               List / create branches (clavus + git)
  clavus checkout <name>      Switch branches (clavus + git)
  clavus merge <branch>       Merge branches (clavus + git)
  clavus diff [hash]          Show what changed in a snapshot
  clavus status               Show current project state
  clavus cue "text" @time     Add a timeline-anchored comment
  clavus cues                 List all pending cues
  clavus remote               Manage remotes
  clavus push                 Push to remotes + git push
  clavus pull                 Pull from remotes + git pull
  clavus sync                 Start auto-sync daemon
"""

from __future__ import annotations

import argparse
import getpass
import hashlib
import json
import os
import sys
import time
from pathlib import Path
from typing import Optional

from clavus.parser import parse_als, project_summary
from clavus.config import ClavusConfig, CONFIG_PATH
from clavus.store import (
    BlobStore, ClavusProject, diff_projects, format_diff,
    DEFAULT_CLAVUS_DIR,
)
from clavus.cues import (
    CueStore, CueFilter, format_cue, format_cue_list,
    render_cues_as_markers, add_cue_command,
)
from clavus.helpers import find_als_file, get_store_and_project, resolve_snapshot, get_desktop_path, get_projects_dir
from clavus.watch import watch
from clavus.store import (
    StemStore, StemEntry, StemManifest,
)
from clavus.sync import (
    load_remotes, save_remotes, Remote, push_to_remote, pull_from_remote, SyncDaemon,
)


# ─── Commands ──────────────────────────────────────────────────────────

def cmd_repair(args: argparse.Namespace) -> None:
    """Repair Clavus storage — recover from backup, cues, refs, or partial data."""
    from clavus.store import BlobStore, ClavusProject
    from dataclasses import asdict
    import json, time, shutil
    from pathlib import Path

    store = BlobStore()
    index_path = store.index_path
    bak_paths = [
        index_path.with_suffix(index_path.suffix + s)
        for s in [".bak", ".bak2", ".bak3"]
    ]

    print("🔧 Clavus Repair")
    print("───")
    print()

    # Phase 1: Check current index health
    if index_path.exists() and not args.force:
        try:
            data = json.loads(index_path.read_text())
            valid = sum(1 for v in data.values() if isinstance(v, dict) and "root_als" in v)
            print(f"✅ index.json exists with {valid} project(s).")
            print("   Use --force to re-scan from scratch.")
            return
        except json.JSONDecodeError:
            print(f"⚠️  index.json is corrupt (will rebuild).")
    elif args.force:
        print("🔨 Force mode: re-scanning from scratch.")

    print("🔍 Scanning for recoverable data...")
    recovered = {}

    # Scan refs/ for HEAD
    head_hash = store.read_ref("HEAD")
    if head_hash:
        print(f"   📍 HEAD ref: {head_hash[:16]}...")
    else:
        print("   📍 No HEAD ref found.")

    # Scan backups
    for bak in bak_paths:
        if bak.exists():
            try:
                data = json.loads(bak.read_text())
                valid = {k: v for k, v in data.items()
                         if isinstance(v, dict) and "root_als" in v}
                if valid:
                    print(f"   💾 Found backup: {bak.name} ({len(valid)} project(s))")
                    recovered.update(valid)
            except (json.JSONDecodeError, OSError):
                continue

    # Scan cues/ directories for project names
    cues_root = Path(store.root) / "cues"
    if cues_root.exists():
        for proj_dir in sorted(cues_root.iterdir()):
            if proj_dir.is_dir() and not proj_dir.name.startswith("."):
                cue_count = len(list(proj_dir.glob("*.json")))
                if cue_count > 0 and proj_dir.name not in recovered:
                    print(f"   📋 Found cues for '{proj_dir.name}' ({cue_count} cue(s))")
                    proj = ClavusProject(
                        name=proj_dir.name,
                        root_als="",
                        created_at=time.time(),
                        head=head_hash,
                        description="(recovered — set .als path with --set-als)",
                    )
                    recovered[proj.name] = asdict(proj)
                elif cue_count > 0:
                    print(f"   📋 Cues for '{proj_dir.name}' ({cue_count} cue(s)) — already in backup")
                else:
                    print(f"   📋 Empty cues dir for '{proj_dir.name}' — skipping")

    if not recovered:
        print("❌ Nothing to recover. No backups, cues, or refs found.")
        return

    # Phase 2: apply --set-als mappings
    if args.set_als:
        if args.set_als.startswith("all="):
            all_path = args.set_als[4:]
            for proj_name in recovered:
                recovered[proj_name]["root_als"] = all_path
            print(f"   🎯 Set .als path for ALL projects: {all_path}")
        elif "=" in args.set_als:
            name, path = args.set_als.split("=", 1)
            if name in recovered:
                recovered[name]["root_als"] = path
                print(f"   🎯 Set .als path for '{name}': {path}")
            else:
                print(f"   ⚠️  Project '{name}' not found in recovered data.")

    # Phase 3: find .als files matching project names
    for proj_name, proj_data in recovered.items():
        if proj_data.get("root_als"):
            continue  # already has a path
        # Search typical locations
        from pathlib import Path as P
        candidates = list(P.home().glob(f"Desktop/{proj_name}*.als")) \
                   + list(P.home().glob(f"Desktop/**/{proj_name}*.als")) \
                   + list(P.home().glob(f"Documents/**/{proj_name}*.als"))
        if candidates:
            best = str(candidates[0])
            recovered[proj_name]["root_als"] = best
            print(f"   🔍 Auto-detected .als for '{proj_name}': {best}")

    # Phase 4: write recovered index
    recovered["_last_project"] = store.read_ref("_last_project") or list(recovered.keys())[0]
    store._backup_index()
    store._write_json(index_path, recovered)
    print()
    print(f"✅ Recovered {len([k for k in recovered if isinstance(recovered[k], dict)])} project(s) to {index_path}")
    print()
    for name, data in recovered.items():
        if not isinstance(data, dict):
            continue
        als = data.get("root_als", "") or "(no path)"
        head = data.get("head", "")[:12] if data.get("head") else "(no snapshots)"
        print(f"   {name:<25} {als}")
    print()
    if any(not data.get("root_als") for data in recovered.values() if isinstance(data, dict)):
        print("💡 Tip: Set .als paths with: clavus repair --set-als 'ProjectName=/path/to/project.als'")
        print("   Or for all at once:    clavus repair --set-als 'all=/path/to/ProjectDir/'")
    else:
        print("✅ All projects have .als paths. Run 'clavus projects' to verify.")


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

    def check(label: str, cond: bool, detail: str = "") -> None:
        sym = "✅" if cond else "❌"
        msg = f"  {sym} {label}"
        if detail:
            msg += f" — {detail}"
        if cond:
            ok.append(msg)
        else:
            fail.append(msg)

    def warn_check(label: str, detail: str = "") -> None:
        msg = f"  ⚠️  {label}"
        if detail:
            msg += f" — {detail}"
        warn.append(msg)

    print(f"🔍 Clavus Doctor")
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
                    sym = "✅" if als_ok else "⚠️"
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
            # Try /api/ping endpoint (defined on all clavus relays)
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
        check(f"remote '{remote.name}' ({host})", ping_ok, "reachable" if ping_ok else "unreachable")

    # 7. Tailscale MagicDNS — resolve each remote's hostname
    print("  network:")
    for remote in remotes:
        try:
            parsed = urllib.parse.urlparse(remote.url)
            if parsed.netloc:
                # Use parsed.hostname (without port) for DNS resolution
                host = parsed.hostname or parsed.netloc.split(":")[0]
                addr = socket.gethostbyname(host)
                ok.append(f"  ✅ '{host}' — {addr}")
        except socket.gaierror:
            try:
                parsed = urllib.parse.urlparse(remote.url)
                host = parsed.hostname or parsed.netloc.split(":")[0]
                warn.append(f"  ⚠️  '{host}' — could not resolve")
            except Exception:
                warn.append(f"  ⚠️  remote '{remote.name}' — could not parse URL")

    # 8. Tailscale serve proxy — the relay's tailnet gateway
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
            fail.append(f"  ❌ tailscale serve — not configured (relay unreachable via MagicDNS)")
            warn.append(f"  💡 Fix: tailscale serve --bg --http {port} http://localhost:{port}")
    except FileNotFoundError:
        warn_check("tailscale", "not installed — LAN-only collab")
    except Exception as e:
        warn_check("tailscale serve", str(e))

    # 9. Relay process + end-to-end MagicDNS reachability
    print("  relay:")
    relay_alive = False
    relay_port = port
    # Try 7890 first (where tailscale serve proxies from), then fall back to raw relay port
    # Try GET (HEAD not supported on all endpoints)
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
        fail.append(f"  ❌ relay process — not running on port {port} (or 7891)")
        fail.append(f"  💡 Fix: run 'clavus share' to start it")

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
            warn.append(f"  💡 Verify from another tailnet machine: curl http://{ts_host or '?'}:{port}/api/ping")

    # Summary
    print()
    print(f"  Summary: {len(ok)} ✅  {len(warn)} ⚠️  {len(fail)} ❌")
    for line in fail:
        print(line)
    for line in warn:
        print(line)
    print()
    if fail:
        print(f"  💡 Run 'clavus repair' to recover from corruption")
        print(f"  💡 Run 'clavus restore-store' to restore from backup")
    elif warn:
        print(f"  💡 Run 'clavus backup' to create a full backup")
    else:
        print(f"  💡 Everything looks good!")


def cmd_p2p(args: argparse.Namespace) -> None:
    """Discover and sync with peers on the tailnet via direct TCP connection.

    Usage:
      clavus p2p                  # discover online peers
      clavus p2p --host           # start listening for incoming connections
      clavus p2p --connect <dns>  # connect to a peer by DNS name

    Conflict detection (git-style):
      Both sides track HEAD. On connect, the connector sends expected_head
      (what they think the listener has). The listener rejects if it doesn't
      match — CONFLICT frame — preventing silent overwrites.
      After successful sync, both update last_peer_head for the next session.
    """
    from clavus.store import BlobStore
    from clavus.p2p_transport import TCPTransport, p2p_sync, discover_peers
    from clavus.config import ClavusConfig
    import time

    store = BlobStore()
    cfg = ClavusConfig.load()

    # ── Resolve tailscale DNS for this machine ────────────────────────────
    import subprocess, json
    ts_dns = ""
    try:
        r = subprocess.run(
            ["tailscale", "status", "--json"],
            capture_output=True, text=True, timeout=5,
        )
        if r.returncode == 0:
            ts_dns = json.loads(r.stdout).get("Self", {}).get("DNSName", "").rstrip(".")
    except Exception:
        pass

    # ── Discover peers ────────────────────────────────────────────────────
    if not args.host and not args.connect:
        print("  Discovering peers on tailnet...")
        peers = discover_peers()
        if not peers:
            print("  No peers found — make sure Tailscale is running")
            return

        online = [p for p in peers if p.get("online", False)]
        offline = [p for p in peers if not p.get("online", False)]

        print(f"\n  Online ({len(online)}):")
        if not online:
            print("    (none)")
        for p in online:
            dns = p.get("dns", "")
            ip = p.get("ip", "")
            print(f"    {p['name']}")
            if dns:
                print(f"      DNS: {dns}")
            if ip:
                print(f"      IP:  {ip}")

        if offline:
            print(f"\n  Offline ({len(offline)}):")
            for p in offline:
                print(f"    {p['name']}")

        print(f"\n  Usage:")
        print(f"    clavus p2p --host              # start listening")
        if ts_dns:
            print(f"    clavus p2p --connect {ts_dns}  # connect to this machine")
        else:
            print(f"    clavus p2p --connect <peer-dns>  # connect to a peer")
        return

    # ── Collect local manifests ────────────────────────────────────────────
    last_proj_name = cfg.default_project or store.read_ref("_last_project") or ""
    active_project = store.get_index(last_proj_name)
    if not active_project:
        print("  No active project. Run 'clavus project <name>' first.")
        return

    proj = active_project
    head = proj.head or ""
    snapshots = [head] if head else []
    blobs: list[str] = []

    # Walk snapshot history to collect all blob references
    current = head
    seen: set[str] = set()
    while current:
        if current in seen:
            break
        seen.add(current)
        snap = store.load_snapshot(current)
        if not snap:
            break
        if snap.content_hash:
            blobs.append(snap.content_hash)
        if snap.als_hash:
            blobs.append(snap.als_hash)
        if snap.sample_hashes:
            blobs.extend(snap.sample_hashes)
        if snap.parent == current:
            break
        current = snap.parent

    print(f"  Project: {proj.name}")
    print(f"  Snapshots: {len(snapshots)}, Blobs: {len(blobs)}")
    if head:
        print(f"  HEAD: {head[:12]}")

    # ── Host mode ─────────────────────────────────────────────────────────
    if args.host:
        port = 7892
        print(f"\n  Listening on port {port}...")
        print(f"  Share this with your collaborator:")
        print(f"    clavus p2p --connect {ts_dns or '<your-dns>'}")
        print()

        # last_peer_head per peer — loaded from refs/peers/<dns>/head
        def _load_peer_head(peer_dns: str) -> Optional[str]:
            return store.read_ref(f"refs/peers/{peer_dns}/head")

        transport = TCPTransport(proj.name, snapshots, blobs, head=head)
        results: dict = {}
        done = False

        def handler(project: str, sock, peer_manifest):
            nonlocal done
            # peer_manifest has what the connector sent — including their head
            peer_head = getattr(peer_manifest, "head", None) or ""
            print(f"\n  Peer connected: {project}")
            print(f"  Peer HEAD: {peer_head[:12] if peer_head else '(none)'}")
            if peer_head and head:
                if peer_head != head:
                    print(f"  ⚠ Heads differ — syncing will detect conflicts")
            r = p2p_sync(
                sock=sock,
                store=store,
                local_snapshots=snapshots,
                local_blobs=blobs,
                local_has=store.has_object,
                peer_snapshots=getattr(peer_manifest, "snapshots", []),
                peer_blobs=getattr(peer_manifest, "blobs", []),
                peer_has=lambda h: h in set(getattr(peer_manifest, "blobs", [])),
            )
            results["sync"] = r
            # Update last_peer_head for this peer
            if peer_head:
                store.update_ref(f"peers/{args.connect or ts_dns}/head", peer_head)
            done = True
            print(f"\n  Sync result: {r}")

        transport.listen_with_peer_manifest(port, handler)

        # Wait for one connection or Ctrl+C
        try:
            while not done:
                time.sleep(1)
        except KeyboardInterrupt:
            print("\n  Stopping...")
        finally:
            transport.close()
        return

    # ── Connect mode ─────────────────────────────────────────────────────
    if args.connect:
        peer_dns = args.connect
        port = 7892
        last_peer_head = store.read_ref(f"peers/{peer_dns}/head")
        print(f"\n  Connecting to {peer_dns}:{port}...")
        if last_peer_head:
            print(f"  Peer HEAD (expected): {last_peer_head[:12]}")

        transport = TCPTransport(
            proj.name, snapshots, blobs,
            head=head,
            last_peer_head=last_peer_head,
        )
        peer_manifest, sock = transport.connect(peer_dns, port)

        if not peer_manifest:
            print(f"  Failed to connect to {peer_dns}")
            transport.close()
            return

        # ── Conflict detected ──────────────────────────────────────────────
        if peer_manifest.project.startswith("CONFLICT:"):
            print(f"\n  ⚠ Sync Conflict")
            print(f"  {peer_manifest.project}")
            print(f"\n  The peer's state has diverged from last sync.")
            print(f"  Sync both machines to the relay first, then try again.")
            transport.close()
            return

        if not sock:
            print(f"  Connection failed")
            transport.close()
            return

        print(f"  Connected. Peer project: {peer_manifest.project}")
        print(f"  Peer snapshots: {len(peer_manifest.snapshots)}, blobs: {len(peer_manifest.blobs)}")
        if peer_manifest.head:
            print(f"  Peer HEAD: {peer_manifest.head[:12]}")
            if head and peer_manifest.head != head:
                print(f"  ⚠ Heads differ — sync will detect conflicts")

        peer_blobs = getattr(peer_manifest, "blobs", [])
        peer_has = lambda h: h in set(peer_blobs)

        r = p2p_sync(
            sock=sock,
            store=store,
            local_snapshots=snapshots,
            local_blobs=blobs,
            local_has=store.has_object,
            peer_snapshots=getattr(peer_manifest, "snapshots", []),
            peer_blobs=peer_blobs,
            peer_has=peer_has,
        )

        # Update last_peer_head on successful sync
        peer_head = getattr(peer_manifest, "head", None)
        if peer_head:
            store.update_ref(f"peers/{peer_dns}/head", peer_head)
            print(f"\n  Updated peer HEAD: {peer_head[:12]}")

        print(f"\n  Sync result: {r}")
        sock.close()
        transport.close()
        return


def cmd_help(args: argparse.Namespace) -> None:
    """Show all available commands including hidden ones."""
    print("clavus — all commands")
    print("─" * 43)
    print()
    print(" Essentials:")
    print("   init              Initialize a new project")
    print("   projects          List all tracked projects")
    print("   project <name>    Switch active project")
    print("   snapshot          Create a snapshot")
    print("   log               Show snapshot history")
    print("   restore           Restore .als from a snapshot")
    print("   tui               Launch the dashboard (recommended)")
    print()
    print(" Daily workflow:")
    print("   status            Current project status")
    print("   cue <text>        Leave a note on the timeline")
    print("   cues              List all cues")
    print("   cue-reply <id>    Reply to a cue thread")
    print("   cue-resolve <id>  Mark a cue done")
    print("   cue-skip <id>     Skip a cue")
    print("   cue-archive       Archive resolved cues")
    print()
    print(" Collaboration:")
    print("   share             Start a share session (one command)")
    print("   join <url>        Connect to a collaborator's session")
    print("   push              Push to remotes")
    print("   pull              Pull from remotes")
    print("   remote            Manage remotes")
    print()
    print(" Background:")
    print("   watch             Auto-snapshot on file changes")
    print()
    print(" Safety:")
    print("   backup            Backup entire store")
    print("   backups           List backups")
    print("   restore-store     Restore store from backup")
    print()
    print(" Utilities:")
    print("   open              Open latest .als in Ableton Live")
    print("   config            View or edit config")
    print("   doctor            Diagnose store health")
    print("   setup             First-run wizard")
    print("   repair            Fix damaged store")
    print()
    print(" Advanced (clavus help):")
    print("   diff              Show snapshot changes")
    print("   cue-render        Export cues as Ableton markers")
    print("   cue-assign        Assign a cue to someone")
    print("   cue-unassign      Remove assignee")
    print("   cue-start         Mark cue in-progress")
    print("   cue-stop          Mark cue not in-progress")
    print("   cue-delete        Delete a cue permanently")
    print("   branch            List/create branches")
    print("   checkout          Switch branches")
    print("   merge             Merge branches")
    print()


def cmd_setup(args: argparse.Namespace) -> None:
    """Guided first-run setup — author, port, Tailscale, Ableton detection."""
    import platform, socket, subprocess
    from clavus.config import ClavusConfig
    from clavus.store import BlobStore

    print("🎹 Clavus Setup")
    print("───")
    print()

    cfg = ClavusConfig.load()

    # 1. Author name
    current = cfg.author or os.environ.get("USER") or os.environ.get("USERNAME") or ""
    print(f"👤 Author name [{current}]: ", end="")
    try:
        author = input().strip()
    except (EOFError, KeyboardInterrupt):
        author = ""
    cfg.set("author", author or current)

    # 2. Port
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
        print(f"   ⚠️  Port {port} in use — choose another: clavus config set port <N>")

    # 3. Tailscale check
    print()
    print("🌐 Network discovery...")
    ts_ip = ""
    try:
        r = subprocess.run(["tailscale", "version"], capture_output=True, text=True, timeout=5)
        if r.returncode == 0:
            print("   ✅ Tailscale found")
            r2 = subprocess.run(["tailscale", "ip", "-4"], capture_output=True, text=True, timeout=5)
            ts_ip = r2.stdout.strip()
            if ts_ip:
                print(f"   📡 Tailscale IP: {ts_ip}")
                cfg.set("tailscale_ip", ts_ip)
        else:
            print("   ℹ️  Tailscale not found — LAN-only collab")
    except (FileNotFoundError, subprocess.TimeoutExpired):
        print("   ℹ️  Tailscale not found — LAN-only collab")

    # Initialize store (needed for remotes)
    store = BlobStore()
    store.init()

    # 4. Collaborator IP — ask for their IP to auto-configure a remote
    print()
    from clavus.sync import load_remotes, save_remotes, Remote
    remotes = load_remotes(store)
    existing_urls = {r.url for r in remotes}

    prompt = "👥 Collaborator's Tailscale IP" if ts_ip else "👥 Collaborator's IP"
    print(f"{prompt} (IP or full URL, enter to skip)")
    try:
        collab_ip = input("   IP/URL [skip]: ").strip()
    except (EOFError, KeyboardInterrupt):
        collab_ip = ""
    if collab_ip:
        # Accept IP or full URL — strip http:// and extract port if present
        from urllib.parse import urlparse
        if collab_ip.startswith("http://") or collab_ip.startswith("https://"):
            parsed = urlparse(collab_ip)
            host = parsed.hostname or collab_ip
            port = parsed.port or port
        else:
            host = collab_ip
        url = f"http://{host}:{port}"
        if url not in existing_urls:
            name = host.replace(".", "-")
            remotes.append(Remote(name=name, url=url))
            save_remotes(store, remotes)
            print(f"   ✅ Added remote '{name}' → {url}")
            existing_urls.add(url)

    # Also add localhost relay if port is in use (already running)
    # Only if no collaborator IP was given — this IS the relay machine
    if not collab_ip and in_use:
        print(f"   🔗 Relay detected on port {port} — good!")
        relay_url = f"http://localhost:{port}"
        if relay_url not in existing_urls:
            remotes.append(Remote(name="relay", url=relay_url))
            save_remotes(store, remotes)
            print(f"   ✅ Added remote 'relay' → {relay_url}")
    elif collab_ip:
        # Remove any stale localhost remotes — they won't work on this machine
        before = len(remotes)
        remotes = [r for r in remotes if "localhost" not in r.url and "127.0.0.1" not in r.url]
        if len(remotes) < before:
            save_remotes(store, remotes)
            print(f"   🧹 Removed {before - len(remotes)} stale localhost remote(s)")

    # Summary
    remotes = load_remotes(store)
    if remotes:
        print(f"   📡 {len(remotes)} remote(s) configured")
    else:
        print("   ℹ️  No remotes — use 'clavus remote add' later or 'clavus join http://...'")

    # 5. Projects folder
    from clavus.config import DEFAULT_PROJECTS_DIR
    current_dir = cfg.projects_dir or DEFAULT_PROJECTS_DIR
    print(f"📁 Projects folder [{current_dir}]: ", end="")
    try:
        pd = input().strip()
        if pd:
            cfg.set("projects_dir", pd)
            Path(pd).mkdir(parents=True, exist_ok=True)
    except (EOFError, KeyboardInterrupt):
        pass
    print(f"   All synced projects go here")

    # 6. Ableton detection
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
        print(f"   ✅ Ableton: {ableton}")
        cfg.set("ableton_path", ableton)
    else:
        print("   ℹ️  Ableton not auto-detected")
        print("      Set: clavus config set ableton_path /path/to/Ableton")

    # Save
    cfg.save()

    print()
    print("✅ Setup complete!")
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
                    print(f"   ✅ {remote.name} — {len(projects)} project(s) found")
                else:
                    print(f"   ⚠️  {remote.name} — relay responded but no projects (run 'clavus init' on host)")
            except Exception:
                print(f"   ⚠️  {remote.name} — unreachable (is 'clavus share' running?)")
    # Smart next-step suggestions based on what was configured
    has_external = any("localhost" not in r.url for r in remotes)
    print()
    print("   Next:")
    if has_external:
        print("     clavus pull               — pull projects and start collaborating")
    print("     clavus tui                — terminal dashboard (press p to pull)")
    if not has_external:
        print("     clavus init /path/to.als  — track a local project")
    print()
    if has_external:
        print("   💡 Quick start: clavus pull && clavus tui")


def init_project(path_str: str | None, auto_confirm: bool = False) -> tuple[Optional[str], list[str]]:
    """Core init logic (non-interactive). Returns (project_name, log_lines). 
    
    If auto_confirm=True, skips all prompts and uses defaults. Used by TUI.
    Returns (None, [...]) on failure.
    """
    logs: list[str] = []
    if path_str:
        target = Path(path_str).resolve()
        als_path = find_als_file(target)
        if als_path is None:
            logs.append(f"❌ No .als file found at {target}")
            return None, logs
    else:
        return None, ["❌ No path provided"]

    store = BlobStore()
    store.init()

    # Check if already initialized
    existing = store.get_index(als_path.stem)
    if existing:
        logs.append(f"⚠️  Project '{als_path.stem}' already tracked at {existing.root_als}")
        return existing.name, logs

    # Project name from .als filename
    project_name = als_path.stem
    logs.append(f"📁 Project: {project_name}")

    # Parse the .als
    project = parse_als(als_path)
    logs.append(f"🔍 {project.track_count} tracks @ {project.bpm}bpm")

    # Copy to ~/Clavus/Projects/
    from clavus.helpers import get_projects_dir
    import shutil

    projects_root = get_projects_dir()
    target_dir = projects_root / project_name
    target_als = target_dir / f"{project_name}.als"

    if target_dir.exists():
        logs.append(f"⚠️  '{project_name}' already exists in {projects_root}")
        return project_name, logs

    source_dir = als_path.parent
    shutil.copytree(
        source_dir, target_dir,
        ignore=shutil.ignore_patterns("Backup*", "Ableton Project Info", ".DS_Store"),
        dirs_exist_ok=True,
    )
    logs.append(f"✅ Copied → {target_dir}")

    # Rename .als to match project name if they differ
    if not target_als.exists():
        existing = list(target_dir.glob("*.als"))
        if existing:
            existing[0].rename(target_als)
            logs.append(f"📛 Renamed {existing[0].name} → {target_als.name}")

    # Update als_path to the copy
    als_path = target_als
    project.file_path = str(als_path)

    config = ClavusConfig.load()
    clavus_proj = ClavusProject(
        name=project_name,
        root_als=str(als_path),
        created_at=time.time(),
        description="",
    )

    snap = store.save_snapshot(project, "Initial import", parent=None)
    clavus_proj.head = snap.hash
    store.update_ref("HEAD", snap.hash)
    store.update_ref("refs/tags/initial", snap.hash)
    store.set_index(clavus_proj)

    # Set as last project
    if store.index_path.exists():
        index = json.loads(store.index_path.read_text())
        index["_last_project"] = project_name
        store.index_path.write_text(json.dumps(index, indent=2, default=str))

    logs.append(f"✅ Initialized '{project_name}' — {project.track_count} tracks, snapshot {snap.short_hash()}")
    return project_name, logs


def cmd_init(args: argparse.Namespace) -> None:
    """Initialize a new Clavus project — friendly, guided setup."""
    if args.path:
        target = Path(args.path).resolve()
        als_path = find_als_file(target)
        if als_path is None:
            print(f"❌ No .als file found at {target}")
            print("   Specify the path to an .als file or a directory containing one.")
            sys.exit(1)
    else:
        # Auto-detect: scan common locations for .als files
        search_dirs = [
            Path.home() / "Desktop",
            Path.home() / "Documents",
            Path.home() / "Music" / "Ableton",
            Path.home() / "Ableton",
        ]
        als_files = []
        for d in search_dirs:
            if d.exists():
                for f in d.rglob("*.als"):
                    if "Backup" not in str(f) and "Ableton Project Info" not in str(f):
                        als_files.append(f)

        if not als_files:
            print("❌ No .als files found on Desktop, Documents, or Music/Ableton.")
            print("   Usage: clavus init /path/to/project.als")
            sys.exit(1)

        if len(als_files) == 1:
            als_path = als_files[0]
            print(f"📁 Found: {als_path.name}")
        else:
            print("📁 Found multiple .als files:")
            for i, f in enumerate(als_files[:20], 1):
                print(f"  {i}. {f.name}  ({f.parent})")
            print()
            print("  Enter number (or 0 to cancel): ", end="")
            try:
                choice = int(input().strip())
                if 1 <= choice <= len(als_files):
                    als_path = als_files[choice - 1]
                else:
                    print("❌ Cancelled.")
                    sys.exit(1)
            except (ValueError, EOFError):
                print("❌ Cancelled.")
                sys.exit(1)

    # ── Config check: prompt for author if not set ──
    config = ClavusConfig.load()
    if not config.author or config.author == getpass.getuser():
        print("👋 First, who's the author for this project?")
        author_input = input(f"   Author [{config.author or getpass.getuser()}]: ").strip()
        if author_input:
            config.author = author_input
            config.save()
            print(f"   Saved to {CONFIG_PATH}")
        else:
            # Keep the default (getpass.getuser())
            pass
        print()

    # ── Welcome ──
    print("╭──────────────────────────────────────────────╮")
    print("│  🎹  Clavus — Ableton Live collaboration     │")
    print("╰──────────────────────────────────────────────╯")
    print()

    store = BlobStore()
    store.init()

    # Check if already initialized
    existing = store.get_index(als_path.stem)
    if existing:
        print(f"⚠️  Project '{als_path.stem}' already tracked at {existing.root_als}")
        return

    # ── Project name ──
    suggested = als_path.stem
    print(f"📁 Project: {suggested}")
    if sys.stdin.isatty():
        name_input = input(f"   Name [{suggested}]: ").strip()
        project_name = name_input if name_input else suggested
    else:
        project_name = suggested
    print()

    # ── Description (optional) ──
    print("📝 Optional description (what is this project?)")
    if sys.stdin.isatty():
        desc_input = input(f"   Description []: ").strip()
    else:
        desc_input = ""
    print()

    # ── Parse ──
    print("🔍 Scanning .als file...")
    from clavus.progress import Spinner
    with Spinner("parsing project file..."):
        project = parse_als(als_path)
    print()

    # ── Summary ──
    print(project_summary(project))
    print()

    # ── Confirm ──
    if sys.stdin.isatty():
        confirm = input("   Ready to track this project? [Y/n]: ").strip().lower()
        if confirm and confirm not in ("y", "yes", ""):
            print("✋ Cancelled. Nothing was saved.")
            return
    print()

    # ── Init ──
    # Copy project to ~/Clavus/Projects/ so everything lives in one place
    from clavus.helpers import get_projects_dir
    import shutil

    projects_root = get_projects_dir()
    target_dir = projects_root / project_name
    target_als = target_dir / f"{project_name}.als"

    if target_dir.exists():
        print(f"⚠️  '{project_name}' already exists in {projects_root}")
        print(f"   Use 'clavus project \"{project_name}\"' to switch to it.")
        return

    print(f"📁 Copying project to {target_dir}...")
    source_dir = als_path.parent
    shutil.copytree(
        source_dir, target_dir,
        ignore=shutil.ignore_patterns("Backup*", "Ableton Project Info", ".DS_Store"),
        dirs_exist_ok=True,
    )

    # Rename .als to match project name if they differ
    if not target_als.exists():
        existing = list(target_dir.glob("*.als"))
        if existing:
            existing[0].rename(target_als)
            print(f"   📛 Renamed {existing[0].name} → {target_als.name}")

    # Update als_path to the copy and re-parse
    als_path = target_als
    project.file_path = str(als_path)  # update path in parsed project
    print(f"   ✅ Copied → {target_dir}")
    print()

    clavus_proj = ClavusProject(
        name=project_name,
        root_als=str(als_path),
        created_at=time.time(),
        description=desc_input,
    )

    snap = store.save_snapshot(project, "Initial import", parent=None)
    clavus_proj.head = snap.hash
    store.update_ref("HEAD", snap.hash)
    store.update_ref(f"refs/tags/initial", snap.hash)
    store.set_index(clavus_proj)

    # ── Done ──
    print(f"✅ Initialized Clavus project '{clavus_proj.name}'")
    print(f"   .als: {als_path}")
    print(f"   Tracks: {project.track_count} @ {project.bpm}bpm")
    print(f"   Snapshot: {snap.short_hash()}")
    if clavus_proj.description:
        print(f"   Notes: {clavus_proj.description}")
    print()
    print("   Next steps:")
    print(f"     open {target_dir}                Open in Finder")
    print(f'     clavus snapshot "my changes"     Save a snapshot')
    print(f"     clavus log                       View history")
    print(f"     clavus push                      Share with collaborators")
    print()
    print(f"   💡 Your original at {source_dir} is untouched.")
    print(f"      Work from {target_dir} going forward.")


def cmd_projects(args: argparse.Namespace) -> None:
    """List all tracked projects."""
    store = BlobStore()
    projects = store.list_projects()
    if not projects:
        print("📁 No Clavus projects found.")
        print("   Run 'clavus init <path>' to add one.")
        return

    print(f"📁 Clavus projects ({len(projects)}):")
    print()
    for p in sorted(projects, key=lambda x: x.name):
        head_str = f" @ {p.head[:8]}" if p.head else " (no snapshots)"
        als_exists = "✅" if Path(p.root_als).exists() else "❌"
        shared_icon = "🌐" if p.shared else "🔒"
        print(f"  {shared_icon} {p.name:<30} {als_exists} {p.root_als}{head_str}")
    print()
    print(f"  Current: {store.read_ref('_last_project') or 'none'}")

    # Show which is active
    try:
        _, active = get_store_and_project()
        print(f"  Active: {active.name}")
    except SystemExit:
        pass


def cmd_project(args: argparse.Namespace) -> None:
    """Switch the active project, or toggle sharing."""
    store = BlobStore()
    proj = store.get_index(args.name)
    if not proj:
        print(f"❌ Project '{args.name}' not found.")
        print("   Run 'clavus projects' to see available projects.")
        sys.exit(1)

    # Toggle share/private
    if args.share:
        if proj.shared:
            print(f"🌐 Project '{args.name}' is already shared.")
        else:
            proj.shared = True
            store.set_index(proj)
            print(f"🌐 Project '{args.name}' is now shared — visible to collaborators.")
        return
    if args.private:
        if not proj.shared:
            print(f"🔒 Project '{args.name}' is already private.")
        else:
            proj.shared = False
            store.set_index(proj)
            print(f"🔒 Project '{args.name}' is now private — hidden from collaborators.")
        return

    store.set_index(proj)
    print(f"✅ Switched to project '{args.name}'")
    print(f"   Path: {proj.root_als}")
    if proj.head:
        print(f"   HEAD: {proj.head[:8]}")
    else:
        print(f"   (no snapshots yet)")
    print(f"   Branch: {proj.branch}")
    print(f"   {'🌐 Shared' if proj.shared else '🔒 Private'}")


def create_snapshot(message: str, allow_frozen: bool = True) -> tuple[Optional[str], list[str]]:
    """Core snapshot logic (non-interactive). Returns (snap_hash, log_lines). 
    
    If allow_frozen=True, skips frozen track prompt. Used by TUI.
    Returns (None, [...]) on failure or no-change.
    """
    logs: list[str] = []
    store, proj = get_store_and_project()
    als_path = Path(proj.root_als) if proj.root_als else None
    if not als_path or not als_path.is_file():
        from clavus.helpers import get_projects_dir
        candidate = get_projects_dir() / proj.name / f"{proj.name}.als"
        if candidate.is_file():
            als_path = candidate
        else:
            logs.append(f"❌ .als file not found: {proj.root_als or 'projects/' + proj.name}")
            # Also try the Project subfolder convention
            candidate2 = get_projects_dir() / proj.name / f"{proj.name} Project" / f"{proj.name}.als"
            if candidate2.is_file():
                als_path = candidate2
                logs.append(f"   Found in project subfolder: {candidate2}")
            else:
                return None, logs

    project = parse_als(als_path)
    frozen_count = sum(1 for t in project.tracks if t.is_frozen) if project else 0
    if frozen_count:
        if not allow_frozen:
            logs.append(f"  ⚠️  {frozen_count} frozen track(s) — pass allow_frozen=True to skip")
            return None, logs
        else:
            logs.append(f"  ⚠️  {frozen_count} frozen track(s) — snapshots may not restore on other platforms")

    snap = store.save_snapshot(
        project, message=message, parent=proj.head, tags=[],
    )

    # Warn if snapshot has no .als backup — it cannot restore
    if not snap.als_hash:
        logs.append(f"  ⚠️  Snapshot saved but .als file has no backup — restore will not be possible")

    prev = store.load_snapshot(proj.head) if proj.head else None
    if prev and snap.als_hash and snap.als_hash == prev.als_hash:
        logs.append(f"⚠️  No changes — {proj.name}.als is identical to last snapshot")
        return None, logs

    store.update_ref("HEAD", snap.hash)
    proj.head = snap.hash
    store.set_index(proj)

    if prev:
        prev_project = store.load_project(prev.hash)
        if prev_project:
            diff = diff_projects(prev_project, project)
            logs.append(f"📸 {snap.short_hash()} — '{snap.message}'")
            logs.append(f"   {diff.summary}")
        else:
            logs.append(f"📸 {snap.short_hash()} — '{snap.message}'")
    else:
        logs.append(f"📸 {snap.short_hash()} — '{snap.message}' ({project.track_count} tracks @ {project.bpm}bpm)")

    return snap.hash, logs


def cmd_snapshot(args: argparse.Namespace) -> None:
    """Create a new snapshot of the current project state."""
    store, proj = get_store_and_project()
    als_path = Path(proj.root_als) if proj.root_als else None
    # Fallback: if root_als is empty or missing, look in projects dir
    if not als_path or not als_path.is_file():
        from clavus.helpers import get_projects_dir
        candidate = get_projects_dir() / proj.name / f"{proj.name}.als"
        if candidate.is_file():
            als_path = candidate
        else:
            print(f"❌ .als file not found: {proj.root_als or 'projects/' + proj.name}")
            sys.exit(1)

    # Prompt for message if not provided
    # Support both positional `clavus snapshot "msg"` and `--message`/`-m` flag
    message = args.message_flag if getattr(args, 'message_flag', None) else args.message
    if not message:
        try:
            message = input("  Snapshot message (or blank to cancel): ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\n❌ Snapshot cancelled.")
            return
        if not message:
            print("❌ Snapshot cancelled.")
            return

    # Parse current state
    project = parse_als(als_path)

    # Check for frozen tracks (cross-platform crash risk)
    frozen_count = sum(1 for t in project.tracks if t.is_frozen) if project else 0
    if frozen_count:
        if args.allow_frozen:
            print(f"  ⚠️  {frozen_count} frozen track(s) — proceeding anyway (--allow-frozen)")
        else:
            print(f"  ⚠️  {frozen_count} frozen track(s) detected — will crash on other platforms.")
            print(f"  Unfreeze tracks in Ableton first for cross-platform compatibility.")
            try:
                choice = input("  Continue anyway? [y/N]: ").strip().lower()
            except (EOFError, KeyboardInterrupt):
                print("\n❌ Snapshot cancelled.")
                return
            if choice != 'y':
                print("❌ Snapshot cancelled.")
                return

    # Create snapshot
    snap = store.save_snapshot(
        project,
        message=message,
        parent=proj.head,
        tags=args.tag.split(",") if args.tag else [],
    )

    # Check if anything actually changed (compare raw .als bytes, not parsed structure)
    prev = store.load_snapshot(proj.head) if proj.head else None
    if prev and snap.als_hash and snap.als_hash == prev.als_hash:
        print(f"⚠️  No changes detected — project state is identical to last snapshot.")
        print(f"   Current HEAD: {snap.short_hash()} — '{store.load_snapshot(proj.head).message if store.load_snapshot(proj.head) else ''}'")
        return

    # Update references
    store.update_ref("HEAD", snap.hash)
    proj.head = snap.hash
    store.set_index(proj)

    # Show diff from previous snapshot
    if args.parent:
        parent_snap = store.load_snapshot(args.parent)
    elif snap.parent:
        parent_snap = store.load_snapshot(snap.parent)
    else:
        parent_snap = None

    if parent_snap:
        parent_project = store.load_project(parent_snap.hash)
        if parent_project:
            diff = diff_projects(parent_project, project)
            print(f"📸 Snapshot: {snap.short_hash()} — '{snap.message}'")
            print(f"   {diff.summary}")
            if args.verbose:
                print()
                print(format_diff(diff, verbose=True))
        else:
            print(f"📸 Snapshot: {snap.short_hash()} — '{snap.message}'")
    else:
        print(f"📸 Snapshot: {snap.short_hash()} — '{snap.message}'")
        print(f"   {project.track_count} tracks @ {project.bpm}bpm (initial state)")

    print(f"   📋 Run 'clavus log' to see history.")


def cmd_log(args: argparse.Namespace) -> None:
    """Show snapshot history."""
    store, proj = get_store_and_project()
    current_hash = proj.head
    count = 0

    if not current_hash:
        print("📋 No snapshots yet. Run 'clavus snapshot' to create one.")
        return

    # Collect all branch refs for graph display
    branch_refs: dict[str, str] = {"HEAD": current_hash}
    if store.refs_dir.exists():
        for ref_file in store.refs_dir.glob("heads/*"):
            branch_refs[ref_file.name] = ref_file.read_text().strip()

    print(f"📋 Snapshot history for '{proj.branch}'")
    print()

    # Build a set of all snapshots for the graph
    all_snaps: dict[str, Snapshot] = {}
    def collect(h: str):
        while h and h not in all_snaps:
            snap = store.load_snapshot(h)
            if snap:
                all_snaps[h] = snap
                h = snap.parent if snap.parent else ""
            else:
                break
    for h in branch_refs.values():
        collect(h)

    # Show graph or linear history
    if args.graph:
        # Build a simple graph with branches
        sorted_hashes = sorted(all_snaps.keys(),
                              key=lambda h: all_snaps[h].timestamp if h in all_snaps else 0,
                              reverse=True)

        # Which branches point to each snapshot
        hash_branches: dict[str, list[str]] = {}
        for bname, bh in branch_refs.items():
            h = bh
            while h:
                if h not in hash_branches:
                    hash_branches[h] = []
                if bname not in hash_branches[h]:
                    hash_branches[h].append(bname)
                snap = all_snaps.get(h)
                h = snap.parent if snap else ""

        for i, h in enumerate(sorted_hashes):
            if count >= (args.limit or 30):
                break
            snap = all_snaps.get(h)
            if not snap:
                continue
            count += 1

            # Graph lines
            graph = "|"
            if i > 0:
                prev = sorted_hashes[i-1]
                if prev in all_snaps and all_snaps[prev].parent != h:
                    graph = "\\"

            time_str = time.strftime("%m/%d %H:%M", time.localtime(snap.timestamp))

            # Show branch labels
            labels = hash_branches.get(h, [])
            label_str = ""
            for lb in labels:
                if lb in ("HEAD", proj.branch):
                    label_str += f" *{lb}*"
                else:
                    label_str += f" {lb}"

            print(f"  {graph} {snap.short_hash()}  {time_str}{label_str}")
            if args.verbose:
                print(f"  |    Tracks: {snap.track_count}  BPM: {snap.bpm}")
            print(f"  |    {snap.message[:70]}")

        if count == 0:
            print("  (no snapshots to show)")
    else:
        # Linear history (original behavior)
        while current_hash and count < (args.limit or 20):
            snap = store.load_snapshot(current_hash)
            if not snap:
                break

            is_head = "➡ " if current_hash == store.read_ref("HEAD") else "  "
            time_str = time.strftime("%Y-%m-%d %H:%M", time.localtime(snap.timestamp))

            tags = ""
            if snap.tags:
                tags = f" [{', '.join(snap.tags[:3])}]"

            print(f"  {is_head}{snap.short_hash()}  {time_str}")
            if args.verbose:
                print(f"     Tracks: {snap.track_count}  BPM: {snap.bpm}")
            print(f"     {snap.message}{tags}")

            current_hash = snap.parent
            count += 1

            if count < (args.limit or 20) and current_hash:
                print()

        if current_hash and count >= (args.limit or 20):
            print()
            print(f"  ... and more. Use --limit to show more.")



def cmd_diff(args: argparse.Namespace) -> None:
    """Show what changed in a snapshot vs its parent."""
    store, proj = get_store_and_project()

    if args.hash:
        hash_str = resolve_snapshot(store, args.hash)
        if not hash_str:
            print(f"❌ Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = store.read_ref("HEAD")
        if not hash_str:
            print("📋 No snapshots yet.")
            return

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"❌ Snapshot not found (resolved: '{hash_str}', meta: {store.objects_dir / hash_str[:2] / (hash_str + '.meta')})")
        return

    current_project = store.load_project(hash_str)
    if not current_project:
        print(f"❌ Could not load project data for {hash_str}")
        return

    if snap.parent:
        parent_project = store.load_project(snap.parent)
        if parent_project:
            diff = diff_projects(parent_project, current_project)

            if args.visual:
                # Render visual timeline diff with clip-level detail
                try:
                    from clavus.visual_diff import render_diff_cli
                    print(f"📊 {snap.short_hash()} — '{snap.message}'")
                    diff = diff_projects(parent_project, current_project)
                    print(render_diff_cli(
                        diff=diff,
                        before_proj=parent_project,
                        after_proj=current_project,
                    ))
                    return
                except ImportError:
                    pass  # Fall through to text diff

            print(f"📊 {snap.short_hash()} — '{snap.message}'")
            print()
            print(format_diff(diff, verbose=args.verbose))
            return

    # No parent — show basic info
    print(f"📊 {snap.short_hash()} — '{snap.message}'")
    print(f"   {snap.track_count} tracks @ {snap.bpm}bpm (initial state)")


def cmd_status(args: argparse.Namespace) -> None:
    """Show current project status."""
    store, proj = get_store_and_project()

    als_path = Path(proj.root_als)
    als_exists = als_path.exists()
    last_snap = store.load_snapshot(proj.head) if proj.head else None

    print(f"📁 '{proj.name}'")
    print(f"   Path: {proj.root_als}")
    print(f"   Branch: {proj.branch}")
    print(f"   Status: {'✅ exists' if als_exists else '❌ missing'}")
    print()

    if last_snap:
        project = None
        if als_exists:
            project = parse_als(als_path)
            old_project = store.load_project(last_snap.hash)
            if old_project and project:
                diff = diff_projects(old_project, project)
                print(f"   HEAD: {last_snap.short_hash()} — '{last_snap.message}'")
                if diff.summary != "No changes":
                    print(f"   ⚠️  Unsaved changes detected:")
                    print(f"      {diff.summary}")
                else:
                    print(f"   ✅ Up to date with last snapshot")
        else:
            print(f"   HEAD: {last_snap.short_hash()} — '{last_snap.message}'")
    else:
        print(f"   No snapshots yet.")


def cmd_watch(args: argparse.Namespace) -> None:
    """Start the file watcher daemon, or manage the system service.

    clavus watch            — start daemon (foreground, tracks active project)
    clavus watch --once     — take one snapshot if changed and exit
    clavus watch install    — install as system service (launchd/systemd)
    clavus watch start      — start the installed service
    clavus watch stop       — stop the service
    clavus watch restart   — stop + start
    clavus watch status    — show if service is running

    The daemon auto-tracks whichever project is active in Clavus. Switch projects
    in the TUI or CLI and the watcher picks it up without restarting.
    """
    sub = getattr(args, 'subcommand', None)

    if sub == 'install':
        from clavus.watch import install_service
        ok = install_service()
        sys.exit(0 if ok else 1)
    elif sub == 'start':
        from clavus.watch import start_service
        ok = start_service()
        sys.exit(0 if ok else 1)
    elif sub == 'stop':
        from clavus.watch import stop_service
        ok = stop_service()
        sys.exit(0 if ok else 1)
    elif sub == 'restart':
        from clavus.watch import stop_service, start_service
        stop_service()
        ok = start_service()
        sys.exit(0 if ok else 1)
    elif sub == 'status':
        from clavus.watch import service_status
        status = service_status()
        if status:
            print(f'✅ Watch service: {status}')
        else:
            print(f'⚠️  Service status not available on this platform')
        return
    else:
        # Default: run the daemon. No store/proj args — reads from index.json each poll.
        from clavus.watch import LOG_FILE
        watch(
            cooldown=args.cooldown,
            verbose=not args.quiet,
            once=args.once,
            log_file=LOG_FILE,
        )


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
        print("❌ Relay server requires fastapi and uvicorn.")
        print("   Install with: pip install fastapi uvicorn")
        sys.exit(1)
    cfg = ClavusConfig.load()
    host = args.host or cfg.host
    port = args.port or cfg.port
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
                print(f'⚠️  Relay already running (PID {old_pid}). Use --kill to stop.')
                return
            except (ProcessLookupError, ValueError):
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
            import httpx
            r = httpx.get(f'http://127.0.0.1:{port}/api/ping', timeout=3)
            if r.status_code == 200:
                print(f'✅ Relay running in background (PID {pid})')
                print(f'   Share URL: http://127.0.0.1:{port}')
                return
        except Exception:
            pass
        print(f'⚠️  Relay started (PID {pid}) but not responding — logs: ~/.clavus/relay.log')
        return
    
    # Check for --kill flag
    if getattr(args, 'kill', False):
        pid_path = Path.home() / '.clavus' / 'relay.pid'
        if pid_path.exists():
            try:
                pid = int(pid_path.read_text().strip())
                os.kill(pid, 9)
                pid_path.unlink()
                print(f'✅ Relay (PID {pid}) stopped.')
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
        print("❌ Relay server requires fastapi and uvicorn.")
        print("   Install with: pip install fastapi uvicorn")
        sys.exit(1)

    cfg = ClavusConfig.load()
    host = args.host or cfg.host
    port = args.port or cfg.port

    # Check if port is already in use — try to kill stale Clavus relay
    import socket, platform, signal, subprocess as sp, time
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    if sock.connect_ex(("127.0.0.1", port)) == 0:
        sock.close()
        print(f"   ⚠️  Port {port} in use — checking for stale Clavus relay...")
        killed = False
        try:
            import httpx
            r = httpx.get(f"http://127.0.0.1:{port}/api/sync/pull", params={"name": "_"}, timeout=2)
            if r.status_code in (200, 404):
                print(f"   🔄 Restarting stale Clavus relay on port {port}...")
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
            print(f"   ❌ Port {port} still in use. Stop the other relay first:")
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
        print()
        print(f"  ⚠️  tailscale serve not configured — MagicDNS URL won't work for collaborators")
        print(f"  💡 Fixing: tailscale serve --bg --http {port} http://localhost:{port}")
        try:
            subprocess.run(
                ["tailscale", "serve", "--bg", "--http", str(port), f"http://localhost:{port}"],
                capture_output=True, text=True, timeout=10,
            )
            print(f"  ✅ tailscale serve enabled")
        except Exception as e:
            print(f"  ❌ Could not enable tailscale serve: {e}")
        print()

    print(f"  🎹 Clavus Share")
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
        print(f"  🔒 Scoped to project: {args.project}")
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
                print(f'⚠️  Relay already running (PID {old_pid}). Use --kill to stop.')
                return
            except (ProcessLookupError, ValueError):
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
            import httpx
            r = httpx.get(f'http://127.0.0.1:{port}/api/ping', timeout=3)
            if r.status_code == 200:
                print(f'✅ Relay running in background (PID {pid})')
                print(f'   Share URL: http://127.0.0.1:{port}')
                return
        except Exception:
            pass
        print(f'⚠️  Relay started (PID {pid}) but not responding — logs: ~/.clavus/relay.log')
        return
    
    # Check for --kill flag
    if getattr(args, 'kill', False):
        pid_path = Path.home() / '.clavus' / 'relay.pid'
        if pid_path.exists():
            try:
                pid = int(pid_path.read_text().strip())
                os.kill(pid, 9)
                pid_path.unlink()
                print(f'✅ Relay (PID {pid}) stopped.')
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
    print("🎹  Clavus — Join a Collaboration Session")
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
        print("    💡  You can also scan your network for active relays:")
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
    print(f"    ✅  Clavus {v}")

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
        print(f"    ✅  Ableton detected")
    else:
        print(f"    ⚠️  Ableton not auto-detected — sync still works,")
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
        print(f"    ✅  Tailscale running ({ts_ip})")
    elif is_tailscale_addr:
        print(f"    ❌  Tailscale required — not installed or not running")
        print(f"        Download: https://tailscale.com/download")
        print(f"        Start it: tailscale up")
        print()
        print(f"    ⚠️  Fix this, then re-run: clavus join {raw}")
        return
    else:
        print(f"    ℹ️  Tailscale not detected — LAN-only collaboration")

    # 2d. Storage init
    store = BlobStore()
    store.init()
    print(f"    ✅  Storage ready")
    print()

    # ── Phase 3: Test Connectivity ───────────────────────────────────
    print(f"🔗  Connecting to {host}:{port}...")
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
            print(f"    ❌  Cannot resolve '{host}' — hostname not found on your network")
            print()
            print(f"    Check the hostname and try again. If this is a Tailscale")
            print(f"    MagicDNS address, make sure Tailscale is running:")
            print(f"      tailscale status")
        elif is_tailscale_addr:
            print(f"    ❌  Cannot reach {host}:{port}")
            print()
            print(f"    This address uses Tailscale MagicDNS. Check:")
            print(f"    1. Is Tailscale running?        → tailscale status")
            print(f"    2. On the same tailnet?         → tailscale status")
            print(f"    3. Host shared their machine?   → ask the host to check")
            print(f"       tailscale.com → Machines → (their machine) → Share")
            print(f"    4. Is the relay running?        → host: clavus share")
            if ts_running:
                print(f"    5. Try the Tailscale IP direct:  → clavus join <100.x.x.x>:{port}")
        else:
            print(f"    ❌  Cannot reach {host}:{port} (TCP connection refused)")
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
            print(f"    ⚠️  Port {port} is open, but it's not a Clavus relay.")
            print(f"       Make sure the host is running 'clavus share', not another service.")
            return

        projects_resp = client.client.get(f"{base}/api/projects", timeout=8)
        if projects_resp.status_code == 200:
            info = projects_resp.json()
        print(f"    ✅  Connected — relay is online")
    except Exception as e:
        err = str(e)
        if "timed out" in err.lower() or "timeout" in err.lower():
            print(f"    ❌  Connection timed out")
            print(f"       Port {port} is reachable but the relay isn't responding.")
            print(f"       The host may have stopped 'clavus share' or crashed.")
        else:
            print(f"    ❌  Connection error: {e}")
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
            print(f"    📡  Already connected to {remote_name}")
        else:
            # Same URL, different name — update name (relay may have rebooted)
            remotes = [r for r in remotes if r.url != base]
            remotes.append(Remote(name=remote_name, url=base))
            save_remotes(store, remotes)
            print(f"    ✅  Updated remote name: {remote_name}")
    else:
        if remotes:
            # Already have a different remote — warn before replacing
            current = remotes[0]
            print(f"    ⚠️  You already have a remote configured: {current.name}")
            print(f"       Replacing it with {remote_name} (single remote at a time).")
            print()
            remotes = [Remote(name=remote_name, url=base)]
            save_remotes(store, remotes)
            print(f"    ✅  Switched to: {remote_name}")
        else:
            remotes.append(Remote(name=remote_name, url=base))
            save_remotes(store, remotes)
            print(f"    ✅  Saved remote: {remote_name}")

    # ── Phase 5: Pull Existing Projects ──────────────────────────────
    projects = info.get("projects", [])

    if not projects:
        print()
        print(f"    ℹ️  No projects on relay yet")
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
                print(f"❌ {result['error']}")
            else:
                s = result.get("snapshots", 0)
                c = result.get("cues", 0)
                print(f"{s} snapshots, {c} cues")
                blob_count, failed = pull_snapshot_blobs(store, proj_data, Remote(name=remote_name, url=base))
                if blob_count:
                    print(f"        📦 {blob_count} audio blob(s)")
                if failed:
                    print(f"        ⚠️  {len(failed)} blob(s) failed — check disk space or network")

    # ── Phase 6: Next Steps ──────────────────────────────────────────
    print()
    print("═" * 48)
    print("✅  You're all set!")
    print()
    print("    Next steps:")
    print(f"      clavus tui       — Open the dashboard (recommended)")
    if not projects:
        print(f"      clavus init ...  — Share one of YOUR projects with the group")
        print(f"      clavus push      — Push it to the relay")
        print()
        print(f"    💡  Quick start:  clavus init /path/to/song.als && clavus push && clavus tui")
    else:
        print()
        print(f"    💡  Run 'clavus tui' — you'll see your projects, cues, and history.")
    print()


def cmd_tui(args: argparse.Namespace) -> None:
    """Launch the Textual TUI."""
    try:
        from clavus.tui import run_tui
    except ImportError:
        print("❌ TUI requires textual and httpx.")
        print("   Install with: pip install textual httpx")
        sys.exit(1)
    cfg = ClavusConfig.load()
    url = args.connect or cfg.default_server
    debug = getattr(args, 'debug', False)
    run_tui(url=url, debug=debug)


def cmd_cue(args: argparse.Namespace) -> None:
    """Add a timeline-anchored comment."""
    store, proj = get_store_and_project()
    head = store.read_ref("HEAD")

    position = args.position or "0.0.0"
    # Strip @ prefix if present
    if position.startswith("@"):
        position = position[1:]
    # Validate position format — must be bars.beats.sixteenths or bars:beats
    def _pos_is_ok(p: str) -> bool:
        try:
            if ":" in p:
                parts = p.split(":")
                int(parts[0]); int(parts[1]) if len(parts) > 1 else 0
                return True
            parts = p.split(".")
            for part in parts:
                int(part)
            return True
        except (ValueError, TypeError):
            return False
    if position != "0.0.0" and not _pos_is_ok(position):
        print(f"  ⚠ Invalid position '{position}' — using 0.0.0 instead")
        position = "0.0.0"

    cue = add_cue_command(
        text=args.text,
        position=position,
        track=args.track or "",
        author=args.author or "",
        store=store,
    )

    if cue:
        print(f"💬 Cue added at @{position}")
        print(f"   \"{cue.text}\"")
        print(f"   id: {cue.id}")
        if args.track:
            print(f"   Track: {args.track}")
        if args.author:
            print(f"   Author: {args.author}")
        print(f"   📋 Reply: clavus cue reply {cue.id} \"your response\"")


def cmd_cue_reply(args: argparse.Namespace) -> None:
    """Reply to a cue thread."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    head = store.read_ref("HEAD")

    reply = cues.reply(args.cue_id, args.text, snapshot_hash=head or "")
    if reply:
        print(f"💬 Reply added to cue {args.cue_id}")
        print(f"   \"{args.text}\"")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_resolve(args: argparse.Namespace) -> None:
    """Resolve a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    cue = cues.resolve(args.cue_id, note=args.note or "")
    if cue:
        print(f"✅ Cue {args.cue_id} resolved")
        if args.note:
            print(f"   \"{args.note}\"")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_skip(args: argparse.Namespace) -> None:
    """Skip a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    cue = cues.skip(args.cue_id, reason=args.reason or "")
    if cue:
        print(f"⏭ Cue {args.cue_id} skipped")
        if args.reason:
            print(f"   \"{args.reason}\"")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cues(args: argparse.Namespace) -> None:
    """List all cues."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    filter_ = CueFilter()
    if args.pending_only:
        filter_.status = "pending"
    if args.author:
        filter_.author = args.author

    all_cues = cues.list_cues(filter_)
    print(format_cue_list(all_cues, verbose=args.verbose))


def cmd_config(args: argparse.Namespace) -> None:
    """View or edit clavus configuration."""
    cfg = ClavusConfig.load()

    # ── Set a value ──
    if args.key and args.value is not None:
        valid_keys = {"author", "port", "host", "default_server", "default_project"}
        if args.key not in valid_keys:
            print(f"❌ Unknown setting '{args.key}'.")
            print(f"   Valid keys: {', '.join(sorted(valid_keys))}")
            return
        setattr(cfg, args.key, args.value)
        cfg.save()
        print(f"✅ {args.key} = {args.value}")
        return

    # ── Show single value ──
    if args.key:
        val = getattr(cfg, args.key, "")
        if isinstance(val, str):
            val = f"'{val}'"
        print(f"  {args.key} = {val}")
        return

    # ── Show all ──
    print(f"  Clavus Configuration ({CONFIG_PATH})")
    print()
    for k, v in cfg.to_dict().items():
        print(f"    {k} = {v}")
    print()
    print(f"  Run 'clavus setup' for interactive setup.")
    print(f"  Run 'clavus config <key> <value>' to set a value.")


def cmd_cue_assign(args: argparse.Namespace) -> None:
    """Assign a cue to someone."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.assign(args.cue_id, args.name)
    if result:
        print(f"👤 {args.name} assigned to cue {args.cue_id}")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_unassign(args: argparse.Namespace) -> None:
    """Remove assignee from a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.unassign(args.cue_id)
    if result:
        print(f"👤 Unassigned cue {args.cue_id}")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_start(args: argparse.Namespace) -> None:
    """Mark a cue as in-progress."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.start(args.cue_id)
    if result:
        print(f"▶ Cue {args.cue_id} marked as in-progress")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_stop(args: argparse.Namespace) -> None:
    """Mark a cue as no longer in-progress."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    result = cues.stop(args.cue_id)
    if result:
        print(f"⏸ Cue {args.cue_id} no longer in-progress")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_delete(args: argparse.Namespace) -> None:
    """Permanently delete a cue."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)
    if cues.delete(args.cue_id):
        print(f"🗑 Deleted cue {args.cue_id}")
    else:
        print(f"❌ Cue '{args.cue_id}' not found.")


def cmd_cue_archive(args: argparse.Namespace) -> None:
    """Archive a specific cue, or all cues."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    if args.cue_id:
        ok = cues.archive(args.cue_id)
        if ok:
            print(f"📦 Archived cue {args.cue_id}")
        else:
            print(f"❌ Cue '{args.cue_id}' not found.")
    else:
        count = cues.archive_resolved()
        print(f"📦 Archived {count} cue(s).")


def cmd_branch(args: argparse.Namespace) -> None:
    """List or create branches."""
    store, proj = get_store_and_project()

    if args.delete:
        # Delete a branch
        branch_ref = f"heads/{args.delete}"
        existing = store.read_ref(branch_ref)
        if not existing:
            print(f"❌ Branch '{args.delete}' not found.")
            return
        if args.delete == proj.branch:
            print(f"❌ Cannot delete the current branch '{args.delete}'.")
            return
        store.delete_ref(branch_ref)
        print(f"🗑 Deleted branch '{args.delete}'")
        return

    if args.list or not args.name:
        # List all branches
        branches = []
        if store.refs_dir.exists():
            for ref_file in store.refs_dir.glob("heads/*"):
                branches.append(ref_file.name)
        if not branches:
            branches = ["main"]
        print(f"🌿 Branches for '{proj.name}'")
        print()
        for b in sorted(branches):
            prefix = "* " if b == proj.branch else "  "
            snap = store.read_ref(f"heads/{b}")
            snap_str = f" ({snap[:8]})" if snap else ""
            print(f"  {prefix}{b}{snap_str}")
        return

    # Create a new branch at current HEAD
    branch_ref = f"heads/{args.name}"
    head = store.read_ref("HEAD")
    if not head:
        print(f"❌ No snapshots yet. Create one first with 'clavus snapshot'.")
        return

    # Check if branch already exists
    existing = store.read_ref(branch_ref)
    if existing:
        print(f"⚠️  Branch '{args.name}' already exists (at {existing[:8]})")
        return

    store.update_ref(branch_ref, head)

    print(f"🌿 Created branch '{args.name}' at {head[:8]}")


def cmd_checkout(args: argparse.Namespace) -> None:
    """Switch branches."""
    store, proj = get_store_and_project()

    if args.b:
        # Create and switch
        head = store.read_ref("HEAD")
        if not head:
            print(f"❌ No snapshots yet.")
            return
        store.update_ref(f"heads/{args.name}", head)

    branch_ref = f"heads/{args.name}"
    branch_head = store.read_ref(branch_ref)
    if not branch_head:
        print(f"❌ Branch '{args.name}' not found.")
        print(f"   Available: {[f.stem for f in store.refs_dir.glob('heads/*')]}")
        return

    # Update project to point to new branch
    proj.branch = args.name
    proj.head = branch_head
    store.update_ref("HEAD", branch_head)
    store.set_index(proj)

    snap = store.load_snapshot(branch_head)
    msg = f" — '{snap.message}'" if snap else ""
    print(f"✅ Switched to branch '{args.name}' at {branch_head[:8]}{msg}")


def cmd_merge(args: argparse.Namespace) -> None:
    """Merge another branch into the current branch."""
    store, proj = get_store_and_project()

    # Get current HEAD
    current_head = store.read_ref("HEAD")
    if not current_head:
        print(f"❌ No snapshots on current branch.")
        return

    # Get the branch to merge
    merge_ref = f"heads/{args.branch}"
    merge_head = store.read_ref(merge_ref)
    if not merge_head:
        print(f"❌ Branch '{args.branch}' not found.")
        return

    if merge_head == current_head:
        print(f"✅ Already up to date — branches are at the same snapshot.")
        return

    current_snap = store.load_snapshot(current_head)
    merge_snap = store.load_snapshot(merge_head)

    # Walk back to find merge base
    def ancestors(hash_str: str) -> list[str]:
        chain = []
        while hash_str:
            chain.append(hash_str)
            snap = store.load_snapshot(hash_str)
            hash_str = snap.parent if snap else None
        return chain

    current_ancestors = ancestors(current_head)
    merge_ancestors = ancestors(merge_head)

    # Find merge base (first common ancestor)
    merge_base = None
    for ca in current_ancestors:
        if ca in merge_ancestors:
            merge_base = ca
            break

    if merge_base is None:
        print(f"⚠️  No common ancestor found. Branches have diverged completely.")
        return

    if merge_base == merge_head:
        # Fast-forward: merge_branch is ancestor of current
        print(f"✅ Already up to date — '{args.branch}' is behind current branch.")
        return

    if merge_base == current_head:
        # Fast-forward: current is ancestor of merge_branch
        if not args.no_ff:
            # Fast-forward: just move HEAD forward
            proj.head = merge_head
            store.update_ref("HEAD", merge_head)
            store.set_index(proj)
            print(f"⏩ Fast-forward merged '{args.branch}' into '{proj.branch}'")
            print(f"   {current_head[:8]}..{merge_head[:8]}")
            return

    # Create a merge commit

    # Parse the current .als for the merge snapshot
    als_path = Path(proj.root_als)
    project = None
    if als_path.exists():
        project = parse_als(als_path)

    if project is None:
        print(f"❌ Could not parse .als file: {proj.root_als}")
        return

    # Save snapshot with TWO parents
    snap = store.save_snapshot(
        project,
        message=args.message or f"Merge branch '{args.branch}' into '{proj.branch}'",
        parent=current_head,
        tags=["merge"],
    )

    # Store the second parent in a sidecar file
    meta_path = store.objects_dir / snap.hash[:2] / f"{snap.hash}.meta"
    if meta_path.exists():
        import json
        data = json.loads(meta_path.read_text())
        data["parent2"] = merge_head
        meta_path.write_text(json.dumps(data, indent=2))

    # Update refs
    proj.head = snap.hash
    store.update_ref("HEAD", snap.hash)
    store.update_ref(f"heads/{proj.branch}", snap.hash)
    store.set_index(proj)

    base_msg = f" ({merge_base[:8]})" if merge_base else ""
    print(f"🔀 Merged '{args.branch}' into '{proj.branch}'")
    print(f"   Merge commit: {snap.short_hash()}")
    print(f"   Base{base_msg} | Parents: {current_head[:8]} + {merge_head[:8]}")


def cmd_remote(args: argparse.Namespace) -> None:
    """Manage remote clavus servers."""
    from clavus.store import BlobStore
    store = BlobStore()
    proj = None

    # Try to load project context, but don't fail if none exists
    try:
        from clavus.helpers import get_store_and_project
        # Suppress stdout to avoid 'no project found' noise
        import io
        old_stdout = sys.stdout
        sys.stdout = io.StringIO()
        try:
            _, proj = get_store_and_project()
        except SystemExit:
            proj = None
        finally:
            sys.stdout = old_stdout
    except Exception:
        proj = None

    remotes = load_remotes(store)

    # ── remote projects <name> — list projects on a remote ──
    if args.action == "projects":
        remote_name = args.name
        if not remote_name:
            if len(remotes) == 1:
                remote_name = remotes[0].name
            else:
                # Try each remote, use the first that responds
                for r in remotes:
                    try:
                        from clavus.sync import SyncClient
                        client = SyncClient(r.url)
                        resp = client.client.get(f"{r.url}/api/projects", timeout=5)
                        client.close()
                        if resp.status_code == 200:
                            remote_name = r.name
                            print(f"📡 Using '{remote_name}' ({r.url})")
                            break
                    except Exception:
                        continue
                if not remote_name:
                    print("❌ Specify a remote name: clavus remote projects <name>")
                    if remotes:
                        print(f"   Available: {', '.join(r.name for r in remotes)}")
                    return
        match = next((r for r in remotes if r.name == remote_name), None)
        if not match:
            print(f"❌ Remote '{remote_name}' not found.")
            return

        from clavus.sync import SyncClient
        client = SyncClient(match.url)
        try:
            r = client.client.get(f"{match.url}/api/projects", timeout=10)
            if r.status_code != 200:
                print(f"❌ Could not reach remote '{remote_name}' at {match.url}")
                return
            data = r.json()
            projects = data.get("projects", [])
            if not projects:
                print(f"📡 No projects on '{remote_name}'")
                return
            print(f"📡 Projects on '{remote_name}' ({match.url}):")
            print()
            for p in projects:
                head_str = f" @ {p.get('head', '?')}" if p.get("head") else " (no snapshots)"
                print(f"  {p.get('name', '?'):<30} branch: {p.get('branch', 'main')}{head_str}")
        finally:
            client.close()
        return

    # ── remote pull <name> [project] — pull a project from remote ──
    if args.action == "pull":
        from clavus.sync import SyncClient, pull_from_remote, pull_snapshot_blobs

        remote_name = args.name
        remote_project = args.url  # Reusing url as optional project name

        if not remote_name:
            if len(remotes) == 1:
                remote_name = remotes[0].name
            else:
                # Try each remote, use the first that responds with projects
                for r in remotes:
                    try:
                        client = SyncClient(r.url)
                        resp = client.client.get(f"{r.url}/api/projects", timeout=5)
                        client.close()
                        if resp.status_code == 200 and resp.json().get("projects"):
                            remote_name = r.name
                            print(f"📡 Using '{remote_name}' ({r.url})")
                            break
                    except Exception:
                        continue
                if not remote_name:
                    print("❌ Specify a remote name: clavus remote pull <name> [project]")
                    if remotes:
                        print(f"   Available: {', '.join(r.name for r in remotes)}")
                    return

        match = next((r for r in remotes if r.name == remote_name), None)
        if not match:
            print(f"❌ Remote '{remote_name}' not found.")
            return

        client = SyncClient(match.url)

        # If no project specified, list available and pick first
        if not remote_project:
            try:
                r = client.client.get(f"{match.url}/api/projects", timeout=10)
                if r.status_code == 200:
                    projects = r.json().get("projects", [])
                    if not projects:
                        print(f"📡 No projects on '{remote_name}'")
                        return
                    if len(projects) == 1:
                        remote_project = projects[0]["name"]
                    else:
                        print(f"📡 Multiple projects on '{remote_name}':")
                        for p in projects:
                            print(f"  {p.get('name', '?')}")
                        print()
                        print(f"  Specify one: clavus remote pull {remote_name} <name>")
                        return
            except Exception:
                pass

        if not remote_project:
            print("❌ No project specified and couldn't auto-detect.")
            return

        print(f"📥 Pulling project '{remote_project}' from '{remote_name}'...")

        # Check if we already have this project locally
        existing = store.get_index(remote_project)

        if not existing:
            # Get project info from remote to init locally
            try:
                r = client.client.get(
                    f"{match.url}/api/sync/pull",
                    params={"name": remote_project},
                    timeout=30,
                )
                if r.status_code != 200:
                    print(f"❌ Project '{remote_project}' not found on remote.")
                    return
                data = r.json()
                remote_info = data.get("project", {})

                # Auto-init a local project entry
                from clavus.store import ClavusProject
                new_proj = ClavusProject(
                    name=remote_project,
                    root_als=remote_info.get("root_als", f"~/{remote_project}/{remote_project}.als"),
                    created_at=time.time(),
                )
                store.set_index(new_proj)
                print(f"   Created local project '{remote_project}'")

                # Switch to the new project
                proj = new_proj
            except Exception as e:
                print(f"❌ Failed to get remote project info: {e}")
                return
        else:
            # Switch to existing project
            store.set_index(existing)
            proj = existing

        print(f"   Syncing data...")

        # Pull cues + snapshots + blobs
        remote_ref = Remote(name=match.name, url=match.url)
        result = pull_from_remote(store, proj, remote_ref)
        parts = []
        if result.get("cues"):
            parts.append(f"{result['cues']} cues")
        if result.get("snapshots"):
            parts.append(f"{result['snapshots']} snapshots")

        blob_count, failed = pull_snapshot_blobs(store, proj, remote_ref)
        if blob_count:
            parts.append(f"{blob_count} blob(s)")
        if failed:
            parts.append(f"{len(failed)} blob(s) failed")

        if parts:
            print(f"   Got {', '.join(parts)}")
        else:
            print(f"   Already up to date")

        print(f"✅ Synced '{remote_project}' from '{remote_name}'")
        print(f"   Switch to it: clavus project '{remote_project}'")
        print(f"   Pull again:   clavus remote pull {remote_name} {remote_project}")
        client.close()
        return

    # ── remote rename <old> <new> ──
    if args.action == "rename" or (args.name and args.url and not args.action):
        old_name = args.name
        new_name = args.url
        if not old_name or not new_name:
            print("❌ Usage: clavus remote rename <old-name> <new-name>")
            return
        match = next((r for r in remotes if r.name == old_name), None)
        if not match:
            print(f"❌ Remote '{old_name}' not found.")
            return
        if any(r.name == new_name for r in remotes):
            print(f"❌ Remote '{new_name}' already exists.")
            return
        match.name = new_name
        save_remotes(store, remotes)
        print(f"✏️ Renamed remote '{old_name}' → '{new_name}'")
        return

    # ── Existing behavior: add / remove / list ──
    name = args.add or (args.name if args.action == "add" else "")
    remove_name = args.remove or (args.name if args.action == "remove" else "")

    if name:
        # Add a remote
        for r in remotes:
            if r.name == name:
                print(f"⚠️  Remote '{name}' already exists")
                return
        url = args.url or f"http://{name}.local:7890"
        remotes.append(Remote(name=name, url=url))
        save_remotes(store, remotes)
        print(f"🌐 Added remote '{name}' → {url}")
        return

    if remove_name:
        remotes = [r for r in remotes if r.name != remove_name]
        save_remotes(store, remotes)
        print(f"🗑 Removed remote '{remove_name}'")
        return

    # ── rename <old_name> <new_name> ──
    if args.action == "rename":
        old_name = args.name
        new_name = args.url  # argparse puts 3rd positional into url
        if not old_name or not new_name:
            print("❌ Usage: clavus remote rename <old_name> <new_name>")
            return
        match = next((r for r in remotes if r.name.lower() == old_name.lower()), None)
        if not match:
            print(f"❌ Remote '{old_name}' not found.")
            if remotes:
                print(f"   Available: {', '.join(r.name for r in remotes)}")
            return
        old = match.name
        if any(r.name.lower() == new_name.lower() and r.name != old for r in remotes):
            print(f"❌ Remote '{new_name}' already exists.")
            return
        match.name = new_name
        save_remotes(store, remotes)
        print(f"✏️  Renamed '{old}' → '{new_name}'")
        return

    # List remotes
    if not remotes:
        print(f"📡 No remotes configured.")
        print(f"   Use 'clavus remote add <name> <url>' to add one.")
        return

    label = proj.name if proj else "this machine"
    print(f"📡 Remotes for {label}")
    print()
    for r in remotes:
        last_sync = time.strftime("%m/%d %H:%M", time.localtime(r.last_sync)) if r.last_sync else "never"
        print(f"  {r.name:<20} {r.url}")
        print(f"  {'':<20} last sync: {last_sync}")
        print()


def cmd_find(args: argparse.Namespace) -> None:
    """Discover Clavus servers on the LAN or Tailscale tailnet."""
    peers = []

    if args.tailscale:
        try:
            from clavus.discovery import discover_tailscale_peers
            print(f"[SCAN] Scanning your Tailscale tailnet for Clavus servers ({args.timeout}s)...")
            peers = discover_tailscale_peers(timeout=args.timeout)
            if not peers:
                print()
                print("  No Clavus servers found on Tailscale.")
                print()
                print("  Make sure you're connected to Tailscale and your friends")
                print("  are running 'clavus relay'.")
                return
        except ImportError:
            peers = []
    else:
        try:
            from clavus.discovery import discover_peers
        except ImportError:
            print("❌ LAN discovery requires zeroconf. Install: pip install zeroconf")
            return

        print(f"[SCAN] Scanning for Clavus servers on LAN ({args.timeout}s)...")
        peers = discover_peers(timeout=args.timeout)

    if not peers:
        if args.tailscale:
            print()
            print("  No Clavus servers found on Tailscale.")
            print()
            print("  Make sure you're connected to Tailscale and your friends")
            print("  are running 'clavus relay'.")
        else:
            print("  No Clavus servers found.")
            print()
            print("  Make sure you or a friend is running 'clavus relay'.")
            print("  Clavus advertises via mDNS (Bonjour) on the local network.")
            print("  Both machines must be on the same subnet.")
        return

    print(f"  Found {len(peers)} Clavus server(s):")
    print()
    for peer in peers:
        host = peer.host or "?"
        port = peer.port or 7890
        proj = peer.project or "unknown project"
        user = f" [{peer.user}]" if peer.user else ""
        print(f"  {peer.name:<20} {host:>15}:{port:<5}  {proj}{user}")

    print()
    print("  To connect:")
    for peer in peers:
        print(f"    clavus remote add {peer.name} http://{peer.host}:{peer.port}")
    print()
    print("    Or launch the TUI pointing at one:")
    for peer in peers:
        print(f"    clavus tui --connect http://{peer.host}:{peer.port}")

    # Auto-pair if requested
    if args.pair:
        matches = [p for p in peers if p.name.lower() == args.pair.lower()]
        if not matches:
            print(f"  ❌ No server found with hostname '{args.pair}'")
            return
        peer = matches[0]
        from clavus.store import BlobStore
        from clavus.sync import save_remotes, Remote, load_remotes
        store = BlobStore()
        remotes = load_remotes(store)
        for r in remotes:
            if r.name == peer.name:
                print(f"  ⚠️  Remote '{peer.name}' already exists")
                return
        remotes.append(Remote(name=peer.name, url=peer.url))
        save_remotes(store, remotes)
        print(f"  ✅ Paired with '{peer.name}' → {peer.url}")
    print()

def cmd_push(args: argparse.Namespace) -> None:
    """Push cues and snapshots to remotes."""
    from clavus.progress import Spinner, ProgressBar, status

    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        print(f"   Use 'clavus remote add <name> <url>' first.")
        return

    # If a remote name is specified, only push to that one
    if args.remote:
        remotes = [r for r in remotes if r.name == args.remote]
        if not remotes:
            print(f"❌ Remote '{args.remote}' not found.")
            return

    force = getattr(args, 'force', False)
    label = "force pushing" if force else "pushing"

    # ── Parallel pre-flight: ping all remotes at once, skip dead ones ──
    from concurrent.futures import ThreadPoolExecutor, as_completed
    from clavus.sync import SyncClient

    live_remotes = []
    dead_remotes = []
    with Spinner(f"pinging {len(remotes)} remote(s)..."):
        with ThreadPoolExecutor(max_workers=len(remotes)) as executor:
            future_to_remote = {
                executor.submit(SyncClient(r.url).fast_ping): r
                for r in remotes
            }
            for future in as_completed(future_to_remote):
                remote = future_to_remote[future]
                try:
                    if future.result():
                        live_remotes.append(remote)
                    else:
                        dead_remotes.append(remote)
                except Exception:
                    dead_remotes.append(remote)

    for remote in dead_remotes:
        print(f"  ⏭  Skipping '{remote.name}' — unreachable")

    for remote in live_remotes:
        with Spinner(f"{label} to '{remote.name}'..."):
            result = push_to_remote(store, proj, remote, force=force)

        if result.get("error"):
            print(f"  ❌ {result['error']}")
        else:
            parts = [f"✅ {result.get('cues', 0)} cues, {result.get('snapshots', 0)} snapshots"]

            # Push snapshot content blobs + .als backups
            with Spinner("syncing blobs..."):
                from clavus.sync import push_snapshot_blobs
                blob_count = push_snapshot_blobs(store, proj, remote)
                if blob_count:
                    parts.append(f"{blob_count} blob{'s' if blob_count != 1 else ''}")

            # Push stems for current HEAD
            head = store.read_ref("HEAD")
            stem_store = StemStore(proj.name, store)
            if head and stem_store.get_manifest(head):
                with Spinner("syncing stems..."):
                    from clavus.sync import push_stems_to_remote
                    stem_count = push_stems_to_remote(store, proj, remote, stem_store, head)
                    parts.append(f"{stem_count} stem{'s' if stem_count != 1 else ''}")

            status(f"  {' — '.join(parts)}")


def cmd_pull(args: argparse.Namespace) -> None:
    """Pull cues and snapshots from remotes."""
    from clavus.store import BlobStore
    store = BlobStore()
    proj = None

    # Try to load project, but don't fail if none exists
    try:
        from clavus.helpers import get_store_and_project
        import io
        old_stdout = sys.stdout
        sys.stdout = io.StringIO()
        try:
            _, proj = get_store_and_project()
        except SystemExit:
            proj = None
        finally:
            sys.stdout = old_stdout
    except Exception:
        proj = None

    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        print(f"   Add one with: clavus remote add <name> <url>")
        return

    # If no local project, pull from any remote that has projects
    from clavus.progress import Spinner, ProgressBar, status

    if not proj:
        from clavus.sync import SyncClient, pull_from_remote, pull_snapshot_blobs
        from clavus.store import ClavusProject

        # args.remote can be a remote name OR a project name — figure out which
        candidates = remotes
        target_project = None
        if args.remote:
            # Check if it matches a remote name
            matching_remotes = [r for r in remotes if r.name == args.remote]
            if matching_remotes:
                candidates = matching_remotes
            else:
                # Not a remote name — treat as project name filter
                target_project = args.remote
                # Still need a remote to pull from
                if len(remotes) == 0:
                    print(f"❌ No remotes configured.")
                    return
                # Try all remotes, pick the first with this project
                for r in remotes:
                    try:
                        client = SyncClient(r.url)
                        resp = client.client.get(f"{r.url}/api/projects", timeout=5)
                        client.close()
                        if resp.status_code == 200:
                            proj_names = [p["name"] for p in resp.json().get("projects", [])]
                            if target_project in proj_names:
                                candidates = [r]
                                break
                    except Exception:
                        continue
                if len(candidates) == len(remotes):
                    print(f"❌ Project '{target_project}' not found on any remote.")
                    return

        pulled_any = False
        # Try external remotes first (localhost is unreachable on Windows)
        candidates_sorted = sorted(candidates, key=lambda r: 0 if "localhost" in r.url else -1)
        for remote in candidates_sorted:
            client = SyncClient(remote.url)
            try:
                r = client.client.get(f"{remote.url}/api/projects", timeout=10)
                if r.status_code != 200:
                    continue
                projects = r.json().get("projects", [])
                if not projects:
                    continue

                for pdata in projects:
                    pname = pdata["name"]
                    # Skip if target_project is set and doesn't match
                    if target_project and pname != target_project:
                        continue
                    # Skip if already have it
                    if store.get_index(pname):
                        store.set_index(store.get_index(pname))
                        proj = store.get_index(pname)
                    else:
                        # Init locally
                        print(f"📥 Pulling '{pname}' from '{remote.name}'...")
                        r2 = client.client.get(
                            f"{remote.url}/api/sync/pull",
                            params={"name": pname},
                            timeout=30,
                        )
                        if r2.status_code != 200:
                            continue
                        data = r2.json()
                        info = data.get("project", {})
                        new_proj = ClavusProject(
                            name=pname,
                            root_als=info.get("root_als", f"~/{pname}/{pname}.als"),
                            created_at=time.time(),
                        )
                        store.set_index(new_proj)
                        proj = new_proj
                        print(f"   Created local project '{pname}'")

                    # Pull data
                    remote_ref = Remote(name=remote.name, url=remote.url)
                    result = pull_from_remote(store, proj, remote_ref)
                    blob_count, failed = pull_snapshot_blobs(store, proj, remote_ref)
                    parts = []
                    if result.get("cues"):
                        parts.append(f"{result['cues']} cues")
                    if result.get("snapshots"):
                        parts.append(f"{result['snapshots']} snapshots")
                    if blob_count:
                        parts.append(f"{blob_count} blob(s)")
                    if failed:
                        parts.append(f"{len(failed)} blob(s) failed")
                    if parts:
                        print(f"   Got {', '.join(parts)}")
                    else:
                        print(f"   Already up to date")
                    pulled_any = True

                if pulled_any:
                    break  # Got what we needed from first responsive remote
            except Exception:
                continue
            finally:
                client.close()

        if not pulled_any:
            print("❌ No projects found on any remote.")
            print("   Make sure the relay is running: clavus relay")
        return

    if not proj:
        return

    # If a remote name is specified, only pull from that one
    if args.remote:
        remotes = [r for r in remotes if r.name == args.remote]
        if not remotes:
            print(f"❌ Remote '{args.remote}' not found.")
            return

    # Deduplicate remotes by URL (same relay shouldn't be pulled twice)
    seen_urls = set()
    unique_remotes = []
    for r in remotes:
        if r.url not in seen_urls:
            seen_urls.add(r.url)
            unique_remotes.append(r)
    remotes = unique_remotes

    # ── Parallel pre-flight: ping all remotes at once, skip dead ones ──
    from concurrent.futures import ThreadPoolExecutor, as_completed
    from clavus.sync import SyncClient

    live_remotes = []
    dead_remotes = []
    with Spinner(f"pinging {len(remotes)} remote(s)..."):
        with ThreadPoolExecutor(max_workers=len(remotes)) as executor:
            future_to_remote = {
                executor.submit(SyncClient(r.url).fast_ping): r
                for r in remotes
            }
            for future in as_completed(future_to_remote):
                remote = future_to_remote[future]
                try:
                    if future.result():
                        live_remotes.append(remote)
                    else:
                        dead_remotes.append(remote)
                except Exception:
                    dead_remotes.append(remote)

    for remote in dead_remotes:
        print(f"  ⏭  Skipping '{remote.name}' — unreachable")

    for remote in live_remotes:
        from clavus.sync import pull_from_remote, pull_snapshot_blobs

        # Auto-snapshot local work before overwriting with remote changes
        try:
            from pathlib import Path
            als_path = Path(proj.root_als)
            if als_path.exists() and proj.head:
                raw_als = als_path.read_bytes()
                current_hash = hashlib.sha256(raw_als).hexdigest()
                if current_hash != proj.head:
                    from clavus import parse_als
                    project = parse_als(als_path)
                    if project:
                        snap = store.save_snapshot(
                            project,
                            message="auto-snapshot before sync",
                            parent=proj.head,
                        )
                        if snap.hash != proj.head:
                            store.update_ref("HEAD", snap.hash)
                            proj.head = snap.hash
                            store.set_index(proj)
                            print(f"📸 Auto-snapshot {snap.hash[:8]} (local changes saved)")
        except Exception:
            pass

        with Spinner(f"pulling from '{remote.name}'..."):
            result = pull_from_remote(store, proj, remote, output_dir=args.output)
        parts = []
        if result.get("error"):
            parts = [f"❌ {result['error']}"]
        else:
            parts = [f"✅ {result['cues']} cues, {result['snapshots']} snapshots"]

            # Pull snapshot content blobs + .als backups
            with Spinner("syncing blobs..."):
                from clavus.sync import pull_snapshot_blobs
                blob_count, failed = pull_snapshot_blobs(store, proj, remote)
                if blob_count:
                    parts.append(f"{blob_count} blob{'s' if blob_count != 1 else ''}")
                if failed:
                    parts.append(f"{len(failed)} blob(s) failed")

            # Pull stems for current HEAD
            head = store.read_ref("HEAD")
            if head:
                with Spinner("syncing stems..."):
                    from clavus.sync import pull_stems_from_remote
                    stem_count = pull_stems_from_remote(store, proj, remote)
                    if stem_count:
                        parts.append(f"{stem_count} stem{'s' if stem_count != 1 else ''}")
                        # Materialize after pull
                        stem_store = StemStore(proj.name, store)
                        manifest = stem_store.get_manifest(head)
                        if manifest and manifest.stems:
                            paths = stem_store.materialize_stems(head)
                            parts.append(f"materialized {len(paths)} file{'s' if len(paths) != 1 else ''}")
        status(f"  {' — '.join(parts)}")


def cmd_pull_all(args: argparse.Namespace) -> None:
    """Pull ALL projects from the active remote."""
    from clavus.progress import Spinner, status

    from clavus.store import BlobStore as _BS
    store = _BS()
    proj = None

    # Try to load current project, but don't fail if none exists
    try:
        from clavus.helpers import get_store_and_project
        import io
        old_stdout = sys.stdout
        sys.stdout = io.StringIO()
        try:
            _, proj = get_store_and_project()
        except SystemExit:
            proj = None
        finally:
            sys.stdout = old_stdout
    except Exception:
        proj = None

    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        print(f"   Add one with: clavus remote add <name> <url>")
        return

    from clavus.sync import SyncClient, pull_from_remote, pull_snapshot_blobs

    # Find the active remote (or first one)
    active_remote = None
    if proj and hasattr(proj, 'active_remote') and proj.active_remote:
        active_remote = next((r for r in remotes if r.name == proj.active_remote), None)
    if not active_remote:
        active_remote = remotes[0] if remotes else None
    if not active_remote:
        print("❌ No remote to pull from.")
        return

    with Spinner(f"fetching projects from '{active_remote.name}'..."):
        from clavus.sync import SyncClient as _SC
        _probe = _SC(active_remote.url)
        try:
            resp = _probe.client.get(f"{active_remote.url}/api/projects", timeout=10)
            if resp.status_code != 200:
                print(f"❌ Remote '{active_remote.name}' unreachable.")
                return
            remote_projects = resp.json().get("projects", [])
        finally:
            _probe.close()

    pulled = 0
    skipped = 0
    for pdata in remote_projects:
        pname = pdata["name"]
        existing = store.get_index(pname)
        if existing:
            skipped += 1
            continue
        with Spinner(f"pulling '{pname}'..."):
            from clavus.store import ClavusProject
            _pull_client = SyncClient(active_remote.url)
            try:
                resp = _pull_client.client.get(
                    f"{active_remote.url}/api/sync/pull",
                    params={"name": pname},
                    timeout=30,
                )
                if resp.status_code != 200:
                    continue
                data = resp.json()
                info = data.get("project", {})
                new_proj = ClavusProject(
                    name=pname,
                    root_als=info.get("root_als", f"~/{pname}/{pname}.als"),
                    created_at=time.time(),
                )
                store.set_index(new_proj)
                result = pull_from_remote(store, new_proj, active_remote)
                pull_snapshot_blobs(store, new_proj, active_remote)
            finally:
                _pull_client.close()
        pulled += 1
        status(f"  ✅ '{pname}' — {result.get('snapshots', 0)} snapshots")

    print(f"  Done: {pulled} pulled, {skipped} already local")


def cmd_sync(args: argparse.Namespace) -> None:
    """Start the auto-sync daemon."""
    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        print(f"❌ No remotes configured.")
        print(f"   Use 'clavus remote add <name> <url>' first.")
        print(f"   Then 'clavus push' to sync manually, or 'clavus sync' for auto.")
        return

    daemon = SyncDaemon(store, proj, interval=args.interval)
    daemon.start()

    print()
    print(f"  Press Ctrl+C to stop.")
    print()
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        daemon.stop()


def cmd_cue_render(args: argparse.Namespace) -> None:
    """Export unresolved cues as Ableton markers — either to file or injected into the .als."""
    store, proj = get_store_and_project()
    cues = CueStore(proj.name, store=store)

    unresolved = cues.list_cues(CueFilter(status="pending"))
    if not unresolved:
        print("💬 No unresolved cues to render.")
        return

    if args.inject:
        # Inject directly into the project's .als file
        from clavus.progress import Spinner, status
        with Spinner("injecting cues into .als..."):
            render_cues_as_markers(unresolved, "", inject_into_als=proj.root_als)
        status(f"[INJECT] Injected {len(unresolved)} cues into {proj.root_als}")
        print(f"   Auto-snapshot to save: clavus snapshot \"injected markers\" or use :snapshot in TUI")
    else:
        output = args.output or f"{proj.name}_cues.xml"
        render_cues_as_markers(unresolved, output)
        print(f"📍 Rendered {len(unresolved)} cues to {output}")
        print(f"   Import into Ableton by merging <CuePoints> into your .als file.")


# ─── Snapshot Restore ─────────────────────────────────────────────────────

def cmd_restore(args: argparse.Namespace) -> None:
    """Restore a snapshot's .als file from the stored raw bytes."""
    store, proj = get_store_and_project()

    if args.hash:
        hash_str = resolve_snapshot(store, args.hash)
        if not hash_str:
            print(f"❌ Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = store.read_ref("HEAD")
        if not hash_str:
            print("❌ No snapshots to restore from.")
            sys.exit(1)

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"❌ Snapshot not found: {hash_str}")
        sys.exit(1)

    if not snap.als_hash:
        print(f"❌ Snapshot {snap.short_hash()} has no raw .als backup.")
        print("   Only snapshots created *after* the restore feature was built")
        print("   store raw .als data. Create a fresh snapshot first.")
        sys.exit(1)

    raw_als = store.get_object(snap.als_hash)
    if not raw_als:
        print(f"❌ Raw .als data missing for snapshot {snap.short_hash()}.")
        print(f"   Looked for blob: {snap.als_hash[:16]}...")
        sys.exit(1)

    als_path = Path(proj.root_als)
    if not als_path.exists():
        print(f"❌ Project .als file not found at {als_path}")
        sys.exit(1)

    # Confirmation
    snap_time = time.strftime("%Y-%m-%d %H:%M", time.localtime(snap.timestamp))
    print(f"📋 Restore snapshot: {snap.short_hash()}")
    print(f"   Message: {snap.message}")
    print(f"   Captured: {snap_time}")
    print(f"   Tracks: {snap.track_count}  BPM: {snap.bpm}")
    print(f"   Will overwrite: {als_path}")
    print()
    print(f"⚠️  This will overwrite your current .als file. A backup will be saved as:")
    print(f"   {als_path}.bak (if one doesn't already exist)")
    print()
    if not args.yes:
        confirm = input("  Continue? (y/N): ").strip().lower()
        if confirm != "y":
            print("❌ Restore cancelled.")
            return

    # Backup existing .als (only first time)
    bak_path = als_path.with_suffix(".als.bak")
    if not bak_path.exists():
        bak_path.write_bytes(als_path.read_bytes())
        print(f"   💾 Current .als backed up to: {bak_path}")

    # Write the restored .als
    als_path.write_bytes(raw_als)
    print(f"✅ Restored {als_path.name} to snapshot {snap.short_hash()}")
    print(f"   '{snap.message}' — from {snap_time}")

    # Update HEAD to point at this snapshot
    store.update_ref("HEAD", hash_str)
    proj.head = hash_str
    store.set_index(proj)
    print(f"   HEAD updated to {snap.short_hash()}")


def cmd_open(args: argparse.Namespace) -> None:
    """Open the latest (or specified) .als snapshot in Ableton Live."""
    import subprocess as sp
    import platform

    store, proj = get_store_and_project()

    # Resolve hash
    if args.hash:
        hash_str = resolve_snapshot(store, args.hash)
        if not hash_str:
            print(f"❌ Could not resolve '{args.hash}'")
            sys.exit(1)
    else:
        hash_str = proj.head
        if not hash_str:
            print("❌ No snapshots. Push from another machine or take a snapshot first.")
            sys.exit(1)

    snap = store.load_snapshot(hash_str)
    if not snap:
        print(f"❌ Snapshot not found: {hash_str}")
        sys.exit(1)

    if not snap.als_hash:
        print(f"❌ Snapshot {snap.short_hash()} has no .als backup.")
        sys.exit(1)

    raw_als = store.get_object(snap.als_hash)
    if not raw_als:
        print(f"❌ .als blob missing. Try pulling again to fetch it.")
        sys.exit(1)

    # Determine output path — Ableton project folder convention:
    # "Song.als" must live inside "Song Project/" or Ableton auto-creates
    # a copy there, making Clavus's root_als stale.
    project_name = proj.name.replace(" ", " ")
    if args.output:
        out_path = Path(args.output)
    else:
        project_dir = get_projects_dir() / project_name
        als_project = project_dir / f"{project_name} Project"
        out_path = als_project / f"{project_name}.als"

    out_path.parent.mkdir(parents=True, exist_ok=True)
    # Create Ableton project folder scaffolding so Ableton recognizes
    # this as a valid project folder and doesn't auto-create nested copies
    (out_path.parent / "Samples").mkdir(exist_ok=True)
    (out_path.parent / "Backup").mkdir(exist_ok=True)
    (out_path.parent / "Ableton Project Info").mkdir(exist_ok=True)

    # Materialize audio samples into the project folder first (so they exist)
    sample_written = 0
    if snap.sample_hashes:
        # Extract filename → RelativePath mapping from the original .als
        import gzip as _gzip, re as _re
        _xml = _gzip.decompress(raw_als).decode("utf-8", errors="replace")
        _als_relpaths: dict[str, str] = {}
        for _m in _re.finditer(r'<RelativePath\s+Value="([^"]+)"', _xml):
            _rp = _m.group(1)
            _fn = _rp.rsplit("/", 1)[-1].rsplit("\\", 1)[-1]
            if _fn not in _als_relpaths:
                _als_relpaths[_fn] = _rp

        base_dir = out_path.parent  # Project folder root
        for sh in snap.sample_hashes:
            fname = store.get_sample_filename(sh)
            relpath = _als_relpaths.get(fname) or store.get_sample_relpath(sh) or ""
            if fname and store.has_object(sh):
                try:
                    store.materialize_sample(sh, base_dir, fname, relpath)
                    sample_written += 1
                except Exception:
                    pass

    # NOTE: Path rewriting DISABLED — it corrupts .als XML on Windows.
    # Write the raw blob as-is. Samples are materialized alongside; Ableton
    # finds them once you point at any one sample in the project folder.
    # from clavus.parser import rewrite_als_sample_paths
    # raw_als = rewrite_als_sample_paths(raw_als, out_path.parent)

    out_path.write_bytes(raw_als)
    print(f"✅ {project_name}.als → {out_path}")
    print(f"   Snapshot: {snap.short_hash()} — {snap.message or '(no message)'}")
    print(f"   {snap.track_count} tracks, {snap.bpm} BPM")
    if sample_written:
        print(f"   🎵 {sample_written} audio sample{'s' if sample_written != 1 else ''} → {base_dir}")

    # Launch Ableton
    system = platform.system()
    if system == "Darwin":
        ableton_path = None
        for candidate in Path("/Applications").glob("Ableton Live*.app"):
            ableton_path = str(candidate)
            break
        if ableton_path:
            sp.run(["open", "-a", ableton_path, str(out_path)])
            print(f"   🎹 Launched Ableton Live")
        else:
            sp.run(["open", str(out_path)])
            print(f"   🎹 Opened .als with default app")
    elif system == "Windows":
        sp.run(["start", "", str(out_path)], shell=True)
        print(f"   🎹 Opened .als with default app")
    else:
        print(f"   ℹ️  Open manually: {out_path}")

    # Helpful tip for first-open sample resolution
    if sample_written:
        print()
        print("   💡 If samples show as offline, point Ableton at any one —")
        print("      all others will resolve automatically.")


# ─── Backup / Restore Store ──────────────────────────────────────────────


def cmd_backup(args: argparse.Namespace) -> None:
    """Backup the entire Clavus store (cues, snapshots, refs, config)."""
    from clavus.progress import Spinner
    from clavus.store import BlobStore
    store = BlobStore()
    with Spinner("creating backup..."):
        archive_path = store.backup_store()
    size_mb = archive_path.stat().st_size / (1024 * 1024)
    print(f"💾 Backup saved: {archive_path}")
    print(f"   Size: {size_mb:.1f} MB")
    print(f"   To restore: clavus restore --store {archive_path}")


def cmd_list_backups(args: argparse.Namespace) -> None:
    """List available store backups."""
    from clavus.store import BlobStore
    store = BlobStore()
    backups = store.list_backups()
    if not backups:
        print("  No backups found.")
        print("  Create one with: clavus backup")
        return
    print(f"  📦 Available backups:")
    for b in backups:
        size_kb = b.stat().st_size / 1024
        print(f"     {b.name}  ({size_kb:.0f} KB)")


def cmd_restore_store(args: argparse.Namespace) -> None:
    """Restore Clavus store from a backup archive."""
    from clavus.store import BlobStore
    store = BlobStore()
    if not args.archive:
        backups = store.list_backups()
        if not backups:
            print("❌ No backups found. Run 'clavus backup' first.")
            return
        archive_path = backups[0]
        print(f"  Using latest backup: {archive_path.name}")
    else:
        archive_path = Path(args.archive)
    store.restore_store(archive_path)


# ─── Stem Commands ─────────────────────────────────────────────────────


def cmd_stem_import(args: argparse.Namespace) -> None:
    """Import a bounced stem file and register it with the current snapshot."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    head = store.read_ref("HEAD")

    if not head:
        print("❌ No snapshot yet. Run 'clavus snapshot' first.")
        return

    entry = stem_store.store_stem_file(args.file, args.track)
    manifest = stem_store.get_manifest(head) or StemManifest(snapshot_hash=head)

    # Check if this track already has a stem — replace it
    manifest.stems = [s for s in manifest.stems if s.track_name != args.track]
    manifest.stems.append(entry)
    manifest.created_at = time.time()
    stem_store.save_manifest(manifest)

    file_size_mb = entry.size / (1024 * 1024)
    print(f"📦 Imported stem: {entry.track_name} ({entry.file_name})")
    print(f"   Hash:   {entry.hash[:12]}")
    print(f"   Size:   {file_size_mb:.1f} MB")
    print(f"   Format: {entry.format} / {entry.sample_rate}Hz / {entry.bit_depth}bit / {entry.channels}ch")
    if entry.duration_seconds:
        print(f"   Length: {entry.duration_seconds:.1f}s")
    print(f"   Snapshot: {head[:12]}")


def cmd_stem_import_folder(args: argparse.Namespace) -> None:
    """Import all WAV files from a folder as stems. Derives track names from filenames."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    head = store.read_ref("HEAD")

    if not head:
        print("❌ No snapshot yet. Run 'clavus snapshot' first.")
        return

    folder = Path(args.folder).expanduser().resolve()
    if not folder.is_dir():
        print(f"❌ Not a directory: {folder}")
        return

    wavs = sorted(set(folder.glob("*.wav")) | set(folder.glob("*.WAV")))
    if not wavs:
        print(f"❌ No .wav files found in {folder}")
        return

    manifest = stem_store.get_manifest(head) or StemManifest(snapshot_hash=head)
    imported = 0
    skipped = 0
    total_size = 0

    print(f"\n  Importing {len(wavs)} stem(s) from {folder}/ ...\n")
    for wav_path in wavs:
        track_name = args.prefix + wav_path.stem if args.prefix else wav_path.stem
        try:
            entry = stem_store.store_stem_file(str(wav_path), track_name)
        except Exception as e:
            print(f"  ✗ {wav_path.name}: {e}")
            skipped += 1
            continue

        # Replace existing stem for this track if present
        manifest.stems = [s for s in manifest.stems if s.track_name != track_name]
        manifest.stems.append(entry)
        imported += 1
        total_size += entry.size

    if imported == 0:
        print("  No stems imported.")
        return

    manifest.created_at = time.time()
    stem_store.save_manifest(manifest)

    total_mb = total_size / (1024 * 1024)
    print(f"\n  ✅ Imported {imported} stem(s)")
    if skipped:
        print(f"  ⚠  Skipped {skipped}")
    print(f"  Total: {total_mb:.1f} MB across {len(manifest.stems)} stems")
    print(f"  Snapshot: {head[:12]}")
    print(f"\n  Ready to push: clavus stem push")


def cmd_stem_list(args: argparse.Namespace) -> None:
    """List stems for the current (or specified) snapshot."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    head = args.snapshot or store.read_ref("HEAD")

    if not head:
        print("❌ No snapshot to show stems for.")
        return

    manifest = stem_store.get_manifest(head)
    if not manifest or not manifest.stems:
        print(f"No stems registered for snapshot {head[:12]}")
        return

    print(f"Stems for snapshot {head[:12]}:\n")
    total_size = 0
    for entry in manifest.stems:
        size_mb = entry.size / (1024 * 1024)
        total_size += entry.size
        duration = f"{entry.duration_seconds:.1f}s" if entry.duration_seconds else "?"
        print(f"  {entry.track_name:20s}  {entry.file_name:25s}  {size_mb:6.1f} MB  {duration}  {entry.hash[:12]}")
    print(f"\n  Total: {total_size / (1024 * 1024):.1f} MB across {len(manifest.stems)} stems")


def cmd_stem_pull(args: argparse.Namespace) -> None:
    """Pull stem files from remotes (materialize working tree)."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    remotes = load_remotes(store)

    if not remotes:
        print("❌ No remotes configured. Use 'clavus remote add' first.")
        return

    head = store.read_ref("HEAD")
    if not head:
        print("❌ No snapshot yet.")
        return

    # Fetch stems from remotes via the sync extension
    from clavus.sync import pull_stems_from_remote
    total = 0
    for remote in remotes:
        print(f"  Pulling stems from '{remote.name}'...")
        count = pull_stems_from_remote(store, proj, remote)
        total += count
        if count:
            print(f"    Downloaded {count} stem(s)")
        else:
            print(f"    No new stems")

    # Materialize working tree
    manifest = stem_store.get_manifest(head)
    if manifest and manifest.stems:
        paths = stem_store.materialize_stems(head)
        print(f"\n  Materialized {len(paths)} stem(s) to ~/.clavus/stems/{proj.name}/{head[:12]}/")
    else:
        print(f"\n  No stem manifest for current snapshot")


def cmd_stem_push(args: argparse.Namespace) -> None:
    """Push stem files to all remotes."""
    store, proj = get_store_and_project()
    stem_store = StemStore(proj.name, store)
    remotes = load_remotes(store)

    if not remotes:
        print("❌ No remotes configured.")
        return

    head = store.read_ref("HEAD")
    if not head:
        print("❌ No snapshot yet.")
        return

    manifest = stem_store.get_manifest(head)
    if not manifest or not manifest.stems:
        print("No stems to push for current snapshot.")
        return

    from clavus.sync import push_stems_to_remote, SyncClient
    for remote in remotes:
        print(f"  Pushing to '{remote.name}'...")
        try:
            client = SyncClient(remote.url)
            if not client.fast_ping(timeout=3.0):
                print(f"    ⏭  Skipping — unreachable")
                client.close()
                continue
            count = push_stems_to_remote(store, proj, remote, stem_store, head)
            print(f"    Pushed {count} stem(s)")
        except Exception as e:
            print(f"    ⏭  Skipping — {e}")


# ─── Main Entry Point ──────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="Clavus — snapshot, sync, and collaborate on Ableton Live projects.",
        prog="clavus",
    )
    parser.add_argument("--clavus-dir", help="Override clavus storage directory")
    parser.add_argument("--version", action="store_true", help="Show version and exit")
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Init
    p_init = subparsers.add_parser("init", help="Initialize a new clavus project")
    p_init.add_argument("path", nargs="?", default=None, help="Path to .als file or project directory")

    # Projects (list)
    subparsers.add_parser("projects", help="List all tracked projects")

    # Project (switch active)
    p_project = subparsers.add_parser("project", help="Switch active project, or toggle sharing")
    p_project.add_argument("name", help="Project name to switch to")
    p_project.add_argument("--share", action="store_true", help="Make project visible to collaborators")
    p_project.add_argument("--private", action="store_true", help="Hide project from collaborators")

    # Snapshot
    p_snap = subparsers.add_parser("snapshot", help="Create a new snapshot")
    p_snap.add_argument("message", nargs="?", default="",
                        help="Description of what changed (prompts if omitted)")
    p_snap.add_argument("--message", "-m", dest="message_flag", default=None,
                        help="Description of what changed (alternative to positional arg)")
    p_snap.add_argument("--tag", "-t", default="", help="Comma-separated tags")
    p_snap.add_argument("--parent", "-p", default=None, help="Override parent snapshot hash")
    p_snap.add_argument("--verbose", "-v", action="store_true", help="Show detailed diff")
    p_snap.add_argument("--allow-frozen", action="store_true", help="Skip frozen track warning (for TUI/non-interactive use)")

    # Log
    p_log = subparsers.add_parser("log", help="Show snapshot history")
    p_log.add_argument("--limit", "-n", type=int, default=20, help="Max snapshots to show")
    p_log.add_argument("--verbose", "-v", action="store_true", help="Show detailed info per snapshot")
    p_log.add_argument("--graph", action="store_true", help="Show branch topology")

    # Restore
    p_restore = subparsers.add_parser("restore", help="Restore .als from a snapshot")
    p_restore.add_argument("hash", nargs="?", default=None,
                          help="Snapshot hash or ref name (default: HEAD)")
    p_restore.add_argument("-y", "--yes", action="store_true",
                          help="Skip confirmation prompt")

    # Open
    p_open = subparsers.add_parser("open", help="Open latest .als in Ableton Live")
    p_open.add_argument("hash", nargs="?", default=None,
                       help="Snapshot hash (default: HEAD/latest)")
    p_open.add_argument("--output", "-o", default=None,
                       help="Output path (default: Desktop/<Project>.als)")

    # Store backup
    p_backup = subparsers.add_parser("backup", help="Backup the entire Clavus store")
    p_list_backups = subparsers.add_parser("backups", help="List available store backups")
    p_restore_store = subparsers.add_parser("restore-store", help="Restore Clavus store from a backup")
    p_restore_store.add_argument("archive", nargs="?", default=None,
                                help="Path to backup archive (default: latest)")

    # Diff
    p_diff = subparsers.add_parser("diff", help="Show changes in a snapshot")
    p_diff.add_argument("hash", nargs="?", default=None, help="Snapshot hash or ref name (default: HEAD)")
    p_diff.add_argument("--verbose", "-v", action="store_true", help="Show unchanged tracks")
    p_diff.add_argument("--visual", action="store_true", help="Show visual timeline diff")

    # Status
    subparsers.add_parser("status", help="Show current project status")

    # Cues
    p_cue = subparsers.add_parser("cue", help="Add a timeline-anchored comment")
    p_cue.add_argument("text", help="The comment text")
    p_cue.add_argument("position", nargs="?", default="0.0.0", help="Position (e.g. @1:23 or 4.1.1)")
    p_cue.add_argument("--track", "-t", default="", help="Which track this cue is about")
    p_cue.add_argument("--author", "-a", default="", help="Override author name")

    p_cue_reply = subparsers.add_parser("cue-reply", help="Reply to a cue thread")
    p_cue_reply.add_argument("cue_id", help="ID of the cue to reply to")
    p_cue_reply.add_argument("text", help="Reply text")

    p_cue_resolve = subparsers.add_parser("cue-resolve", help="Mark a cue as resolved")
    p_cue_resolve.add_argument("cue_id", help="ID of the cue to resolve")
    p_cue_resolve.add_argument("--note", "-n", default="", help="Optional resolution note")

    p_cue_skip = subparsers.add_parser("cue-skip", help="Skip a cue without resolving")
    p_cue_skip.add_argument("cue_id", help="ID of the cue to skip")
    p_cue_skip.add_argument("--reason", "-r", default="", help="Why it was skipped")

    p_cues = subparsers.add_parser("cues", help="List all cues")
    p_cues.add_argument("--pending", "-p", dest="pending_only", action="store_true",
                       help="Show only pending (unresolved) cues")
    p_cues.add_argument("--author", "-a", default="", help="Filter by author")
    p_cues.add_argument("--verbose", "-v", action="store_true", help="Show replies and resolved cues")

    p_cue_assign = subparsers.add_parser("cue-assign", help="Assign a cue to someone")
    p_cue_assign.add_argument("cue_id", help="ID of the cue to assign")
    p_cue_assign.add_argument("name", help="Name of the person to assign to")

    p_cue_unassign = subparsers.add_parser("cue-unassign", help="Remove assignee from a cue")
    p_cue_unassign.add_argument("cue_id", help="ID of the cue to unassign")

    p_cue_start = subparsers.add_parser("cue-start", help="Mark a cue as in-progress")
    p_cue_start.add_argument("cue_id", help="ID of the cue to start")

    p_cue_stop = subparsers.add_parser("cue-stop", help="Mark a cue as no longer in-progress")
    p_cue_stop.add_argument("cue_id", help="ID of the cue to stop")

    p_cue_delete = subparsers.add_parser("cue-delete", help="Permanently delete a cue")
    p_cue_delete.add_argument("cue_id", help="ID of the cue to delete")

    p_cue_archive = subparsers.add_parser("cue-archive", help="Archive a resolved/skipped cue")
    p_cue_archive.add_argument("cue_id", nargs="?", default="", help="ID of the cue to archive (omit to archive all resolved/skipped)")

    # Config
    p_config = subparsers.add_parser("config", help="View or edit configuration")
    p_config.add_argument("key", nargs="?", default="", help="Setting key to view or set")
    p_config.add_argument("value", nargs="?", default=None, help="Value to set (omit to view)")

    # ── Remote / Push / Pull / Sync ──
    p_remote = subparsers.add_parser("remote", help="Manage remote clavus servers")
    p_remote.add_argument("action", nargs="?", choices=["list", "add", "remove", "rename", "projects", "pull"], default="list",
                         help="Action: list, add, remove, rename, projects, or pull")
    p_remote.add_argument("name", nargs="?", default="", help="Remote name (and project name for pull)")
    p_remote.add_argument("url", nargs="?", default="", help="Remote URL or project name (for pull)")
    p_remote.add_argument("--add", default="", help=argparse.SUPPRESS)
    p_remote.add_argument("--remove", default="", help=argparse.SUPPRESS)

    p_push = subparsers.add_parser("push", help="Push cues/snapshots to remotes")
    p_push.add_argument("remote", nargs="?", default=None,
                        help="Remote name (default: all)")
    p_push.add_argument("--force", "-f", action="store_true",
                        help="Force push — skip conflict check, overwrite relay state")

    p_pull = subparsers.add_parser("pull", help="Pull cues/snapshots from remotes")
    p_pull.add_argument("remote", nargs="?", default=None,
                        help="Remote name (default: all)")
    p_pull.add_argument("--output", "-o", type=str, default=None,
                        help="Output directory for project folder")

    p_pull_all = subparsers.add_parser("pull-all", help="Pull ALL projects from active remote")
    p_pull_all.add_argument("remote", nargs="?", default=None,
                            help="Remote name (default: active remote)")

    p_sync = subparsers.add_parser("sync", help="Start auto-sync daemon")
    p_sync.add_argument("--interval", "-i", type=int, default=30,
                        help="Poll interval in seconds (default: 30)")

    # ── Branch / Checkout / Merge ──
    p_branch = subparsers.add_parser("branch", help="List or create branches")
    p_branch.add_argument("name", nargs="?", default=None, help="Branch name to create")
    p_branch.add_argument("--delete", "-d", default="", help="Delete a branch")
    p_branch.add_argument("--list", "-l", action="store_true", help="List all branches")

    p_checkout = subparsers.add_parser("checkout", help="Switch branches")
    p_checkout.add_argument("name", help="Branch to switch to")
    p_checkout.add_argument("-b", action="store_true", help="Create branch then switch")

    p_merge = subparsers.add_parser("merge", help="Merge another branch into current")
    p_merge.add_argument("branch", help="Branch name to merge from")
    p_merge.add_argument("--message", "-m", default="", help="Merge commit message")
    p_merge.add_argument("--no-ff", action="store_true", help="Create a merge commit even if fast-forward")

    p_cue_render = subparsers.add_parser("cue-render", help="Export cues as Ableton markers")
    p_cue_render.add_argument("--output", "-o", default="", help="Output file path")
    p_cue_render.add_argument("--inject", action="store_true",
                             help="Inject cues directly into the project's .als file (creates backup)")

    # Watch
    p_watch = subparsers.add_parser("watch", help="Auto-snapshot on file changes")
    p_watch.add_argument("subcommand", nargs="?", default=None,
                        choices=["install", "start", "stop", "restart", "status"],
                        help="install (set up launchd/systemd service), start, stop, restart, status")
    p_watch.add_argument("--cooldown", "-c", type=int, default=30,
                        help="Seconds to wait after last change before snapshotting (default: 30)")
    p_watch.add_argument("--quiet", "-q", action="store_true",
                        help="Suppress detailed output")
    p_watch.add_argument("--once", action="store_true",
                        help="Take one snapshot and exit (useful for cron jobs)")

    # Relay (stripped-down always-on server)
    p_relay = subparsers.add_parser("relay", help="Start the Clavus relay server for collaboration")
    p_relay.add_argument("--host", default=None,
                        help="Host to bind to (default: from config or 0.0.0.0)")
    p_relay.add_argument("--port", "-p", type=int, default=None,
                        help="Port to listen on (default: from config or 7890)")
    p_relay.add_argument("--project", type=str, default=None,
                        help="Only serve this project (hide others from collaborators)")

    # Share (relay + auto-discovery)
    p_share = subparsers.add_parser("share", help="Start a share session — relay + auto-discovery")
    p_share.add_argument("--host", default=None,
                        help="Host to bind to (default: from config or 0.0.0.0)")
    p_share.add_argument("--port", "-p", type=int, default=None,
                        help="Port to listen on (default: from config or 7890)")

    p_share.add_argument("--background", "-b", action="store_true",
                        help="Run relay in background, return immediately")
    p_share.add_argument("--bg", action="store_true",
                        help="Alias for --background")
    p_share.add_argument("--kill", action="store_true",
                        help="Stop the background relay")
    p_share.add_argument("--project", type=str, default=None,
                        help="Only serve this project (hide others from collaborators)")
    # Join (discover and connect)
    p_join = subparsers.add_parser("join", help="Discover and connect to a Clavus share session")
    p_join.add_argument("code", nargs="?", default="",
                       help="Share code or relay URL to connect to (e.g. BRIGHT-DUCK-7 or http://100.127.1.109:7890)")
    p_join.add_argument("--timeout", "-t", type=int, default=5,
                       help="Seconds to scan for (default: 5)")
    p_join.add_argument("--lan", action="store_true",
                       help="Scan LAN only (default: LAN + Tailscale)")
    p_join.add_argument("--tailscale", action="store_true",
                       help="Scan Tailscale only (default: LAN + Tailscale)")

    # Inject (alias for cue-render --inject)
    subparsers.add_parser("inject", help="Inject unresolved cues as Ableton markers")

    # TUI (terminal dashboard)
    p_tui = subparsers.add_parser("tui", help="Launch the TUI dashboard")
    p_tui.add_argument("--connect", "-c", default="",
                       help="Clavus server URL (default: from config or http://localhost:7890)")
    p_tui.add_argument("--debug", "-d", action="store_true",
                       help="Enable diagnostic logging to ~/.clavus/debug.log")

    # Find (LAN discovery)
    p_find = subparsers.add_parser("find", help="Find Clavus servers on your LAN or Tailscale tailnet")
    p_find.add_argument("--timeout", "-t", type=int, default=3,
                        help="Seconds to scan for (default: 3)")
    p_find.add_argument("--pair", "-p", default="",
                        help="Auto-pair with a discovered server by hostname")
    p_find.add_argument("--tailscale", action="store_true",
                        help="Scan your Tailscale tailnet instead of LAN")

    # ── Stem subcommands ──
    p_stem = subparsers.add_parser("stem", help="Manage stems (audio exports)")
    stem_sub = p_stem.add_subparsers(dest="stem_action", help="Stem commands")

    p_stem_import = stem_sub.add_parser("import", help="Import a stem file")
    p_stem_import.add_argument("file", help="Path to the stem audio file")
    p_stem_import.add_argument("--track", "-t", required=True,
                               help="Track name (e.g., 'Kick', 'Bass', 'Vocal')")

    p_stem_import_folder = stem_sub.add_parser("import-folder", help="Import all WAV files from a folder as stems")
    p_stem_import_folder.add_argument("folder", help="Path to folder containing .wav files")
    p_stem_import_folder.add_argument("--prefix", "-p", default="",
                                     help="Optional prefix for track names (e.g. 'Drums -')")

    p_stem_list = stem_sub.add_parser("list", help="List stems for a snapshot")
    p_stem_list.add_argument("--snapshot", "-s", default="",
                             help="Snapshot hash (default: current HEAD)")

    p_stem_push = stem_sub.add_parser("push", help="Push stem files to remotes")

    p_stem_pull = stem_sub.add_parser("pull", help="Pull stem files from remotes")

    # Repair (recover from corrupt/missing index)
    p_doctor = subparsers.add_parser("doctor", help="Diagnose Clavus store health (read-only)")
    p_doctor.add_argument("--verbose", "-v", action="store_true", help="Show detailed info")

    # P2P (peer-to-peer sync over Tailscale)
    p_p2p = subparsers.add_parser("p2p", help="Discover and sync with peers on the tailnet")
    p_p2p.add_argument("--host", action="store_true", help="Start listening for incoming connections")
    p_p2p.add_argument("--connect", type=str, default="",
                       help="Connect to a peer by their Tailscale DNS name")

    p_setup = subparsers.add_parser("setup", help="Guided first-run setup")
    p_repair = subparsers.add_parser("repair", help="Repair Clavus storage — recover projects from backup, cues, and refs")
    p_repair.add_argument("--force", "-f", action="store_true",
                          help="Force repair even if index.json exists")
    p_repair.add_argument("--set-als", type=str, default="",
                          help="Set .als path for recovered projects (name=/path format or 'all=/path')")

    args = parser.parse_args()

    if args.version:
        try:
            from importlib.metadata import version
            v = version("clavus")
        except ImportError:
            v = "0.8.0-beta"
        print(f"clavus {v}")
        return

    # Override clavus directory if specified
    if args.clavus_dir:
        DEFAULT_CLAVUS_DIR = Path(args.clavus_dir)

    # Dispatch
    commands = {
        "init": cmd_init,
        "projects": cmd_projects,
        "project": cmd_project,
        "snapshot": cmd_snapshot,
        "log": cmd_log,
        "diff": cmd_diff,
        "status": cmd_status,
        "watch": cmd_watch,
        "relay": cmd_relay,
        "share": cmd_share,
        "join": cmd_join,
        "tui": cmd_tui,
        "branch": cmd_branch,
        "checkout": cmd_checkout,
        "remote": cmd_remote,
        "doctor": cmd_doctor,
        "p2p": cmd_p2p,
        "setup": cmd_setup,
        "help": cmd_help,
        "repair": cmd_repair,
        "push": cmd_push,
        "pull": cmd_pull,
        "pull-all": cmd_pull_all,
        "sync": cmd_sync,
        "merge": cmd_merge,
        "restore": cmd_restore,
        "open": cmd_open,
        "backup": cmd_backup,
        "backups": cmd_list_backups,
        "restore-store": cmd_restore_store,
        "cue": cmd_cue,
        "cue-reply": cmd_cue_reply,
        "cue-resolve": cmd_cue_resolve,
        "cue-skip": cmd_cue_skip,
        "cues": cmd_cues,
        "cue-render": cmd_cue_render,
        "cue-assign": cmd_cue_assign,
        "cue-unassign": cmd_cue_unassign,
        "cue-start": cmd_cue_start,
        "cue-stop": cmd_cue_stop,
        "cue-delete": cmd_cue_delete,
        "cue-archive": cmd_cue_archive,
        "config": cmd_config,
        "find": cmd_find,
    }

    if args.command in commands:
        commands[args.command](args)
    elif args.command == "inject":
        args.inject = True
        cmd_cue_render(args)
    elif args.command == "stem":
        stem_actions = {
            "import": cmd_stem_import,
            "import-folder": cmd_stem_import_folder,
            "list": cmd_stem_list,
            "push": cmd_stem_push,
            "pull": cmd_stem_pull,
        }
        if args.stem_action in stem_actions:
            stem_actions[args.stem_action](args)
        else:
            print("Usage: clavus stem {import|list|push|pull}")
    else:
        cmd_help(args)


if __name__ == "__main__":
    main()
