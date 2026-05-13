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

Two ways to sync — pick what fits your workflow.

#### Direct P2P (two people, same session)

```
┌──────────┐                      ┌──────────┐
│   You    │ ◄── push/pull ─────► │   Peer   │
│  share   │    via Tailscale     │  share   │
└──────────┘                      └──────────┘
```

Both machines run `clavus share`. Each discovers the other and pushes/pulls directly. No relay machine, nobody has to be "the host." Both need to be online at the same time.

```bash
# Both people do this:
clavus share &                                       # each runs their own relay
clavus find --tailscale                              # discover each other
clavus remote add <their-name> http://<their-url>    # add each other
clavus tui                                           # work + S + P as usual
```

**⚠️ Cross-account?** Share machines + use MagicDNS (see Tailscale note below).

#### Shared relay (any number, async)

```
┌─────────┐              ┌──────────────┐              ┌─────────┐
│   You   │ ◄── push ──► │    Relay     │ ◄── push ──► │  Peer   │
└─────────┘              └──────────────┘              └─────────┘
```

One machine runs the relay. Everyone else joins. The relay stores everything — people can push and pull at different times. Good for 3+ people or when not everyone is online at once. No cloud, no dedicated server — the relay is just someone's machine.

**Relay host (do once):**
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

**Daily (both modes):** `p` to pull, work in your DAW, `S` to snapshot, `P` to push.

#### Tailscale note (cross-account)

If you and your collaborator use different Tailscale accounts:

1. **Share your machine** from [admin.tailscale.com](https://login.tailscale.com/admin/machines) → your machine → Share → their email
2. **They must accept** the invite
3. **Use MagicDNS, not raw IP** — `100.x.x.x` only works same-account. Shared users get blocked on raw TCP

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
| `S` | Snapshot |
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
clavus stem import-folder ~/Desktop/Stems/   # import all WAVs in one shot
clavus stem import kick.wav --track "Kick"    # single file
clavus stem push
clavus stem pull
```
TUI: `U` to push, `:stem pull` to pull. Files land in `~/.clavus/stems/<project>/<hash>/`.

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
- Max for Live integration (snapshot/push/pull from inside Ableton — no terminal switching)
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
Initialize or join: `:init /path/to/project.als`, or `clavus join http://<url>:7890` then `p` to pull.

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
