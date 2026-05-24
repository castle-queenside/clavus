# Clavus — Version Control for Ableton Live

Snapshots, cues, and sync for your Ableton projects. No cloud. No plugins. No accounts.

- **Solo** — save checkpoints, diff what changed, restore any version in one keypress
- **Together** — push snapshots to a shared relay, pull your collaborator's changes, resolve conflicts cleanly
- **Cross-platform** — Mac ↔ Windows, studio ↔ home, works with Tailscale or LAN

[Get started in 2 minutes](#quick-install) &darr;

```
  Mac Studio     push/pull      ┌──────────┐      push/pull     Windows PC
  (producer)  ────────────────→ │  Relay   │ ←──────────────── (co-producer)
              ←──────────────── │ (host)   │ ────────────────→
                                └──────────┘
                                     │
                               stores snapshots,
                               cues, stems
```

One person runs the relay (`clavus share`). Others connect (`clavus join`), push snapshots up, pull changes down. No cloud.

**The relay runs on someone's machine.** When that machine goes to sleep or shuts down, the relay goes with it. Push and pull only work when the relay is reachable. This is not a cloud service — there's no always-on server unless you put the relay on one (a VPS, a Raspberry Pi, an old laptop that stays on).

> **Status: early but active.** Clavus came together through late nights and real studio sessions — a small team of producers who needed this to exist, building and testing it across Mac and Windows between sessions. The TUI is functional, not beautiful, and you may hit rough edges. We fix bugs quickly and iterate constantly. See [Contributing](#contributing) below.

---

## What You Get

| What | How |
|------|-----|
| **Snapshots** | `S` — saves a content-addressed checkpoint with the full `.als` file, parent chain, and integrity verification |
| **Timeline cues** | `c` — pinned comments at specific bars/beats; inject as Ableton markers |
| **Push / Pull** | `P` / `p` — sync snapshots and cues through a relay |
| **Stem sync** | `U` — upload/download WAV stems through the relay |
| **Conflict resolution** | ⚠ warns when both sides edited the same thing; `!` to pick whose |
| **Restore** | `T` — roll back to any previous snapshot |
| **Diff** | `d` — see exactly what changed between snapshots (tracks, devices, clips) |
| **Backup** | `clavus backup` — full store archive with auto-rotating index backups |
| **Health check** | `clavus doctor` — read-only store integrity check and diagnostics |

**Platform:** macOS, Windows · **DAW:** Ableton Live 11+ (Suite/Standard/Intro)  
**Requirements:** Python 3.10+, [Tailscale](https://tailscale.com/download) (free tier)

---

## Quick Install

**Requirements:** Python 3.10+, [Tailscale](https://tailscale.com/download) (free tier). Takes about 2 minutes.

```bash
# 1. Grab the code
git clone https://github.com/castle-queenside/clavus
cd clavus

# 2. Install
pip install -e .          # Windows: py -m pip install -e .

# If 'clavus' isn't recognized after install, use:
# Windows: py -m clavus <command>
# Mac/Linux: python -m clavus <command>

# 3. Add a project and open the dashboard
clavus init /path/to/your/project.als
clavus tui
```

> **Why Tailscale?** It creates a secure, private network between you and your collaborators without configuring routers or firewalls. Install it, sign in with any account (Google, GitHub, etc.), and Clavus picks it up automatically.

Press `S` to snapshot, `c` to add a cue, `T` to restore. Full walkthrough below.

---

## Starting a Project

### Solo — Import an existing Ableton project

```bash
clavus init /path/to/your/project.als
clavus tui
```

Now press `S` to snapshot whenever you've made changes worth keeping. Press `T` to restore any previous snapshot. Press `d` to diff two snapshots.

### Solo — Daily workflow

```
clavus tui          # open dashboard
p                   # pull latest (nothing to pull solo, but good habit)
... work in Ableton ...
S                   # snapshot — saves a checkpoint
```

### With a Collaborator — One person hosts

The collaborator who wants to share chooses a port (e.g. 7891) and runs the relay:

```bash
# Start the relay (keeps running; use a separate terminal window)
clavus share --port 7891

# Check it's live
curl http://localhost:7891/api/ping
# → {"status":"ok"}
```

Share your Tailscale URL with your collaborator. Clavus prints it when the relay starts — it looks like `http://your-machine.tailXXXX.ts.net:7891`.

> **Cross-account?** If you and your collaborator use different Tailscale accounts, go to [admin.tailscale.com](https://login.tailscale.com/admin/machines) → your machine → Share → their email. They must accept the invite before they can connect. Raw `100.x.x.x` IPs are blocked between different Tailscale accounts.

> **If you want to use port 80/443** (cleaner URL, no port number), Tailscale can proxy:
> `tailscale serve --bg --http 80 http://localhost:7891`
> This is optional. The direct port works fine.

### With a Collaborator — Everyone else joins

```bash
# Paste the URL your collaborator sent you (use the same port they chose)
clavus join http://their-name.tailXXXX.ts.net:7891

# Pull all their projects
clavus pull

# Open the dashboard
clavus tui
```

That's it. Clavus shows you everything they have available.

---

## What It Looks Like

![Clavus TUI dashboard with cues and snapshot history](docs/screenshots/tui-main.jpg)

Dashboard with cue list (left) and snapshot history (right).

![Clavus diff viewer showing track-level changes](docs/screenshots/tui-diff.jpg)

Diff view: see exactly what changed between snapshots — tracks added, devices changed, clips moved.

---

## Daily Collaboration Workflow

Open the dashboard:
```bash
clavus tui
```

Then each session:
```
p          → pull your collaborator's latest changes (your work is auto-snapshotted first)
... work in Ableton ...
S          → save a snapshot of your work
P          → push your changes to the relay
```

Your collaborator does the same from their machine. Whoever runs the relay needs to leave it running for push/pull to work. If you're both working on a session, pick whose machine stays on. If you want the relay available 24/7, put it on an always-on machine (a VPS, a Raspberry Pi, an old laptop).

### Adding timeline comments (cues)

Press `c`, type your note, and optionally add a position like `@1:23` (bar 1, beat 3) or `@3:45:20` (minutes:seconds:frames). When your collaborator runs `:inject`, those cues appear as markers inside Ableton at the right position.

```
c          → new cue
e          → edit selected cue
r          → reply to selected cue
a          → assign cue to a collaborator
R          → mark cue resolved / unresolved
!          → resolve a sync conflict (both edited same cue)
```

### Opening a project in Ableton

Press `o` — Clavus opens the latest synced version in Ableton Live. Press `o` while a snapshot is selected in the history pane to open that specific version.

### Inject cues as Ableton markers

Press `:` then type `inject`. Clavus adds your cue comments as named markers in the Ableton arrangement at each cue's timestamp.

---

## Keyboard Reference

<details>
<summary>Press <code>?</code> in the dashboard at any time — or expand this section.</summary>

### Navigation

| Key | What it does |
|-----|-------------|
| `j` / `k` | Move down / up in the list |
| `Tab` | Switch between **Cues** pane and **History** pane |
| `:` | Enter command mode |

### Cues

| Key | What it does |
|-----|-------------|
| `c` | New cue — type your note, optionally `@bar.beat` for position |
| `e` | Edit selected cue text or snapshot message |
| `r` | Reply to selected cue |
| `a` | Assign cue to a collaborator |
| `R` | Toggle resolved / unresolved |
| `!` | Resolve conflict (both edited same cue — pick yours or theirs) |
| `i` | Inject all cues as Ableton markers |

### Snapshots & Sync

| Key | What it does |
|-----|-------------|
| `S` | Snapshot — save a checkpoint now |
| `P` | Push — send your changes to the relay |
| `p` | Pull — get collaborator's changes (auto-snapshots your work first) |
| `d` | Diff selected snapshot against its parent |
| `T` | Restore — roll back to the selected snapshot |
| `o` | Open selected snapshot / HEAD in Ableton |

### Projects & Remotes

| Key | What it does |
|-----|-------------|
| `:project <name>` | Switch to a different project |
| `:projects` | Pick from a list of all synced projects |
| `:remotes` | Manage relay connections |
| `:init /path/to/file.als` | Add a new project to Clavus |

### Commands

| Command | What it does |
|---------|-------------|
| `:inject` | Add cues as Ableton markers |
| `:push!` | Force push — bypass conflict check (use when told to) |
| `:snapshot <message>` | Snapshot with a custom message |
| `:pull` | Pull latest changes |
| `:stem push` | Push WAV stems to relay |
| `:stem pull` | Pull WAV stems from relay |
| `:projects` | Show all projects |
| `:remotes` | Show all remotes |
| `:freeze` | Toggle frozen track warning (warn / block / allow) |
</details>

---

## Command-Line Reference

```bash
# Setup / first run
clavus setup                 # guided first-run wizard

# Projects
clavus init /path/to.als     # add a project
clavus tui                   # open the dashboard
clavus projects              # list all projects

# Sync
clavus pull                  # pull snapshots, cues, stems from relay
clavus push                  # push snapshots, cues, stems to relay
clavus status                # show current project and sync status

# Snapshots & cues
clavus snapshot "message"   # save checkpoint with a note
clavus history               # list snapshots for current project

# Stems
clavus stem import-folder ~/Desktop/Stems/   # import WAVs, AIFFs from folder
clavus stem import Kick.wav --track "Kick"    # import single file
clavus stem push                              # push to relay
clavus stem pull                              # pull from relay

# Diagnostics
clavus doctor                  # read-only store health check
clavus status                  # show relay connection status

# Backup & repair
clavus backup                  # full store archive
clavus backups                 # list backups
clavus restore-store <file>    # restore from backup
clavus repair                  # fix corrupted index

# Relay
clavus share --port 7891       # start hosting a relay
clavus join <url>              # connect to a relay

# Help
clavus help                   # show all commands
```

---

## If Something Goes Wrong

### "HEAD has moved — pull first"
Someone pushed while you were working. Your work is safe — Clavus auto-snapshotted it. Press `p` to pull their changes, then press `P` to push yours.

### "⚠ on a cue or snapshot"
Both of you edited the same thing simultaneously. Press `!` and pick **yours** or **theirs**.

### "clavus isn't recognized" (Windows)
`pip install -e .` on Windows can fail silently if you ran it from the wrong directory — it binds to the exact path where you executed it. If `clavus` commands don't work after install:

```powershell
# Navigate to your cloned repo first, THEN install
cd C:\path\to\clavus
py -m pip install -e .

# Or use the module form directly (always works)
py -m clavus share
py -m clavus init
py -m clavus tui
```

### "Connection refused" / "Cannot reach relay"
- Is the host still running `clavus share`?
- Is Tailscale running? Run `tailscale status` to check.
- If you're on different Tailscale accounts: did the host share their machine with you at [admin.tailscale.com](https://login.tailscale.com/admin/machines)? You must accept the invite.

### "Nothing shows up — no project"
You haven't added the project yet. Run `clavus join <url>` to connect to the relay, then `p` to pull the project. Or add one manually with `clavus init /path/to/project.als`.

### "Frozen track warning"
Ableton Live crashes when opening projects with frozen tracks from a different OS. When you see this warning, unfreeze the tracks in Ableton before snapshotting, or press `S` with `--allow-frozen` if everyone is on the same OS.

### "404 on install.ps1" (Windows)
GitHub's CDN sometimes takes a few minutes to pick up newly pushed files. If `irm https://raw.githubusercontent.com/.../install.ps1 | iex` fails, use the git clone approach instead:

```powershell
git clone https://github.com/castle-queenside/clavus
cd clavus
py -m pip install -e .
clavus setup
```

---

## Updating Clavus

```bash
cd clavus
git pull
pip install -e .     # Windows: py -m pip install -e .
```

---

## Known Limitations

**OneDrive / Files On-Demand:** Ableton has trouble with `.als` files in OneDrive-synced folders. Keep projects on a local drive.

**Single relay at a time:** `clavus join` replaces any existing remote. Your snapshots and history stay local regardless.

**No two-person-at-once push protection:** Simultaneous pushes may get a 409. Clavus handles it gracefully — pull first, then push again.

---

## Contributing

Clavus is MIT-licensed and open to contributions. Here's how to help:

- **Report bugs** — open an [issue on GitHub](https://github.com/castle-queenside/clavus/issues). Include what you were doing, what you expected, and what happened.
- **Suggest improvements** — the workflow makes sense to us because we built it. If something's confusing or missing, tell us.
- **Submit code** — PRs welcome. Keep changes focused, add tests if applicable, and match the existing code style.
- **Spread the word** — if Clavus saves you time, tell another producer.

This is a small project built by a small team of producers who needed it. Every issue filed, every feature request, every "this part was confusing" helps make it better.

---

## Support Clavus

If Clavus saves you time, frustration, or a lost arrangement, consider tossing a few dollars our way.

- Cash App: `$slowhandschris`
- Venmo: `@chrisandcarr`

No tiers, no rewards, no subscriptions. Just a donation if you find it useful.

---

## License

MIT — see [LICENSE](LICENSE).