# Clavus — Ableton Live Project Collaboration

**Version:** 0.7.0-beta  **Platforms:** macOS · Windows · Linux

Clavus snapshots, syncs, and helps you collaborate on Ableton Live projects. Think of it as Git for your `.als` files — threaded comments pinned to timeline positions, push/pull sync over Tailscale, and a keyboard-driven terminal dashboard. No cloud, no plugins, no accounts.

## Quick Start

```bash
# Install
git clone https://github.com/castle-queenside/clavus
cd clavus
pip install -e .

# First-time setup (guided wizard)
clavus setup

# Open the dashboard
clavus tui
```

## How It Works

Two people. One relay. Zero friction.

```
┌─────────┐                    ┌───────────────┐                    ┌─────────┐
│   You   │ ◄── push/pull ───► │     Relay     │ ◄── push/pull ───► │  Peer   │
│  (Mac)  │   via Tailscale    │  (your Mac)   │   via Tailscale    │  (Win)  │
└─────────┘                    └───────────────┘                    └─────────┘
```

One person runs the relay (just `clavus share` on their machine). Both push and pull through it. The relay is dumb — it just stores what's pushed. No middleman, no cloud.

**The rhythm:**
1. `clavus tui` — open the dashboard
2. Press `p` — pull the latest cues and snapshots
3. Work in Ableton, save your project
4. Press `C` — snapshot your changes with a message
5. Press `P` — push to share your work
6. Repeat

## Collaborator Onboarding

Send this to anyone joining your project:

**1. Install prerequisites**
```bash
# Python 3.10+
winget install Python.Python.3.13      # Windows
brew install python@3.13               # macOS

# Git
winget install Git.Git                 # Windows (macOS: pre-installed)
```

**2. Install Tailscale** — creates a private network between machines
- Download from https://tailscale.com/download
- After install: `tailscale ip -4` — save this, you'll need to share it

**3. Install Clavus**
```bash
git clone https://github.com/castle-queenside/clavus
cd clavus
pip install -e .
clavus setup
```

**4. Connect to the project** — the host gives you their Tailscale IP + port

```bash
clavus join http://<host-tailscale-ip>:7890
clavus pull
clavus tui
```

Projects land at `~/Clavus/Projects/` (macOS) or `C:\Users\<you>\Clavus\Projects\` (Windows).

**5. Each session:**
```bash
clavus tui     # open dashboard
p              # pull latest
C              # snapshot after changes
P              # push
```

## Keybindings (TUI)

| Key | Action |
|-----|--------|
| `c` | New cue — comment at current timeline position |
| `C` | Snapshot — save a checkpoint of your project |
| `r` | Reply to a cue |
| `e` | Edit cue text |
| `a` | Assign a cue to someone |
| `R` | Resolve / unresolve |
| `x` | Archive a cue |
| `!` | Resolve sync conflict |
| `T` | Restore to a previous snapshot |
| `d` | Show diff of selected snapshot |
| `p` | Pull from remotes |
| `P` | Push to remotes |
| `Tab` | Switch between cues / history pane |
| `j` / `k` | Navigate up / down |
| `q` | Quit |
| `:` | Command mode (`:snapshot msg`, `:project name`, `:join URL`, etc.) |

## FAQ

### I just installed. Now what?

`clavus setup` walks you through first-time config. After that:

- **Working solo?** `clavus init "My Track"` then `clavus tui` — snapshots, cues, and restores work great solo. No relay needed.
- **Joining a project?** Get the host's Tailscale IP, then `clavus join http://<ip>:7890` then `clavus pull`.

### Nothing shows up in the TUI. It says "no project."

You need to either initialize a project (`:init ~/path/to/project.als`) or pull from a remote (`:join http://IP:PORT` then `p`). If you've already pulled via CLI, just open the TUI — it picks up your last project.

### The dot in the header is yellow (or dim).

- **Green ●** — connected and synced. You're good.
- **Yellow ○** — remote configured but no data pulled yet. Press `p`.
- **Dim ○** — no remote configured. Use `:join http://IP:PORT` to add one.

### I pressed `p` and nothing happened.

Check `:status` in the TUI. If it says "no remotes," you need to add one with `:join http://IP:PORT`. If the relay is unreachable, make sure Tailscale is connected and the host is running `clavus share`.

### My collaborator can't connect.

1. Host: run `clavus share` — it prints the exact URL
2. Host: verify Tailscale is connected: `tailscale status`
3. Collaborator: `clavus join http://<host-ip>:7890`
4. Collaborator: `clavus pull`

If it still fails, check the relay is reachable:
```bash
curl http://<host-tailscale-ip>:7890/api/ping
# Should return: {"status":"ok"}
```

### I'm on Windows and the TUI looks weird or blank.

Use **Windows Terminal** (install from the Microsoft Store). The old PowerShell/conhost terminal has rendering issues with Textual apps. Also, make sure you only have ONE remote configured — remove any localhost entry with `clavus remote remove relay http://localhost:7890`.

### I have two remotes both named "relay." Is that a problem?

Yes — the localhost one (`http://localhost:7890`) will fail on Windows because the relay runs on the Mac, not your machine. Remove it:
```bash
clavus remote remove relay http://localhost:7890
```

### How do I resolve a sync conflict?

If both people edit the same cue, the TUI shows ⚠ on that cue. Press `!` to pick a winner — your version or theirs. Push after resolving. The other side pulls and gets the resolved version automatically.

### Can I use this without a collaborator?

Absolutely. Clavus works great solo:
- **Snapshots** — version control for your `.als` file. Roll back to any checkpoint.
- **Cues** — leave yourself notes at specific timeline positions. Injects as Ableton markers.
- **Diffs** — see what changed between snapshots (tracks, devices, clips).

### Where are my files?

Everything lives under `~/Clavus/` (macOS/Linux) or `C:\Users\<you>\Clavus\` (Windows):
- `Projects/` — your `.als` files organized by project
- `store/` — snapshots, cue data, and sync metadata

### How do I update Clavus?

```bash
cd clavus
git pull
pip install -e .
```

If you're running the relay, restart it afterward: `pkill -f "clavus relay" ; clavus share`

### Something's broken. How do I debug?

```bash
clavus doctor           # health check
clavus log              # recent activity
:status                 # connection status (in TUI)
curl http://<relay-ip>:7890/api/ping   # relay reachability
```

## Features

- **Snapshot version control** — content-addressed snapshots with diffs
- **Cues** — threaded comments pinned to timeline positions, injected as Ableton markers
- **P2P sync** — push/pull over Tailscale via a shared relay
- **Sample sync** — WAV files hashed and synced alongside snapshots
- **Conflict detection** — ⚠ warns on concurrent edits, `!` to resolve
- **Snapshot restore** — roll back to any saved checkpoint
- **TUI dashboard** — keyboard-driven, Ableton-style dark theme
- **Auto-snapshot** — file watcher daemon for automatic checkpoints

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
│   ├── web.py            # FastAPI relay server
│   ├── visual_diff.py    # Clip-level ASCII timeline diff
│   ├── tui.py            # Textual terminal dashboard
│   └── cli.py            # CLI entry point
├── pyproject.toml
```

## License

MIT
