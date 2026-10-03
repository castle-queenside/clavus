from __future__ import annotations

"""Clavus CLI commands: remote, find, push, pull, pull-all, sync."""
import argparse
from clavus.helpers import get_store_and_project
from clavus.sync import load_remotes

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
                            print(f"Using '{remote_name}' ({r.url})")
                            break
                    except Exception:
                        continue
                if not remote_name:
                    print("-- Specify a remote name: clavus remote projects <name>")
                    if remotes:
                        print(f"   Available: {', '.join(r.name for r in remotes)}")
                    return
        match = next((r for r in remotes if r.name == remote_name), None)
        if not match:
            print(f"-- Remote '{remote_name}' not found.")
            return

        from clavus.sync import SyncClient
        client = SyncClient(match.url)
        try:
            r = client.client.get(f"{match.url}/api/projects", timeout=10)
            if r.status_code != 200:
                print(f"-- Could not reach remote '{remote_name}' at {match.url}")
                return
            data = r.json()
            projects = data.get("projects", [])
            if not projects:
                print(f"No projects on '{remote_name}'")
                return
            print(f"Projects on '{remote_name}' ({match.url}):")
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
                            print(f"Using '{remote_name}' ({r.url})")
                            break
                    except Exception:
                        continue
                if not remote_name:
                    print("-- Specify a remote name: clavus remote pull <name> [project]")
                    if remotes:
                        print(f"   Available: {', '.join(r.name for r in remotes)}")
                    return

        match = next((r for r in remotes if r.name == remote_name), None)
        if not match:
            print(f"-- Remote '{remote_name}' not found.")
            return

        client = SyncClient(match.url)

        # If no project specified, list available and pick first
        if not remote_project:
            try:
                r = client.client.get(f"{match.url}/api/projects", timeout=10)
                if r.status_code == 200:
                    projects = r.json().get("projects", [])
                    if not projects:
                        print(f"No projects on '{remote_name}'")
                        return
                    if len(projects) == 1:
                        remote_project = projects[0]["name"]
                    else:
                        print(f"Multiple projects on '{remote_name}':")
                        for p in projects:
                            print(f"  {p.get('name', '?')}")
                        print()
                        print(f"  Specify one: clavus remote pull {remote_name} <name>")
                        return
            except Exception:
                pass

        if not remote_project:
            print("-- No project specified and couldn't auto-detect.")
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
                    print(f"-- Project '{remote_project}' not found on remote.")
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
                print(f"-- Failed to get remote project info: {e}")
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

        print(f"[ok] Synced '{remote_project}' from '{remote_name}'")
        print(f"   Switch to it: clavus project '{remote_project}'")
        print(f"   Pull again:   clavus remote pull {remote_name} {remote_project}")
        client.close()
        return

    # ── remote rename <old> <new> ──
    if args.action == "rename" or (args.name and args.url and not args.action):
        old_name = args.name
        new_name = args.url
        if not old_name or not new_name:
            print("-- Usage: clavus remote rename <old-name> <new-name>")
            return
        match = next((r for r in remotes if r.name == old_name), None)
        if not match:
            print(f"-- Remote '{old_name}' not found.")
            return
        if any(r.name == new_name for r in remotes):
            print(f"-- Remote '{new_name}' already exists.")
            return
        match.name = new_name
        save_remotes(store, remotes)
        print(f"[edit] Renamed remote '{old_name}' -> '{new_name}'")
        return

    # ── Existing behavior: add / remove / list ──
    name = args.add or (args.name if args.action == "add" else "")
    remove_name = args.remove or (args.name if args.action == "remove" else "")

    if name:
        # Add a remote
        for r in remotes:
            if r.name == name:
                print(f"[WARN]  Remote '{name}' already exists")
                return
        url = args.url or f"http://{name}.local:7890"
        remotes.append(Remote(name=name, url=url))
        save_remotes(store, remotes)
        print(f"Added remote '{name}' -> {url}")
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
            print("-- Usage: clavus remote rename <old_name> <new_name>")
            return
        match = next((r for r in remotes if r.name.lower() == old_name.lower()), None)
        if not match:
            print(f"-- Remote '{old_name}' not found.")
            if remotes:
                print(f"   Available: {', '.join(r.name for r in remotes)}")
            return
        old = match.name
        if any(r.name.lower() == new_name.lower() and r.name != old for r in remotes):
            print(f"-- Remote '{new_name}' already exists.")
            return
        match.name = new_name
        save_remotes(store, remotes)
        print(f"[edit]  Renamed '{old}' -> '{new_name}'")
        return

    # List remotes
    if not remotes:
        print(f"No remotes configured.")
        print(f"   Use 'clavus remote add <name> <url>' to add one.")
        return

    label = proj.name if proj else "this machine"
    print(f"Remotes for {label}")
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
            print("-- LAN discovery requires zeroconf. Install: pip install zeroconf")
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
            print(f"  -- No server found with hostname '{args.pair}'")
            return
        peer = matches[0]
        from clavus.store import BlobStore
        from clavus.sync import save_remotes, Remote, load_remotes
        store = BlobStore()
        remotes = load_remotes(store)
        for r in remotes:
            if r.name == peer.name:
                print(f"  [WARN]  Remote '{peer.name}' already exists")
                return
        remotes.append(Remote(name=peer.name, url=peer.url))
        save_remotes(store, remotes)
        print(f"  [ok] Paired with '{peer.name}' -> {peer.url}")
    print()

