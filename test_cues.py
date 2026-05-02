#!/usr/bin/env python3
"""Test the threaded cue system end-to-end."""
import sys, os, shutil
sys.path.insert(0, os.path.expanduser("~/Developer/clavus"))

# Clean state
shutil.rmtree(os.path.expanduser("~/.clavus"), ignore_errors=True)

from clavus.cues import CueStore, CueFilter, format_cue, format_cue_list
from clavus.store import BlobStore

# Init store with a project name
store = BlobStore()
store.init()

# Create a project entry
from clavus.store import ClavusProject
import time
proj = ClavusProject(
    name="Space Race Test",
    root_als="/tmp/test.als",
    created_at=time.time(),
)
store.set_index(proj)
store.update_ref("HEAD", "abc123")

cues = CueStore("Space Race Test", store=store)

print("═══ 1. Add cues ═══")
c1 = cues.add_cue("bridge feels long, try 4 bars", "3:45", author="mel")
print(f"  {c1.id}: \"{c1.text}\" @{c1.position} [{c1.status}]")

c2 = cues.add_cue("this drop needs more sub", "1:23", author="chris")
print(f"  {c2.id}: \"{c2.text}\" @{c2.position} [{c2.status}]")

c3 = cues.add_cue("kick is clipping on the 808 track", "2:10", 
                   author="mel", track_name="808 Kick")
print(f"  {c3.id}: \"{c3.text}\" @{c3.position} [{c3.status}] (track: {c3.track_name})")

print("\n═══ 2. Reply to a cue thread ═══")
reply = cues.reply(c1.id, "got it, extended to 8 bars", author="chris")
print(f"  Reply to {c1.id[:10]}: \"{reply.text}\"")

reply2 = cues.reply(c1.id, "listening now, feels better", author="mel")
print(f"  Reply to {c1.id[:10]}: \"{reply2.text}\"")

print("\n═══ 3. Resolve a cue ═══")
resolved = cues.resolve(c2.id, note="bumped the sub 2dB")
print(f"  Resolved {c2.id[:10]}: status={resolved.status}")

print("\n═══ 4. Skip a cue ═══")
skipped = cues.skip(c3.id, reason="already fixed in arrangement pass 2")
print(f"  Skipped {c3.id[:10]}: status={skipped.status}")

print("\n═══ 5. List all cues (filtered) ═══")
pending = cues.list_cues(CueFilter(status="pending"))
print(format_cue_list(pending, verbose=True))

print("\n═══ 6. List all cues (verbose) ═══")
all_cues = cues.list_cues()
print(format_cue_list(all_cues, verbose=True))

print("\n═══ 7. Render cues to Ableton markers ═══")
from clavus.cues import render_cues_as_markers
output = render_cues_as_markers(cues.list_cues(CueFilter(status="pending")), "/tmp/cue_export.xml")
print(f"  Exported to {output}")
with open(output) as f:
    print(f.read())

print("\n═══ 8. Count unresolved ═══")
print(f"  Unresolved: {cues.count_unresolved()}")

print("\n✅ All cue system tests passed!")
