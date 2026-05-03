# Clavus Build Plan — Current State & What's Next

## Current Files

```
clavus/
├── __init__.py      # Format auto-detection (Ableton 10+ vs 9)
├── __main__.py      # CLI entry
├── cli.py           # CLI commands (init, snapshot, log, diff, status, cue, serve, tui, remote, branch, merge)
├── cues.py          # Cue CRUD + marker injection
├── git_integration.py  # Git-aware branch/merge
├── helpers.py       # Shared utilities (circular import breaker)
├── parser.py        # .als XML parser
├── store.py         # BlobStore, Snapshot, diff engine
├── sync.py          # Peer-to-peer sync daemon (HTTP + WebSocket)
├── tui.py           # Textual TUI — navigation, cue management, WebSocket real-time
├── watch.py         # File watcher daemon + auto-snapshot
└── web.py           # FastAPI web companion + sync endpoints + WebSocket

## Phase Status

| Phase | What | Status |
|-------|------|--------|
| 1 | .als parser + project model | ✅ Done |
| 2 | Blob storage + snapshots + diff | ✅ Done |
| 3 | CLI — init, snapshot, log, diff, status | ✅ Done |
| 4 | Cue system (CRUD, threaded replies, marker render) | ✅ Done |
| 5 | File watcher daemon + cue injection | ✅ Done |
| 6a | FastAPI web companion dashboard | ✅ Done |
| 6b | Sync endpoints (pull/push) | ✅ Done |
| 6c | Peer-to-peer sync daemon (sync.py) | ✅ Done |
| 6d | TUI — navigation, cue management, CRUD | ✅ Done |
| 6e | TUI — WebSocket real-time sync | ✅ Done |
| 6f | TUI — auto-reconnect + scroll preservation | ✅ Done |
| 6g | TUI — push format fix (wrap cues in dict) | ✅ Done |
| 6h | TUI — two-step cue input (text then position) | ✅ Done |
| 6i | TUI — `:name` command for author identity | ✅ Done |
| 6j | TUI — `:projects` lists + interactive switching | ✅ Done |
| 6k | TUI — `:init <path>` import project from TUI | ✅ Done |
| 6l | TUI — `:browse [dir]` filesystem browser | ✅ Done |
| 6m | Server — `POST /api/projects/init` endpoint | ✅ Done |
| 6n | Server — `GET /api/projects/browse` endpoint | ✅ Done |
| 7 | Git integration (branch/merge CLI) | ✅ Done |
| 8 | Tailscale + relay transport | ❌ Not started |
| 9 | Snapshot restore (`clavus restore <hash>` rewrites .als from saved file) | ❌ Not started |
| 10 | .als diff visualization in TUI | ❌ Not started |

## What Actually Needs Work

### 1. Snapshot restore (`clavus restore <hash>`)
- `cmd_snapshot` already parses `.als` — add step to save raw `.als` bytes to `~/.clavus/objects/<hash>.als`
- New `cmd_restore` — takes hash, loads stored `.als`, warns before overwriting current project
- `clavus log` should flag snapshots that have a full `.als` backup
- TUI `:restore <hash>` command
- ~1 day of focused work

### 2. Test the full real-time flow
End-to-end tested ✅ — TUI connects, pulls, pushes. Cue push format bug found and fixed.

### 3. TUI polish for jam sessions
- ✅ `:name Chris` — author attribution (persisted to ~/.config/clavus/config.json)
- ✅ `:projects` lists and `:project Name` switches
- ✅ Two-step cue input: text first, then position
- ✅ `:init <path>` — register project from TUI (via POST /api/projects/init)
- ✅ `:browse [dir]` — filesystem browser with navigation
- ✅ Browse shows registered vs unregistered .als files
- ✅ Init auto-switches to the new project after import

### 3. The sync daemon (sync.py) needs attention
- It expects `websockets` library but imports it lazily
- It's pull-only right now — needs push integration
- No way to start it from the TUI

### 4. Git integration (git_integration.py)
- Has branch/merge CLI commands but they're untested
- Would be powerful: "clavus branch try-vocal-rise" saves current cue state as a branch
- Low priority for jam sessions

### 5. Tailscale transport (Phase 8)
- Currently everything runs over HTTP on localhost or LAN
- For two people on different networks: need Tailscale awareness
- `clavus serve` already supports `--tailscale` but it's janky
- Migrate to Tailscale Funnel or just document: "both on Tailscale, use your Tailscale IP"

## What Matters for Tonight's Jam (Prioritized)

1. ✅ Real-time sync via WebSocket (done)
2. ✅ Auto-reconnect + scroll preservation (done)
3. ⚡ Quick test session — launch server, two TUI instances, verify cues flow
4. ✅ `:name Chris` — set author so cues show who wrote them (persisted to ~/.config/clavus/config.json)
5. ✅ `:projects` command in TUI — list + switch projects without quitting
6. ✅ Cue input stepped: `c` → text → enter → position → enter
7. ✅ `:init <path>` — register project from TUI without dropping to terminal
8. ✅ `:browse [dir]` — browse filesystem for .als files
9. 🔵 Status bar on WS events should show author name

## Run Commands

```bash
# Server
clavus serve                          # Starts web + WebSocket on :7890

