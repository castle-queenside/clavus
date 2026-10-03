from __future__ import annotations

"""Clavus CLI command: p2p — P2P sync over Tailscale."""
import argparse

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
                    print(f"  [WARN] Heads differ — syncing will detect conflicts")
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
            print(f"\n  [WARN] Sync Conflict")
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
                print(f"  [WARN] Heads differ — sync will detect conflicts")

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