def cmd_push(args: argparse.Namespace) -> None:
    """Push cues and snapshots to remotes."""
    from clavus.progress import Spinner, ProgressBar, status

    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        # Fallback: if localhost exists but no remotes configured, use it silently
        from clavus.sync import Remote
        localhost_url = "http://localhost:7890"
        alt = next((r for r in load_remotes(store) if r.url.rstrip("/") == localhost_url), None)
        if alt:
            remotes = [alt]
        else:
            print(f"-- No remotes configured.")
            print(f"   Use 'clavus remote add <name> <url>' first.")
            return

    # If a remote name is specified, only push to that one
    if args.remote:
        remotes = [r for r in remotes if r.name == args.remote]
        if not remotes:
            print(f"-- Remote '{args.remote}' not found.")
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
            print(f"  -- {result['error']}")
        else:
            parts = [f"[ok] {result.get('cues', 0)} cues, {result.get('snapshots', 0)} snapshots"]

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
        print(f"-- No remotes configured.")
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
                    print(f"-- No remotes configured.")
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
                    print(f"-- Project '{target_project}' not found on any remote.")
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
            print("-- No projects found on any remote.")
            print("   Make sure the relay is running: clavus relay")
        return

    if not proj:
        return

    # If a remote name is specified, only pull from that one
    if args.remote:
        remotes = [r for r in remotes if r.name == args.remote]
        if not remotes:
            print(f"-- Remote '{args.remote}' not found.")
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
                            store.set_project_head(proj, snap.hash, source="auto-snapshot-cli")
                            print(f"📸 Auto-snapshot {snap.hash[:8]} (local changes saved)")
        except Exception:
            pass

        with Spinner(f"pulling from '{remote.name}'..."):
            result = pull_from_remote(store, proj, remote, output_dir=args.output)
        parts = []
        if result.get("error"):
            parts = [f"-- {result['error']}"]
        else:
            parts = [f"[ok] {result['cues']} cues, {result['snapshots']} snapshots"]

            # Pull snapshot content blobs + .als backups
            with Spinner("syncing blobs..."):
                from clavus.sync import pull_snapshot_blobs
                blob_count, failed = pull_snapshot_blobs(store, proj, remote)
                if blob_count:
                    parts.append(f"{blob_count} blob{'s' if blob_count != 1 else ''}")
                if failed:
                    parts.append(f"{len(failed)} blob(s) failed")

            # Materialize audio samples from store -> project folder
            head_ref = store.read_ref("HEAD")
            if head_ref:
                snap = store.load_snapshot(head_ref)
                if snap and snap.sample_hashes:
                    out_path = Path(proj.root_als)
                    base_dir = out_path.parent
                    base_dir.mkdir(parents=True, exist_ok=True)
                    samples_dir = base_dir / "Samples"
                    samples_dir.mkdir(exist_ok=True, parents=True)
                    written = 0
                    for sh in snap.sample_hashes:
                        fname = store.get_sample_filename(sh)
                        relpath = store.get_sample_relpath(sh) or ""
                        if fname and store.has_object(sh):
                            try:
                                if relpath and relpath.count("/") > 0:
                                    out_dir = base_dir / Path(relpath).parent
                                else:
                                    out_dir = samples_dir
                                out_dir.mkdir(parents=True, exist_ok=True)
                                store.materialize_sample(sh, out_dir, fname, relpath)
                                written += 1
                            except Exception:
                                pass
                    if written:
                        parts.append(f"{written} sample{'s' if written != 1 else ''}")

            # Pull stems for current HEAD
            head = store.read_ref("HEAD")
            if head:
                with Spinner("syncing stems..."):
                    from clavus.sync import pull_stems_from_remote
                    stem_count, _ = pull_stems_from_remote(store, proj, remote)
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
        print(f"-- No remotes configured.")
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
        print("-- No remote to pull from.")
        return

    with Spinner(f"fetching projects from '{active_remote.name}'..."):
        from clavus.sync import SyncClient as _SC
        _probe = _SC(active_remote.url)
        try:
            resp = _probe.client.get(f"{active_remote.url}/api/projects", timeout=10)
            if resp.status_code != 200:
                print(f"-- Remote '{active_remote.name}' unreachable.")
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
        status(f"  [ok] '{pname}' — {result.get('snapshots', 0)} snapshots")

    print(f"  Done: {pulled} pulled, {skipped} already local")


def cmd_sync(args: argparse.Namespace) -> None:
    """Start the auto-sync daemon."""
    store, proj = get_store_and_project()
    remotes = load_remotes(store)

    if not remotes:
        print(f"-- No remotes configured.")
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


