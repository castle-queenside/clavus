# Clavus — Ableton Live Project Collaboration

**Version:** 0.8.0-beta  **Platforms:** macOS · Windows · Linux

Clavus snapshots, syncs, and helps you collaborate on Ableton Live projects. Think of it as Git for your `.als` files — threaded comments pinned to timeline positions, push/pull sync over Tailscale, and a keyboard-driven terminal dashboard. No cloud, no plugins, no accounts.

## Quick Start

```bash
# Install
git clone https://github.com/castle-queenside/clavus
cd clavus
pip install -e .            # macOS / Linux
# Windows: py -m pip install -e .

# First-time setup (guided wizard)
clavus setup

# Open the dashboard
clavus tui
```

## How It Works

**Two paths — pick yours:**

### Solo (no relay needed)
Just you, your `.als`, and the TUI. Snapshots, cues, diffs, and restores all work locally. Great for version control and leaving yourself notes.

### Collaborating (one person shares, everyone else joins)

```
┌─────────┐                    ┌───────────────┐                    ┌─────────┐
│   You   │ ◄── push/pull ───► │     Relay     │ ◄── push/pull ───► │  Peer   │
│  (Mac)  │   via Tailscale    │(any machine)  │   via Tailscale    │  (Win)  │
└─────────┘                    └───────────────┘                    └─────────┘
```

One person runs `clavus share` — that machine becomes the relay. It can be anyone's Mac, Windows, or Linux box. **Share blocks the terminal** (it runs until you stop it), so use a dedicated terminal or run it in the background. Both people push and pull through it. The relay is dumb — it just stores what's pushed. No cloud, no dedicated server.

**The rhythm (relay host):**
1. `clavus share` — starts the relay in a dedicated terminal, prints the URL
2. Open a second terminal: `clavus tui` — your dashboard
3. Press `C` to snapshot after changes in Ableton
4. Press `P` to push

**The rhythm (everyone else):**
1. `clavus join http://<host-ip>:7890` then `clavus pull`
2. `clavus tui` — open the dashboard
3. Press `p` — pull the latest cues and snapshots
4. Work in Ableton, save your project
5. Press `C` — snapshot your changes with a message
6. Press `P` — push to share your work
7. Repeat 3-6

## Why It's Safe

Clavus is built to protect your work, not destroy it.

- **Optimistic locking** — if someone else pushes while you're working, your push is rejected with a "pull first" warning. You'll never silently overwrite someone's changes.
- **Auto-snapshot before pull** — the TUI snapshots your local changes before pulling from the relay, so nothing is ever lost.
- **Atomic push ordering** — snapshots land first, then cues. If snapshots fail (network drop, conflict), nothing touches the relay. No half-baked state.
- **Freeze detection** — frozen tracks crash Ableton on other platforms. Clavus warns you before snapshotting frozen tracks, so you don't accidentally ship a project that wrecks your collaborator's session.
- **Index backup & recovery** — your project index is backed up before every write (rotating `.bak`, `.bak2`, `.bak3`). A full store backup is created daily. If the index corrupts, Clavus auto-restores from backup.
- **Network retry** — transient failures (timeouts, connection resets) are retried up to 3 times with backoff. Only actual errors get through.

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
pip install -e .            # macOS / Linux
py -m pip install -e .      # Windows (if pip isn't on PATH)
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
| `!` | Resolve sync conflict (⚠ cue) |
| `T` | Restore to a previous snapshot |
| `d` | Show diff of selected snapshot |
| `p` | Pull from remotes (auto-snapshots local changes first) |
| `P` | Push to remotes |
| `U` | Push stems (WAV files) to remotes |
| `Tab` | Switch between cues / history pane |
| `j` / `k` | Navigate up / down |
| `q` | Quit |
| `:` | Command mode (`:snapshot msg`, `:share`, `:join URL`, `:stem push/pull`, `:backup`, `:restore`, `:doctor`, etc.) |

## Stem Sync

Sync WAV files alongside your `.als` project. Stems are content-addressed — only changed files transfer.

```bash
# CLI
clavus stem import ~/Samples/kick.wav     # add a stem to the project
clavus stem list                           # see what's tracked
clavus stem push                           # push stems to the relay
clavus stem pull                           # pull stems from the relay

# TUI
U                                          # push stems
:stem pull                                 # pull stems
```

Stems land in `~/Clavus/Projects/<name>/Stems/` after pull. Ableton finds them automatically (macOS).

## Backup & Recovery

```bash
clavus backup                   # create a full store backup (tar.gz)
clavus backups                  # list available backups
clavus restore-store <file>     # restore from a backup
clavus repair                   # repair corrupted index (auto-restores from .bak files)
```

