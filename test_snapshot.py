"""Test the snapshot engine with a real Ableton project."""
from clavus import parse_als
from clavus.store import BlobStore, diff_projects, format_diff

base = "/Volumes/LaCie/Ableton Live Projects (Local Drive)/Ableton Live Projects (Local Drive)"

# Initialize the store
store = BlobStore()
store.init()

# Parse a project
path = f"{base}/Space Race Project/Space Race.als"
print(f"📁 Parsing: {path}")
project = parse_als(path)
print(f"   {project.track_count} tracks @ {project.bpm}bpm")

# Snapshot it
snap1 = store.save_snapshot(project, "Initial snapshot of Space Race")
print(f"📸 Snapshot: {snap1.short_hash()} — '{snap1.message}'")
print(f"   Track count: {snap1.track_count}")
print(f"   Parent: {snap1.parent}")
print()

# Save a ref
store.update_ref("HEAD", snap1.hash)
store.update_ref("refs/tags/v1", snap1.hash)

# Verify we can load it back
loaded_snap = store.load_snapshot(snap1.hash)
assert loaded_snap is not None
print(f"✅ Snapshot loaded back: {loaded_snap.short_hash()} — '{loaded_snap.message}'")

loaded_project = store.load_project(snap1.hash)
assert loaded_project is not None
print(f"✅ Project loaded back: {loaded_project.track_count} tracks @ {loaded_project.bpm}bpm")
assert loaded_project.track_count == project.track_count
assert loaded_project.bpm == project.bpm
print()

# --- Test diff: simulate adding a track ---
# We'll create a modified version
from clavus.parser import Project, Track, Marker

project2 = parse_als(path)
# Simulate a change: add a new track, change BPM
new_track = Track(name="New Bass", track_type="Audio", index=len(project2.tracks))
project2.tracks.append(new_track)
project2.bpm = 124.0

snap2 = store.save_snapshot(project2, "Added bass track, bumped BPM", parent=snap1.hash)
store.update_ref("HEAD", snap2.hash)
print(f"📸 Snapshot 2: {snap2.short_hash()} — '{snap2.message}'")

# Diff against parent
project1 = store.load_project(snap1.hash)
project2_loaded = store.load_project(snap2.hash)
diff = diff_projects(project1, project2_loaded)
print(f"\n📊 Diff:")
print(format_diff(diff, verbose=True))
print()

# Show log
print("📋 Log:")
snap = store.load_snapshot(store.read_ref("HEAD"))
while snap:
    prefix = "➡ " if snap.hash == store.read_ref("HEAD") else "  "
    print(f"  {prefix}{snap.short_hash()} — {snap.message} ({snap.timestamp:.0f})")
    snap = store.load_snapshot(snap.parent) if snap.parent else None

print("\n✅ All snapshot tests passed!")
