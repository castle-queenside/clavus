# Clavus — Version & Collaborate on Ableton Projects

**Clavus** gives your Ableton projects time-travel. Snapshot your work, add timeline comments your collaborator can see, and sync everything through a lightweight relay — no cloud, no plugins, no accounts.

Work alone: save checkpoints, diff what changed, restore if you need to.  
Work together: push snapshots to a shared relay, pull their changes, resolve conflicts with one keypress.

**New here?** Start with [Quick Install](#quick-install) below. Takes about 5 minutes.

---

## What You Get

| What | How |
|------|-----|
| **Snapshots** | `S` — saves a content-addressed checkpoint with the full `.als` file |
| **Timeline cues** | `c` — pinned comments at specific bars/beats; inject as Ableton markers |
| **Push / Pull** | `P` / `p` — sync through a Tailscale relay; work offline, sync when ready |
| **Stem sync** | `U` — upload/download WAV stems through the relay |
| **Conflict resolution** | ⚠ warns when both sides edited the same thing; `!` to pick whose |
| **Restore** | `T` — roll back to any previous snapshot |
| **Diff** | `d` — see exactly what changed between snapshots (tracks, devices, clips) |
| **Backup** | `clavus backup` — full store archive with auto-rotating index backups |

**Platform:** macOS, Windows (Linux coming)  
**DAW:** Ableton Live 11+ (Suite/Standard/Intro) — Reaper and Bitwig on the roadmap  
**Requirements:** Python 3.10+, [Tailscale](https://tailscale.com/download) (free tier)

---

## Quick Install

### Step 1 — Install Tailscale (one-time, 2 minutes)

Download at [tailscale.com/download](https://tailscale.com/download). Sign in with Google, Microsoft, or GitHub. That's it — free tier is all you need.

> **Why Tailscale?** Clavus uses it to create a secure, private network between you and your collaborators without configuring routers, ports, or firewalls. It just works.

### Step 2 — Clone Clavus

Open Terminal (Mac) or PowerShell (Windows):

```bash
git clone https://github.com/castle-queenside/clavus
cd clavus
```

### Step 3 — Install

**macOS / Linux:**
```bash
pip3 install -e .
```

**Windows:**
```powershell
py -m pip install -e .
```

### Step 4 — First-Run Setup

```bash
clavus setup
```

The wizard asks for:
- **Your name** — shows up on cues and snapshots
- **Relay port** — default is 7890, almost always fine
- **Projects folder** — where Clavus stores its copy of synced projects (default is fine)
- **Ableton detection** — it finds Live automatically if it's installed

It also detects your Tailscale MagicDNS name (e.g. `slow-hands-studio-1.tail46b8d9.ts.net`) — this is what you share with collaborators so they can join your relay.

### Step 5 — Open the Dashboard

```bash
clavus tui
```

You're ready. Here's what each screen shows and how to navigate.

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

The collaborator who wants to share runs these **once**:

```bash
# Start the relay (keeps running; use a separate terminal window)
clavus share --port 7891

# Expose it on your Tailscale network (one-time, survives reboots)
tailscale serve --bg --http 7890 http://localhost:7891

# Check it's live — you should see {"status":"ok"}
curl http://localhost:7891/api/ping
```

The second command prints your MagicDNS URL (e.g. `http://slow-hands-studio-1.tail46b8d9.ts.net:7890`). **Share this URL with your collaborator.**

> **Cross-account?** If you and your collaborator use different Tailscale accounts, go to [admin.tailscale.com](https://login.tailscale.com/admin/machines) → your machine → Share → their email. They must accept the invite before they can connect. Raw `100.x.x.x` IPs are blocked between different Tailscale accounts.

### With a Collaborator — Everyone else joins

```bash
# Paste the URL your collaborator sent you
clavus join http://their-name.tailXXXX.ts.net:7890

# Pull all their projects
clavus pull

# Open the dashboard
clavus tui
```

That's it. Clavus shows you everything they have available.

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

Your collaborator does the same from their machine. Neither of you needs to be online at the same time — Clavus queues everything through the relay.

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

## TUI Keybindings

Press `?` at any time to see the full reference.

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
clavus status                # show relay connection status
clavus pull                  # pull from relay
clavus push                  # push to relay
clavus join <url>           # connect to a relay (one URL)
clavus share --port 7891    # start hosting a relay

# Snapshots & cues
clavus snapshot "message"   # save checkpoint with a note
clavus history               # list snapshots for current project

# Stems
clavus stem import-folder ~/Desktop/Stems/   # import WAVs
clavus stem push                              # push to relay
clavus stem pull                              # pull from relay

# Backup & repair
clavus backup                  # full store archive
clavus backups                 # list backups
clavus restore-store <file>    # restore from backup
clavus repair                  # fix corrupted index

# Help
clavus help                   # show all commands
```

---

## If Something Goes Wrong

### "HEAD has moved — pull first"
Someone pushed while you were working. Your work is safe — Clavus auto-snapshotted it. Press `p` to pull their changes, then press `P` to push yours.

### "⚠ on a cue or snapshot"
Both of you edited the same thing simultaneously. Press `!` and pick **yours** or **theirs**.

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

**Ableton Suite vs Intro/Standard:** Clavus snapshots the raw `.als` file. Suite-only features are preserved but Intro/Standard can't decode them. Always verify a restored `.als` opens before deleting the original.

**OneDrive / Files On-Demand:** Ableton has trouble with `.als` files in OneDrive-synced folders. Keep projects on a local drive.

**Single relay at a time:** `clavus join` replaces any existing remote. Your snapshots and history stay local regardless.

**No two-person-at-once push protection:** Simultaneous pushes may get a 409. Clavus handles it gracefully — pull first, then push again.

---

## Roadmap

**v1.0** (coming)
- Reaper adapter (`.rpp` — plain text, markers map directly to cue positions)
- Max for Live integration (snapshot/push/pull from inside Ableton)
- DAW-agnostic project detection
- Phone dashboard companion
- Stem sync hardening (partial transfers, resume)

**v1.5+**
- Bitwig adapter (`.bwproject`)
- FL Studio / Logic Pro adapters
- Web-based diff viewer
- MIDI Remote Script integration

---

## License

MIT — see [LICENSE](LICENSE).