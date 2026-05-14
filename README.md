# Clavus — DAW Project Versioning & Collaboration

**Version:** 0.1.0-beta · **Platforms:** macOS, Windows, Linux · **DAWs:** Ableton Live (Reaper, Bitwig coming)

Snapshots, timeline-anchored discussion cues, push/pull sync over Tailscale, and a keyboard-driven terminal dashboard. No cloud, no plugins, no accounts. Works solo. Works with your crew.

---

## Quick Install

```bash
git clone https://github.com/castle-queenside/clavus
cd clavus
pip install -e .            # macOS/Linux
# Windows:   py -m pip install -e .
clavus setup                # guided first-time wizard
clavus tui                  # open the dashboard
```

That's it. You're ready to work alone or join a session.

---

## Solo Mode

No setup needed beyond the quick install. Every project you open gets:

- **Snapshots** — content-addressed checkpoints you can restore anytime
- **Cues** — timeline-pinned comments at specific positions (bar.beat or timecode)
- **Diffs** — see exactly what changed between snapshots (tracks, devices, clips, markers)
- **Restore** — roll back to any previous state
- **Backup** — full-store archive with rotating auto-backups

```bash
clavus init /path/to/project.als   # import a project
clavus tui                          # work in the TUI
S                                   # snapshot (save checkpoint)
T                                   # restore to a previous snapshot
```

---

## Collaboration Mode (Relay)

This is the primary path for sharing work. One person runs a **relay**, everyone else **joins** it. Push/pull happens through the relay, so people don't need to be online at the same time.

```text
┌─────────┐    push/pull    ┌──────────────┐    push/pull    ┌─────────┐
│   You   │ ◄─────────────► │    Relay     │ ◄─────────────► │  Peer   │
└─────────┘                 └──────────────┘                 └─────────┘
```

### Host Setup (do once)

```bash
# 1. Start the relay
clavus share --port 7891

# 2. Expose it on your Tailscale network (one-time, survives reboot)
tailscale serve --bg --http 7890 http://localhost:7891

# 3. Verify it's live
curl http://$(hostname).tailXXXX.ts.net:7890/api/ping
# → {"status":"ok"}
```

Send your collaborator the URL: `http://your-machine.tailXXXX.ts.net:7890`

> **Cross-account?** Share your machine from [admin.tailscale.com](https://login.tailscale.com/admin/machines) → your machine → Share → their email. They **must accept** the invite. Raw `100.x.x.x` IPs are blocked for cross-account TCP.

### Joining a Session

```bash
clavus join http://host.tailXXXX.ts.net:7890
clavus pull      # sync all projects
clavus tui       # open the dashboard
```

### Daily Workflow

```
p → work in Ableton → S → P
```

| Key | Action |
|-----|--------|
| `p` | Pull latest (auto-snapshots your local work first) |
| `S` | Snapshot (save a checkpoint) |
| `P` | Push your changes to the relay |
| `i` | Inject cues as Ableton markers |

Full keybinding reference inside the TUI with `?` or `h`.

---

## If Something Goes Wrong

**"HEAD has moved — pull first."**
Someone else pushed while you were working. Press `p` (your work is auto-snapshotted), then push again. Nothing lost.

**"⚠ on a cue."**
Both people edited the same cue. Press `!` to pick your version or theirs.

**"Frozen track warning."**
Frozen tracks crash Ableton cross-platform. Unfreeze before snapshotting, or pass `--allow-frozen` if everyone's on the same OS.

**"Nothing shows up. It says no project."**
You need to init or join: `:init /path/to/project.als`, or `clavus join http://<url>:7890` then `p` to pull.

**"Connection refused."**
Is the relay running? Is Tailscale on? If cross-account, is the machine shared?

---

## Feature Overview

| Capability | How |
|------------|-----|
| Snapshots | `S` — content-addressed, includes raw .als backup |
| Cues | `c` — threaded timeline comments, injectable as DAW markers |
| Sync | `p` / `P` — push/pull over Tailscale relay (or direct P2P) |
| Stem sync | `U` / `:stem pull` — content-addressed WAV transfer |
| Conflict handling | ⚠ warns on concurrent edits, `!` to resolve |
| Restore | `T` — roll back to any snapshot |
| Backup | `clavus backup` — full-store archive, auto-rotating index backups |
| Freeze detection | Warns before snapshotting frozen tracks |
| Force push | `:push!` — bypass conflict detection when needed |

---

## Known Limitations

**Ableton Suite vs Intro/Standard:** Clavus snapshots the raw `.als` file (GZIP-compressed XML). Suite-only features are preserved as binary data inside the `.als`, but Intro/Standard can't decode them. Verify a restored `.als` opens before deleting the original.

**OneDrive / Files On-Demand:** If `.als` files live in a OneDrive-synced folder, Ableton may have trouble reading/writing them. Keep projects on a local drive.

**Single remote at a time:** `clavus join` replaces any existing remote. Your snapshots and history remain local.

**No concurrent push protection:** Two people pushing at the exact same instant may get a 409. Clavus handles it gracefully, but best practice is to pull before pushing.

^ See post, referenced.

---

## Stems

```bash
clavus stem import-folder ~/Desktop/Stems/   # import all WAVs
clavus stem push                              # upload to relay
clavus stem pull                              # download from relay
```

TUI: `U` to push, `:stem pull` to pull. Files land in `~/.clavus/stems/<project>/<hash>/`.

---

## Backup

```bash
clavus backup                  # full store archive (tar.gz)
clavus backups                 # list backups
clavus restore-store <file>    # restore from backup
clavus repair                  # fix corrupted index (auto-restores from .bak)
```

Rotating index backups run automatically before every write. Full daily archives too.

---

## Roadmap

**v1.0** (next)
- Reaper adapter (`.rpp` — plain text, markers map to cue positions)
- Max for Live integration (snapshot/push/pull from inside Ableton)
- DAW-agnostic project detection
- Phone dashboard companion
- Stem sync hardening (partial transfers, resume)
- Linux end-to-end testing

**v1.5+**
- Bitwig adapter (`.bwproject`)
- FL Studio / Logic Pro adapters
- Web-based diff viewer
- MIDI Remote Script integration

---

## Updating

```bash
cd clavus
git pull
pip install -e .    # Windows: py -m pip install -e .
```

---

## License

MIT
