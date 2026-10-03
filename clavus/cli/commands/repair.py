from __future__ import annotations

"""Clavus CLI command: repair — Repair Clavus storage."""
import argparse
from clavus.store import BlobStore, ClavusProject

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
            print(f"[ok] index.json exists with {valid} project(s).")
            print("   Use --force to re-scan from scratch.")
            return
        except json.JSONDecodeError:
            print(f"[WARN] index.json is corrupt (will rebuild).")
    elif args.force:
        print("[build] Force mode: re-scanning from scratch.")

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
                    print(f"Found backup: {bak.name} ({len(valid)} project(s))")
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
                    print(f"   [note] Found cues for '{proj_dir.name}' ({cue_count} cue(s))")
                    proj = ClavusProject(
                        name=proj_dir.name,
                        root_als="",
                        created_at=time.time(),
                        head=head_hash,
                        description="(recovered — set .als path with --set-als)",
                    )
                    recovered[proj.name] = asdict(proj)
                elif cue_count > 0:
                    print(f"   [note] Cues for '{proj_dir.name}' ({cue_count} cue(s)) — already in backup")
                else:
                    print(f"   [note] Empty cues dir for '{proj_dir.name}' — skipping")

    if not recovered:
        print("-- Nothing to recover. No backups, cues, or refs found.")
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
                print(f"   [WARN]  Project '{name}' not found in recovered data.")

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
    print(f"[ok] Recovered {len([k for k in recovered if isinstance(recovered[k], dict)])} project(s) to {index_path}")
    print()
    for name, data in recovered.items():
        if not isinstance(data, dict):
            continue
        als = data.get("root_als", "") or "(no path)"
        head = data.get("head", "")[:12] if data.get("head") else "(no snapshots)"
        print(f"   {name:<25} {als}")
    print()
    if any(not data.get("root_als") for data in recovered.values() if isinstance(data, dict)):
        print("[TIP] Tip: Set .als paths with: clavus repair --set-als 'ProjectName=/path/to/project.als'")
        print("   Or for all at once:    clavus repair --set-als 'all=/path/to/ProjectDir/'")
    else:
        print("[ok] All projects have .als paths. Run 'clavus projects' to verify.")


