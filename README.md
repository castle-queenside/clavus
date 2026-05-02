# Clavus

> *Latin: keel, nail, rudder — the thing that steers and holds together.*

Git for Ableton. Snapshot, diff, sync, and comment on Ableton Live projects.

## Commands

```
clavus init                      # Initialize a new clavus project
clavus snapshot "message"        # Tag current state
clavus push                      # Sync to partners
clavus pull                      # Pull latest from partners
clavus status                    # Show uncommitted changes, pending cues
clavus log                       # Snapshot history (git log style)
clavus diff [snapshot]           # What changed since last snapshot
clavus branch <name>             # Fork the project
clavus merge <branch>            # Merge changes
clavus cue "text" @1:23          # Add a cue at timeline position
clavus cues                      # List all cues
clavus watch                     # Start file watcher daemon
clavus render                    # Export stems
```

## Build Status

Phase 1: .als parser — in progress
