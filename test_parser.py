from pathlib import Path
from clavus.parser import parse_als, project_summary

# Test with our synthetic fixture
fixture = Path(__file__).parent / "fixtures" / "test_project.als"
project = parse_als(fixture)

print(project_summary(project))
print("\n" + "─" * 50 + "\n")

# Validate
assert project.bpm == 128.0, f"Expected 128bpm, got {project.bpm}"
assert project.track_count == 8, f"Expected 8 tracks, got {project.track_count}"
assert len(project.return_tracks) == 2, f"Expected 2 return tracks, got {len(project.return_tracks)}"
assert len(project.markers) == 5, f"Expected 5 markers, got {len(project.markers)}"

# Check specific tracks
names = [t.name for t in project.tracks]
assert "Kick" in names
assert "Reverb" in [rt.name for rt in project.return_tracks]

# Check frozen/mute state
for t in project.tracks:
    assert t.is_frozen == False
    assert t.is_muted == False

# Check devices
kick = next(t for t in project.tracks if t.name == "Kick")
assert len(kick.devices) == 1
assert kick.devices[0].device_type == "Compressor"

print("✅ All assertions passed!")
