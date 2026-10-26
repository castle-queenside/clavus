from __future__ import annotations

"""Clavus CLI command: cue-render — Export cues as Ableton markers."""
import argparse
from clavus.cues import CueStore, CueFilter, render_cues_as_markers
from clavus.helpers import get_store_and_project

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

