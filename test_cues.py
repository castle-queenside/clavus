#!/usr/bin/env python3
"""Test the threaded cue system end-to-end. Self-contained, uses temp dirs."""
import sys, os, shutil, tempfile, time
from pathlib import Path

# Use isolated temp dir — never touch real ~/.clavus
_TEST_CLAVUS_DIR = Path(tempfile.mkdtemp(suffix="_clavus_test"))

sys.path.insert(0, os.path.expanduser("~/Developer/clavus"))

from clavus.cues import CueStore, CueFilter, format_cue, format_cue_list
from clavus.store import BlobStore, ClavusProject

# Init store with a project name
store = BlobStore(_TEST_CLAVUS_DIR)
store.init()

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

print("═══ 5. List all cues (filtered) ═══")
pending = cues.list_cues(CueFilter(status="pending"))
print(format_cue_list(pending, verbose=True))

print("\n═══ 6. List all cues (verbose) ═══")
all_cues = cues.list_cues()
print(format_cue_list(all_cues, verbose=True))

print("\n═══ 7. Assign and unassign cues ═══")
c4 = cues.add_cue("bass needs sidechain", "4:12", author="chris")
assign_result = cues.assign(c4.id, "mel")
assert assign_result.status == "pending", f"Expected pending, got {assign_result.status}"
assert assign_result.assignee == "mel", f"Expected 'mel', got '{assign_result.assignee}'"
assert not assign_result.in_progress, "Should not be in_progress after assign"
print(f"  👤 Assigned {c4.id[:10]} to {assign_result.assignee} [{assign_result.status}]")

start_result = cues.start(c4.id)
assert start_result.in_progress, "Should be in_progress after start"
print(f"  ▶ Started {c4.id[:10]} (in_progress={start_result.in_progress})")

stop_result = cues.stop(c4.id)
assert not stop_result.in_progress, "Should not be in_progress after stop"
print(f"  ⏸ Stopped {c4.id[:10]} (in_progress={stop_result.in_progress})")

unassign_result = cues.unassign(c4.id)
assert unassign_result.assignee == "", f"Expected empty assignee, got '{unassign_result.assignee}'"
print(f"  👤 Unassigned {c4.id[:10]}")

print("\n═══ 8. Backward compat: load old-format cue (no assignee/in_progress) ═══")
import json
old_cue = {
    "id": "oldformat001", "position": "5.1.1",
    "text": "old format cue test", "author": "chris",
    "timestamp": time.time(), "status": "pending",
    "snapshot_hash": "", "track_name": "",
    "replies": [],
}
cues_dir = _TEST_CLAVUS_DIR / "cues" / "Space Race Test"
cues_dir.mkdir(parents=True, exist_ok=True)
old_path = cues_dir / "oldformat001.json"
with open(old_path, "w") as f:
    json.dump(old_cue, f)
loaded = cues.get_cue("oldformat001")
assert loaded is not None, "Old-format cue should load"
assert loaded.assignee == "", f"Expected empty assignee, got '{loaded.assignee}'"
assert not loaded.in_progress, "Should default to False"
print(f"  ✅ Old-format cue loaded: assignee='{loaded.assignee}', in_progress={loaded.in_progress}")

print("\n═══ 9. Delete and archive cues ═══")
assert cues.delete(c4.id), "Should delete"
assert cues.get_cue(c4.id) is None, "Deleted cue should be gone"
print(f"  🗑 Deleted {c4.id[:10]}")

# Re-add for archive test
c5 = cues.add_cue("test archive", "1:23", author="chris")
cues.resolve(c5.id, note="done")
ok = cues.archive(c5.id)
assert ok, "Archive should succeed"
print(f"  📦 Archived {c5.id[:10]} (status→archived)")

# Test archive_resolved
c6 = cues.add_cue("another test", "6:00", author="chris")
cues.resolve(c6.id, note="done too")
count = cues.archive_resolved()
print(f"  📦 Archived {count} resolved cue(s)")

print("\n═══ 10. Render cues to Ableton markers ═══")
from clavus.cues import render_cues_as_markers
# Re-add a pending cue so there's something to render
c7 = cues.add_cue("marker test cue", "4.1.1")
output = render_cues_as_markers(cues.list_cues(CueFilter(status="pending")), f"{_TEST_CLAVUS_DIR}/cue_export.xml")
print(f"  Exported to {output}")
if output:
    with open(output) as f:
        print(f.read()[:200])

print("\n═══ 11. Count unresolved ═══")
print(f"  Unresolved: {cues.count_unresolved()}")

# Cleanup
shutil.rmtree(str(_TEST_CLAVUS_DIR), ignore_errors=True)
print(f"\n✅ All cue system tests passed! (cleaned up {_TEST_CLAVUS_DIR})")
