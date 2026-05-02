#!/usr/bin/env python3
"""Test the full CLI workflow end-to-end."""
import sys, os
sys.path.insert(0, os.path.expanduser("~/Developer/clavus"))

base = "/Volumes/LaCie/Ableton Live Projects (Local Drive)/Ableton Live Projects (Local Drive)"
path = f"{base}/Space Race Project/Space Race.als"

from clavus.cli import cmd_init, cmd_snapshot, cmd_log, cmd_diff, cmd_status
from argparse import Namespace

# Clean any previous test data
import shutil
shutil.rmtree(os.path.expanduser("~/.clavus"), ignore_errors=True)

print("═══ 1. clavus init ═══")
args = Namespace(path=path, command="init", clavus_dir=None)
cmd_init(args)

print("\n═══ 2. clavus status ═══")
args = Namespace(command="status", clavus_dir=None)
cmd_status(args)

print("\n═══ 3. clavus snapshot (first change) ═══")
args = Namespace(message="added reverb on master", tag="mix", parent=None,
                 verbose=True, command="snapshot", clavus_dir=None)
cmd_snapshot(args)

print("\n═══ 4. clavus snapshot (second change) ═══")
args = Namespace(message="brought kick up 2dB", tag="mix,kick",
                 parent=None, verbose=False, command="snapshot", clavus_dir=None)
cmd_snapshot(args)

print("\n═══ 5. clavus log ═══")
args = Namespace(limit=10, verbose=True, command="log", clavus_dir=None)
cmd_log(args)

print("\n═══ 6. clavus diff HEAD ═══")
args = Namespace(hash=None, verbose=True, command="diff", clavus_dir=None)
cmd_diff(args)

print("\n═══ 7. clavus diff (first snapshot by hash) ═══")
from clavus.store import BlobStore
store = BlobStore()
head = store.read_ref("HEAD")
snap = store.load_snapshot(head)
if snap:
    args = Namespace(hash=snap.short_hash(), verbose=True, command="diff", clavus_dir=None)
    cmd_diff(args)

print("\n✅ CLI workflow test complete!")
