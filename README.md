# Clavus — Ableton Live Project Collaboration

**Version:** 0.6.0  **Platforms:** macOS · Windows · Linux

Clavus is a Git-for-Ableton collaboration tool. It parses `.als` files, snapshots project state, tracks threaded comments (cues) pinned to timeline positions, computes visual diffs between versions, and syncs everything over Tailscale or LAN — no cloud, no plugins, no hassle.

## Quick Start

```bash
pip install clavus          # or: pip install -e .   (from source)
clavus init                  # guided project setup
clavus tui                   # terminal dashboard
```

**First time?** Run `clavus config --wizard` to set your author name.

```bash
# Typical workflow:
clavus project "My Track"       # switch to a saved project
clavus cue "fix the kick @2"    # add a cue at bar 2
clavus snapshot "arranged intro" # save a checkpoint
clavus log                       # view history
```

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
- **Visual diffs** — see what changed between snapshots (tracks, clips, BPM, markers)
- **Cues** — threaded comments pinned to timeline positions (e.g. `@2:1.1 fix the kick`)
- **Stem sync** — content-addressed audio WAV transfer between peers
- **P2P sync** — pull/push over Tailscale or LAN, no server needed
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
| `D` | Delete cue |
| `x` | Archive |
| `U` | Push stems |
| `T` | Restore to selected snapshot |
| `d` | Show diff of selected snapshot |
| `p` | Pull cues from remotes |
| `P` | Push cues to remotes |
| `Tab` | Switch between cues/history panes |
| `j` / `k` | Navigate up/down |
| `q` | Quit |
| `:` | Command mode (`:snapshot msg`, `:stem push`, etc.) |

### Quick share/join

```bash
# Person A (sharer):
clavus share
# → Share code: BRIGHT-DUCK-7

# Person B (joiner):
clavus join
# → finds A, auto-configures remote, pulls project
```

## Setup for Collaborators

See [SETUP_STEVEN.md](SETUP_STEVEN.md) — step-by-step Windows setup guide.

**Collaborator needs:** Python 3.10+, Git, Tailscale, and `pip install clavus`

## Architecture

```
clavus/
├── clavus/
│   ├── parser.py         # .als XML parser
│   ├── store.py          # BlobStore, snapshots, diff engine
│   ├── visual_diff.py    # Clip-level side-by-side arrangement diff
│   ├── cues.py           # Cue CRUD + Ableton marker injection
│   ├── config.py         # User config
│   ├── helpers.py        # Shared utilities
│   ├── watch.py          # File watcher daemon
│   ├── sync.py           # P2P sync over HTTP
│   ├── discovery.py      # mDNS + Tailscale peer discovery
│   ├── web.py            # FastAPI relay server (API + WebSocket)
│   ├── tui.py            # Textual terminal dashboard
│   └── cli.py            # CLI entry point
├── SETUP_STEVEN.md       # Windows collaborator guide
└── pyproject.toml
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
- Git integration

## What's Stable (May 2026)

- Full TUI with cues list, snapshot history, assignee tracking
- P2P push/pull of cues and audio stems over Tailscale/LAN
- Snapshots with visual diffs (arrangement, tracks, clips, BPM)
- Snapshot restore (CLI + TUI)
- Live 12 `.als` format support (Ableton wrapper, tracks container, palette colors)
- Cue injection as Ableton markers
- Assignees survive push/pull cycles
- Cues sorted by timeline position
- `j`/`k` navigation on both cues and snapshot panes
- Full teal border focus indicator on active panel
- Interactive init wizard with .als summary and project description
- Config wizard for first-time setup
- Archive uses status change (no file moving, full history preserved)
- Share/Join — Tailscale-first peer discovery with human-friendly codes
- Relay server for always-on collaboration (VPS, Pi, old laptop)

## Install

```bash
pip install clavus

# From source:
git clone https://github.com/slowhands/clavus
cd clavus
pip install -e .
```

## License

MIT