Clavus also creates rotating index backups automatically before every write, and a daily full-store backup. If `index.json` goes missing, it auto-restores from `.bak` → `.bak2` → `.bak3` on next run.

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

### How do I run the relay in the background?

On macOS/Linux, add `&` to run it in the background:
```bash
clavus share &
```
To stop it later: `pkill -f "clavus relay"`

On Windows, just keep the relay terminal open (minimized is fine). You don't need to look at it.

### I got "HEAD has moved — pull first" when pushing.

This is conflict protection. Someone else pushed while you were working. Just press `p` to pull their changes (your work is auto-snapshotted first), then push again. You won't lose anything.

### How do I resolve a sync conflict?

If both people edit the same cue, the TUI shows ⚠ on that cue. Press `!` to open the conflict resolution screen — pick your version or theirs. Push after resolving. The other side pulls and gets the resolved version automatically.

### Clavus warned me about frozen tracks. What do I do?

Frozen tracks crash Ableton when opened on a different platform (e.g. Mac → Windows). Unfreeze those tracks in Ableton before snapshotting. If you're sure everyone's on the same OS, pass `--allow-frozen` to skip the warning.

### I'm on Windows and the TUI looks weird or blank.

Use **Windows Terminal** (install from the Microsoft Store). The old PowerShell/conhost terminal has rendering issues with Textual apps. Also, make sure you only have ONE remote configured — remove any localhost entry with `clavus remote remove relay http://localhost:7890`.

### I have two remotes both named "relay." Is that a problem?

Yes — the localhost one (`http://localhost:7890`) will fail on Windows because the relay runs on the Mac, not your machine. Remove it:
```bash
clavus remote remove relay http://localhost:7890
```

### Can I use this without a collaborator?

Absolutely. Clavus works great solo:
- **Snapshots** — version control for your `.als` file. Roll back to any checkpoint.
- **Cues** — leave yourself notes at specific timeline positions. Injects as Ableton markers.
- **Diffs** — see what changed between snapshots (tracks, devices, clips).
- **Backups** — rotating index backups and daily full-store archives.

### Where are my files?

Everything lives under `~/Clavus/` (macOS/Linux) or `C:\Users\<you>\Clavus\` (Windows):
- `Projects/` — your `.als` files organized by project (and `Stems/` folder per project)
- `store/` — snapshots, cue data, and sync metadata
- `backups/` — automatic daily backups and manual archive files

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
clavus repair           # fix corrupted index (restores from backup)
clavus backups          # list available store backups
:status                 # connection status (in TUI)
curl http://<relay-ip>:7890/api/ping   # relay reachability
```

## Features

- **Snapshot version control** — content-addressed snapshots with diffs (tracks, devices, clips, markers)
- **Cues** — threaded comments pinned to timeline positions, injected as Ableton markers
- **P2P sync** — push/pull over Tailscale via a shared relay
- **Optimistic locking** — 409 conflict protection prevents overwriting collaborators' work
- **Conflict detection & resolution** — ⚠ warns on concurrent cue edits, `!` to pick a winner
- **Stem sync** — content-addressed WAV file push/pull alongside snapshots
- **Sample path rewriting** — Ableton finds samples immediately after pull (macOS)
- **Snapshot restore** — roll back to any saved checkpoint
- **Auto-snapshot before pull** — never lose local changes
- **Freeze detection** — warns before snapshotting frozen tracks (cross-platform crash risk)
- **Index backup & recovery** — rotating `.bak` files, daily full-store archives, auto-restore
- **Atomic push ordering** — snapshots → cues; partial failure = clean stop, no half-state
- **Network retry** — automatic retry with backoff on transient failures
- **TUI dashboard** — keyboard-driven, Ableton-style dark theme
- **Auto-snapshot daemon** — file watcher for automatic checkpoints while you work

## Architecture

```
clavus/
├── clavus/
│   ├── parser.py         # .als XML parser
│   ├── store.py          # BlobStore, snapshots, diff engine, index backup/recovery
│   ├── cues.py           # Cue CRUD + Ableton marker injection + conflict detection
│   ├── config.py         # User config
│   ├── helpers.py        # Shared utilities
│   ├── watch.py          # File watcher daemon (auto-snapshot)
│   ├── sync.py           # P2P sync over HTTP (atomic push, retry, optimistic lock)
│   ├── web.py            # FastAPI relay server (per-project mutex, 409 protection)
│   ├── visual_diff.py    # Clip-level ASCII timeline diff
│   ├── tui.py            # Textual terminal dashboard
│   └── cli.py            # CLI entry point
├── pyproject.toml
```

## License

MIT
