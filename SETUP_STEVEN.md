# Steven — Clavus Setup Checklist (Windows)

Here's everything you need to do, step by step.

---

## ☐ 1. Install Python

```cmd
winget install Python.Python.3.13
```

Or download from [python.org/downloads](https://www.python.org/downloads/) — **check "Add Python to PATH"** during install.

Verify:
```cmd
python --version
```
→ Should say `Python 3.13.x`

---

## ☐ 2. Install Git

```cmd
winget install Git.Git
```

Or from [git-scm.com](https://git-scm.com/).

Verify:
```cmd
git --version
```

---

## ☐ 3. Install Tailscale

Tailscale creates a secure private network between our machines. No port forwarding, no firewalls.

1. Go to [tailscale.com/download/windows](https://tailscale.com/download/windows)
2. Download and run the installer
3. Sign in with any email (Google, GitHub, Microsoft — doesn't matter)
4. **Find your Tailscale IP:**
   - Hover over the Tailscale icon in your system tray (bottom-right, grey whale)
   - Your IP shows as **100.x.x.x**
   - Or run: `tailscale ip -4`

**→ Send your 100.x.x.x IP to Chris**

---

## ☐ 4. Clone Clavus

```cmd
cd Desktop
git clone https://github.com/slowhands-RSTR/clavus.git
cd clavus
pip install -e .
```

Verify:
```cmd
clavus --help
```
→ You should see a list of commands.

---

## ☐ 5. Connect to Chris

Chris will give you his Tailscale IP. Run this in Command Prompt:

```cmd
clavus remote add chris http://100.126.94.21:7890
```

Test the connection:
```cmd
clavus pull chris
```
→ Should download the Northern Lights project.

---

## ☐ 6. Pull stems (audio files)

```cmd
clavus stem pull chris
```
→ Downloads the WAV audio files. Might take a minute the first time.

---

## ☐ 7. Open in Ableton

The `.als` file is in the `clavus` folder on your Desktop. Open it in Ableton Live. Chris will have bounced everything to audio beforehand, so you shouldn't need any plugins.

---

## ☐ 8. Start the TUI

**This is the main way you'll use Clavus.** It's a terminal dashboard for managing comments (cues), snapshots, and sync.

```cmd
clavus tui
```

---

### TUI quickstart

When the TUI starts, you'll see two panels side-by-side:

| Left (Cues) | Right (History/Snapshots) |
|---|---|
| Threaded comments pinned to timeline positions | List of saved project versions |

**Navigate:**
- `Tab` — switch between the two panels
- `j` / `k` — move up/down in whichever panel has focus
- Arrow keys also work

### Keybinding reference

| Key | Action |
|-----|--------|
| `c` | **New cue** — add a comment pinned to a timeline position |
| `r` | **Reply** — reply to the selected cue |
| `e` | **Edit** — change the text of the selected cue |
| `a` | **Assign** — assign the cue to someone |
| `R` | **Resolve** — mark cue as resolved/done |
| `D` | **Delete** — delete a cue (asks for confirmation) |
| `x` | **Archive** — archive the cue |
| `s` | **Skip** — skip the cue |
| `S` | **Start/stop** — toggle "in progress" on a cue |
| `T` | **Restore** — restore the project to the selected snapshot |
| `d` | **Diff** — show what changed in the selected snapshot |
| `C` | **Snapshot** — save a new checkpoint |
| `p` | **Pull** — get latest from Chris |
| `P` | **Push** — send your changes to Chris |
| `:` | **Command mode** — type commands like `:project <name>` or `:snapshot <message>` |
| `q` | **Quit** — exit the TUI |

### Cue colors tell you the status

- **Teal dot** — pending (open)
- **Green dot** — resolved (done)
- **Yellow ▶** — in progress (actively working on it)
- **Dim/grey** — skipped or archived

---

### Sending changes back

When you've made edits in Ableton and saved:

```cmd
clavus snapshot "my first edit"
clavus push chris
```

To also send audio files:
```cmd
clavus stem import --track "Bass" C:\path\to\bass_bounce.wav
clavus stem push chris
```

---

## Also available: web companion

If you prefer a browser view:

```cmd
clavus serve
```

Then open http://localhost:7890 — shows the same cues and snapshots in a mobile-friendly web UI with tabs (Project / Cues / Snapshots).

---

## Quick reference

| Command | What it does |
|---------|-------------|
| `clavus tui` | **Main interface** — terminal dashboard |
| `clavus pull chris` | Get latest from Chris |
| `clavus push chris` | Send your changes to Chris |
| `clavus stem pull chris` | Download audio stems |
| `clavus stem push chris` | Upload audio stems |
| `clavus snapshot "msg"` | Save a checkpoint |
| `clavus serve` | Web companion at localhost:7890 |
| `clavus log` | See snapshot history |
| `clavus diff --visual` | See what changed in the arrangement |
