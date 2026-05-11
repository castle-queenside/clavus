# Clavus — DAW Project Versioning & Collaboration

**Version:** 0.8.0-beta  **Platforms:** macOS · Windows · Linux · **DAWs:** Ableton (Reaper, Bitwig coming)

Save points that travel. Snapshots, timeline-anchored cues, push/pull sync over Tailscale, and a keyboard-driven terminal dashboard. No cloud, no plugins, no accounts. Works solo. Works with your crew.

## Quick Start

```bash
git clone https://github.com/castle-queenside/clavus
cd clavus
pip install -e .            # macOS / Linux  (Windows: py -m pip install -e .)
clavus setup                # guided first-time wizard
clavus tui                  # open the dashboard
```

## Two Ways to Use It

### Solo

Just you, your project, and the TUI. Snapshots, cues, diffs, and restores all work locally with zero setup. Great for leaving yourself notes at specific timeline positions and rolling back when an idea doesn't land.

### Collaborate

```
┌─────────┐                    ┌───────────────┐                    ┌─────────┐
│   You   │ ◄── push/pull ───► │     Relay     │ ◄── push/pull ───► │  Peer   │
│  (Mac)  │   via Tailscale    │(any machine)  │   via Tailscale    │  (Win)  │
└─────────┘                    └───────────────┘                    └─────────┘
```

One machine runs the relay. Everyone pushes and pulls through it. The relay is dumb — just stores what's pushed. No cloud, no dedicated server.

**⚠️ Cross-account collaborators (different Tailscale accounts) require three things:**

1. **Share your machine** from [admin.tailscale.com](https://login.tailscale.com/admin/machines) → your machine → Share → their email
2. **They must accept** the invite
3. **Use MagicDNS, not raw IP** — `100.x.x.x` only works same-account. Shared users get blocked on raw TCP

**Host setup (do once):**
```bash
clavus share --port 7891 &                              # start relay
tailscale serve --bg --http 7890 http://localhost:7891   # HTTP proxy for cross-account
```
Verify: `curl http://<your-machine>.tailxxxx.ts.net:7890/api/ping` → `{"status":"ok"}`  
After reboot, re-run `tailscale serve`. The relay auto-starts.

**Everyone else:**
```bash
clavus join http://<magicdns-url>:7890   # NOT the raw IP
clavus pull
clavus tui
```

**Daily:** `p` to pull, work in your DAW, `C` to snapshot, `P` to push.  
Full networking details: `references/tailscale-serve-relay.md`

## Features

- **Snapshots** — content-addressed checkpoints with diffs (tracks, devices, clips, markers)
- **Cues** — threaded timeline-pinned comments, injected as DAW markers
- **Sync** — push/pull over Tailscale with optimistic locking (409 conflict protection)
- **Conflict resolution** — ⚠ warns on concurrent edits, `!` resolves them
- **Restore** — roll back to any snapshot
- **Stem sync** — content-addressed WAV push/pull (only changed files transfer)
- **Auto-snapshot** on pull — never lose local changes
- **Freeze detection** — warns before snapshotting frozen tracks (cross-platform crash risk)
- **Backup & recovery** — rotating `.bak` index backups, daily full-store archives, auto-restore

## Keybindings

| Key | Action |
|-----|--------|
| `c` | New cue |
| `C` | Snapshot |
| `r` | Reply to cue |
| `e` | Edit cue / snapshot message |
| `a` | Assign cue |
| `R` | Resolve / unresolve cue |
| `x` | Archive cue |
| `!` | Resolve sync conflict |
| `T` | Restore snapshot |
| `d` | Diff snapshot |
| `o` | Open project in DAW |
| `p` | Pull (auto-snapshots first) |
| `P` | Push |
| `U` | Push stems |
| `Tab` | Switch cues / history pane |
| `j` / `k` | Navigate |
| `:` | Command mode |
| `?` / `h` | Help |
| `q` | Quit |

## Stems

```bash
clavus stem import ~/Samples/kick.wav
clavus stem push
clavus stem pull
```
TUI: `U` to push, `:stem pull` to pull. Files land in `~/Clavus/Projects/<name>/Stems/`.

## Backup

```bash
clavus backup                  # full store backup (tar.gz)
clavus backups                 # list backups
clavus restore-store <file>    # restore
clavus repair                  # fix corrupted index (auto-restores from .bak)
```

Rotating index backups run automatically before every write. Full daily backups too.

## Roadmap

**v0.9 — First non-Ableton DAW**
- Reaper adapter (`.rpp` — plain text, markers map directly to cue positions)
- DAW-agnostic project detection (no more hardcoded `.als` assumptions)
- Linux end-to-end testing (Reaper + Bitwig both run natively)

**v1.0 — Multi-DAW + polish**
- Bitwig adapter (`.bwproject`)
- Phone dashboard companion (tiny HTTP server for monitoring from your phone)
- Stem sync hardening (partial transfers, resume)
- Auto-snapshot daemon refinements (configurable intervals, smarter change detection)

**Ideas for later**
- FL Studio adapter
- Logic Pro adapter
- Web-based diff viewer for snapshots
- MIDI Remote Script integration (control Clavus from your controller)

## FAQ

**"Nothing shows up. It says no project."**  
Initialize or join: `:init /path/to/project.als`, `:browse` to find one, or `:join http://<url>:7890` then `p` to pull.

**"HEAD has moved — pull first."**  
Someone else pushed. Press `p` (your work is auto-snapshotted), then push again. Nothing lost.

**"⚠ on a cue."**  
Both people edited it. Press `!` to pick your version or theirs. Push after.

**"Frozen track warning."**  
Frozen tracks crash Ableton cross-platform. Unfreeze before snapshotting. Pass `--allow-frozen` if everyone's on the same OS.

**"How do I update?"**  
`cd clavus && git pull && pip install -e .`

## License

MIT
