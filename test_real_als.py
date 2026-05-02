"""Test the .als parser against real Ableton projects."""
from clavus.parser import parse_als, project_summary

base = "/Volumes/LaCie/Ableton Live Projects (Local Drive)/Ableton Live Projects (Local Drive)"

projects = [
    f"{base}/Space Race Project/Space Race.als",
    f"{base}/Me And You Project/Me And You.als",
    f"{base}/Bernard Wright Edit Project/Bernard Wright Edit.als",
]

for path in projects:
    print(f"{'='*60}")
    try:
        proj = parse_als(path)
        print(project_summary(proj))
        print(f"   Return tracks: {len(proj.return_tracks)}")
        print(f"   Tempo events: {len(proj.tempo_events)}")
        
        # List all devices across all tracks
        all_devices = {}
        for t in proj.tracks:
            for d in t.devices:
                all_devices[d.device_type] = all_devices.get(d.device_type, 0) + 1
        if all_devices:
            print(f"   Device types: {all_devices}")
            
    except Exception as e:
        print(f"❌ {path}: {e}")
    print()
