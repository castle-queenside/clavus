# Clavus — Ableton Live Project Collaboration

**Version:** 0.7.0-beta  **Platforms:** macOS · Windows · Linux

Clavus snapshots, syncs, and helps you collaborate on Ableton Live projects. It parses `.als` files, snapshots project state, tracks threaded comments (cues) pinned to timeline positions, and syncs everything over Tailscale or LAN — no cloud, no plugins, no hassle.

## How Collaboration Actually Works

**Two ways to collaborate — pick the one that fits your setup:**

### Option A: Direct P2P (no extra machine)

```
┌─────────┐                    ┌─────────┐
│   You   │ ◄── share/join ──► │  Peer   │
│  (Mac)  │   via Tailscale    │  (Win)  │
└─────────┘                    └─────────┘
```

One person runs `clavus share`, the other runs `clavus join`. That's it — Clavus discovers peers over Tailscale and you push/pull directly. No relay to set up, no extra hardware. Perfect for ad-hoc sessions.

### Option B: Dedicated relay (always-on — runs on anything)

```
┌─────────┐                    ┌───────────────┐                    ┌─────────┐
│   You   │ ◄── push/pull ───► │     Relay     │ ◄── push/pull ───► │  Peer   │
│  (Mac)  │   via Tailscale    │macOS/Win/Linux│   via Tailscale    │  (Win)  │
└─────────┘                    │  (Pi or VPS)  │                    └─────────┘
                               └───────────────┘
```

The relay is a dumb middleman — it just stores whatever is pushed to it. Run it on a Raspberry Pi, an old laptop, or a cloud VPS. Start it with: `clavus relay --port 7890 --host 0.0.0.0`

