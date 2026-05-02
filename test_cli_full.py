#!/usr/bin/env python3
"""Full CLI workflow test with simulated project changes."""
import sys, os, shutil, time
sys.path.insert(0, os.path.expanduser("~/Developer/clavus"))

# Clean any previous test data
shutil.rmtree(os.path.expanduser("~/.clavus"), ignore_errors=True)

base = "/Volumes/LaCie/Ableton Live Projects (Local Drive)/Ableton Live Projects (Local Drive)"
src = f"{base}/Space Race Project/Space Race.als"
# Copy to a test location so we can modify it
test_dir = os.path.expanduser("~/tmp/clavus_test")
os.makedirs(test_dir, exist_ok=True)
test_als = f"{test_dir}/Space Race Test.als"
shutil.copy2(src, test_als)

from clavus.cli import cmd_init, cmd_snapshot, cmd_log, cmd_diff, cmd_status
from argparse import Namespace

print("═══ 1. clavus init ═══")
cmd_init(Namespace(path=test_als, command="init", clavus_dir=None))

print("\n═══ 2. clavus status ═══")
cmd_status(Namespace(command="status", clavus_dir=None))

print("\n═══ 3. clavus snapshot (no change — should warn) ═══")
cmd_snapshot(Namespace(message="just checking", tag="", parent=None,
                       verbose=True, command="snapshot", clavus_dir=None))

print("\n═══ 4. Modify the .als (simulate: add a track via XML edit) ═══")
# Simulate a change by modifying the .als BPM
import gzip, xml.etree.ElementTree as ET
with gzip.open(test_als, "rb") as f:
    raw = f.read()
root = ET.fromstring(raw)
live_set = root.find("LiveSet")
master = live_set.find("MasterTrack")
chain = master.find("DeviceChain")
mixer = chain.find("Mixer")
tempo = mixer.find("Tempo")
manual = tempo.find("Manual")
manual.set("Value", "130")  # Change BPM from 122 to 130
with gzip.open(test_als, "wb") as f:
    f.write(b'<?xml version="1.0" encoding="UTF-8"?>\n')
    f.write(ET.tostring(root, encoding="utf-8"))
print("   ✅ BPM changed from 122 → 130")

print("\n═══ 5. clavus snapshot (actual change) ═══")
cmd_snapshot(Namespace(message="bumped tempo to 130", tag="arrangement",
                       parent=None, verbose=True, command="snapshot", clavus_dir=None))

print("\n═══ 6. clavus log ═══")
cmd_log(Namespace(limit=10, verbose=True, command="log", clavus_dir=None))

print("\n═══ 7. Modifying again: rename track ═══")
with gzip.open(test_als, "rb") as f:
    raw = f.read()
root = ET.fromstring(raw)
live_set = root.find("LiveSet")
tracks = live_set.find("Tracks")
# Rename first audio track
for t in tracks:
    if t.tag == "AudioTrack":
        name_elem = t.find("Name")
        effective = name_elem.find("EffectiveName")
        effective.set("Value", "Kick_130")
        break
with gzip.open(test_als, "wb") as f:
    f.write(b'<?xml version="1.0" encoding="UTF-8"?>\n')
    f.write(ET.tostring(root, encoding="utf-8"))
print("   ✅ Track renamed")

cmd_snapshot(Namespace(message="renamed kick track for new tempo", tag="",
                       parent=None, verbose=True, command="snapshot", clavus_dir=None))

print("\n═══ 8. clavus log (full) ═══")
cmd_log(Namespace(limit=20, verbose=True, command="log", clavus_dir=None))

print("\n═══ 9. clavus diff HEAD ═══")
cmd_diff(Namespace(hash=None, verbose=True, command="diff", clavus_dir=None))

print("\n═══ 10. clavus status ═══")
cmd_status(Namespace(command="status", clavus_dir=None))

# Cleanup
shutil.rmtree(test_dir, ignore_errors=True)
print("\n✅ Full CLI workflow test passed!")