# TUI
clavus tui                            # Connects to localhost:7890
clavus tui --connect http://10.0.0.5:7890  # Connect to friend

# CLI
clavus list                           # List projects
clavus switch "Project Name"          # Switch active project
clavus cue "text @position"           # Add a cue via CLI
clavus watch                          # Auto-snapshot on file changes
```

## Design Decisions that Are Set

- All state on the **server** (one source of truth). TUI is a client.
- WebSocket for real-time, HTTP pull/push for catch-up
- No cloud, no accounts, no auth (trust-based LAN/Tailscale)
- Cues are the collaboration primitive, not snapshots (snapshots are for versioning)

## Snapshot Restore (Phase 9)

**The feature:** `clavus restore <hash>` — rewrites the .als file from a saved snapshot. Full undo for when you've messed up your project.

**The clever-but-dumb approach:**

```bash
clavus snapshot "before I ruin the mix"   # saves a copy of the full .als file
# ... mess up in Ableton ...
clavus restore abc123                      # copies saved .als back over the current one
```

Instead of trying to regenerate a `.als` from parts (tracks + devices + plugins + markers), **just save a copy of the full `.als`** alongside the metadata. On restore, copy it back.

**Why this works:**
- Plugin binary blobs (VST3/AU presets) pass through untouched — no parsing needed
- Audio clips in the `.als` (warped audio data) are preserved as-is
- Ableton version drift doesn't matter — you're restoring the exact file that worked
- ~2-6MB per snapshot on disk — 100 snapshots is ~300MB, negligible

**Implementation:**
- `cmd_snapshot` already parses the `.als` — add a step to also copy the raw `.als` bytes to `~/.clavus/objects/<hash>.als`
- `cmd_restore` takes a hash, loads the stored `.als`, asks "⚠️ This will overwrite your current .als. Continue? (y/N)", then writes it back
- `clavus log` should show which snapshots have a full `.als` backup available

**Limitation:** The restored `.als` must be opened in the same Ableton version or newer (Ableton backwards-compat handles this). No guarantee for downgrade (Live 12 → Live 11).

**Why not the smart approach?** Generating a .als from scratch requires deep knowledge of Ableton's binary XML schema, which changes per-version and per-update. Saving the raw file is bulletproof and costs nothing in storage.

## What's Next: Ableton Sync via OSC

**The goal:** Push cues directly into Ableton's arrangement markers in real-time, so they show up in the arranger view alongside your clips. And optionally, read Ableton's transport state (playhead position, current BPM, scene changes) back into Clavus.

**How it'd work:**

A small companion process sits alongside Clavus and talks to Ableton via two channels:

1. **OSC → Ableton (outgoing)** — Send `/live/marker/add [position] [name]` when a cue is created. Ableton's LiveOSC or a M4L device picks it up and drops a marker in the arrangement.

2. **Ableton → OSC (incoming)** — Ableton sends transport state: `/live/play`, `/live/tempo`, `/live/stop`, `/live/current_time`. Clavus picks these up to auto-populate position fields or log arrangement checkpoints.

**The approach (3 options, worst to best):**

**Option A: LiveOSC (third-party, janky)**
A Python script that listens on an OSC port and translates to Clavus's HTTP API. Relies on LiveOSC being installed in Ableton. Fragile and unmaintained.

**Option B: M4L Device (recommended)**
A Max for Live device that exposes OSC endpoints for markers + transport. M4L is native to Ableton Suite and rock-solid. The device:
- Listens for incoming OSC messages to add markers
- Sends outgoing OSC on transport events (play, stop, position change)
- Clavus companion connects to the same OSC port

**Option C: Clavus Injector (simplest, already mostly works)**
Don't do real-time OSC at all. Instead:
- Run `clavus cue-render --inject` periodically (or on save via the watch daemon) to write cues as Ableton markers into the `.als` file
- When the user saves in Ableton, the watch daemon detects the change and re-injects markers
- No OSC, no M4L, no extra infra — just file-based marker injection you already built

**Why Option B wins for real-time:** Option C is fine for checkpoints but doesn't give real-time feedback in the arranger. Option A is a dependency hell. Option B (M4L) is the cleanest path: native Ableton integration, real-time, no fragile third-party tools.

**What an M4L approach needs:**
- A `.amxd` device exposing OSC send/receive
- A companion Python sidecar (`clavus-osc`) that bridges between clavus and the M4L device
- The companion runs alongside `clavus serve`, connected to the same WebSocket for real-time cue events

**Order of magnitude:** ~2-3 days of focused work for a basic M4L device + Python bridge, assuming existing Ableton/M4L knowledge.