Only one relay is needed (doesn't matter who runs it). Both peers push and pull through it.

**The rhythm (same for both):**
1. **Both open the TUI:** `clavus tui` — this is the main interface
2. **Pull before working:** press `p` to get the latest cues and snapshots
3. **Work in Ableton** — save your project
4. **Snapshot your changes:** press `C` with a message like "added bassline"
5. **Push when done:** press `P` to send your cues and snapshots
6. **Pull to see their work:** press `p` — new cues and snapshots appear

## Quick Start

```bash
pip install clavus          # or: pip install -e .   (from source)
clavus setup                 # guided first-time configuration
clavus tui                   # terminal dashboard
```

```bash
# Typical workflow:
clavus init "My Track"          # initialize a new project
clavus project "My Track"       # switch to a saved project
clavus cue "fix the kick @2"    # add a cue at bar 2
clavus snapshot "arranged intro" # save a checkpoint
clavus log                       # view history
```

## Tips

- **Restart the relay after updating Clavus.** The relay holds code in memory. After upgrading on the relay machine, kill and restart it: `pkill -f "clavus relay" ; clavus relay --port 7890 --host 0.0.0.0 &`
- **Samples show offline on first Ableton open.** Click any sample in Ableton's browser and it resolves them all. This is an Ableton quirk, not a Clavus bug.
- **Conflicts resolve cleanly on one side.** If you both edit the same cue, pick a winner with `!`, push — the other side pulls and gets the resolved version automatically. No ping-pong.
- **The relay is just HTTP.** If something's not syncing, check: `curl http://<tailscale-ip>:7890/api/ping` — should return `{"status":"ok"}`.

## Environment

| Variable | Default | Description |
|----------|---------|-------------|
| `CLAVUS_AUTHOR` | system username | Author name for cues and snapshots |
| `CLAVUS_PORT` | `7890` | Server port for sync |
| `CLAVUS_HOST` | `0.0.0.0` | Server bind address |
| `CLAVUS_SERVER` | `http://localhost:7890` | Server URL for TUI/peers |

CLI flags (`--author`, `--port`) override env vars, which override config file.

## Features

- **Snapshot version control** — content-addressed snapshots of your `.als` project
- **Snapshot diffs** — bullet-list change summaries showing tracks, devices, and clip deltas
- **Cues** — threaded comments pinned to timeline positions (e.g. `@2:1.1 fix the kick`)
- **Cue injection** — unresolved cues rendered as Ableton markers in the `.als` file
- **Sync conflict detection** — ⚠ warns when both collaborators edit the same cue; press `!` to resolve
- **P2P sync** — pull/push over Tailscale or LAN, no server needed
- **Sample sync** — WAV audio files hashed and synced alongside snapshots
- **Share/Join** — one-shot share sessions with human-friendly codes
- **TUI dashboard** — keyboard-driven terminal interface (main way to use it)
- **Auto-snapshot** — file watcher daemon for automatic checkpoints
- **Snapshot restore** — roll back to any saved checkpoint

## Keybindings (TUI)

| Key | Action |
|-----|--------|
| `c` | New cue |
| `C` | Snapshot — save a checkpoint |
| `r` | Reply to a cue |
| `e` | Edit cue text |
| `a` | Assign a cue |
| `R` | Resolve/unresolve |
| `S` | Start/stop (toggle in-progress) |
| `x` | Archive |
| `!` | Resolve sync conflict |
| `T` | Restore to selected snapshot |
| `d` | Show diff of selected snapshot |
| `p` | Pull from remotes |
| `P` | Push to remotes |
| `Tab` | Switch between cues/history panes |
| `j` / `k` | Navigate up/down |
| `q` | Quit |
| `:` | Command mode (`:snapshot msg`, `:project name`, etc.) |

### Quick share/join

```bash
# Person A (sharer):
clavus share
# → Share code: BRIGHT-DUCK-7

# Person B (joiner):
clavus join
# → finds A, auto-configures remote, pulls project
```

## Collaborator Onboarding

Send this to anyone joining your project:

**1. Install prerequisites**
```bash
# Python 3.10+
winget install Python.Python.3.13      # Windows
brew install python@3.13               # macOS

# Git
winget install Git.Git                 # Windows
# macOS: pre-installed

# Tailscale — creates a private network between your machines
# Download from https://tailscale.com/download
# After install, run: tailscale ip -4   → this is your address
```

**2. Install Clavus**
```bash
git clone https://github.com/castle-queenside/clavus
cd clavus
pip install -e .
clavus setup
```

**3. Connect to the project**

*Option A — Share code (easiest):*
```bash
clavus join
# → auto-discovers the relay and pulls the project
```

*Option B — Direct Tailscale IP (if share doesn't work):*
```bash
clavus remote add relay http://<their-tailscale-ip>:7890
clavus pull
```

**4. Open in Ableton and start the TUI**
```bash
clavus tui
```
Projects live in `~/Clavus/Projects/` (macOS) or `C:\Users\<you>\Clavus\Projects\` (Windows).

## Architecture

```
clavus/
├── clavus/
│   ├── parser.py         # .als XML parser
│   ├── store.py          # BlobStore, snapshots, diff engine
│   ├── cues.py           # Cue CRUD + Ableton marker injection + conflict detection
│   ├── config.py         # User config
│   ├── helpers.py        # Shared utilities
│   ├── watch.py          # File watcher daemon
│   ├── sync.py           # P2P sync over HTTP
│   ├── discovery.py      # mDNS + Tailscale peer discovery
│   ├── web.py            # FastAPI relay server (API + WebSocket)
│   ├── visual_diff.py    # Clip-level ASCII timeline diff (CLI only)
│   ├── tui.py            # Textual terminal dashboard
│   └── cli.py            # CLI entry point
├── pyproject.toml
```

## Platform Compatibility

| Platform | Status | Notes |
|----------|--------|-------|
| macOS    | ✅ Primary | Tested on Sequoia 15.x, Apple Silicon + Intel |
| Windows  | ✅ Supported | Windows 10/11, Python 3.10+, Windows Terminal recommended |
| Linux    | ✅ Supported | For relay server & CLI (Ableton not available natively) |

All core features work on all platforms:
- TUI (Textual framework)
- CLI commands
- Relay server
- mDNS discovery (zeroconf)
- Tailscale discovery
- File polling watcher

## What's Stable (May 2026)

- Full TUI with cues list, snapshot history, assignee tracking
- P2P push/pull of cues, snapshots, and audio samples over Tailscale/LAN
- Sync conflict detection and resolution (⚠ indicator, `!` to resolve)
- Snapshot diffs (tracks, devices, clip counts)
- Snapshot restore (CLI + TUI)
- Live 12 `.als` format support (Ableton wrapper, tracks container, palette colors)
- Cue injection as Ableton markers
- Assignees survive push/pull cycles
- Cues sorted by timeline position
- `j`/`k` navigation on both cues and snapshot panes
- Peer connectivity indicator (● green dot in header)
- Dim teal border on cue list panel
- Interactive `clavus setup` wizard
- Archive uses status change (no file moving, full history preserved)
- Share/Join — Tailscale-first peer discovery with human-friendly codes
- Relay server for always-on collaboration (VPS, Pi, old laptop)

## Install

```bash
pip install clavus

# From source:
git clone https://github.com/castle-queenside/clavus
cd clavus
pip install -e .
```

## License

MIT
