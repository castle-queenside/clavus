# Clavus — Collaborator Quickstart

**You got a link from a producer. Here's what to do.**

---

## Before You Start

You need three things:

1. **Tailscale** installed — this is how your computer talks to theirs securely
2. **Clavus** installed — the collaboration tool
3. The URL from your collaborator (something like `http://your-machine.tailXXXX.ts.net:7891`)

No accounts. No cloud. Just these three things.

---

## Step 1: Install Tailscale (5 minutes, one-time)

Go to [tailscale.com/download](https://tailscale.com/download) and download the installer for your OS.

- **Mac:** Open the `.pkg` and click through. It'll appear in your menu bar.
- **Windows:** Run the `.msi` installer. It'll appear in your system tray.

Open Tailscale and sign in with a Google/Microsoft/GitHub account. Free tier is fine — that's what everyone uses.

> **⚠️ Cross-account note:** If you and the host use different Tailscale accounts, the host needs to share their machine with you. They do this at [admin.tailscale.com](https://login.tailscale.com/admin/machines) → their machine → Share → your email. You'll get an invite. Accept it.

---

## Step 2: Install Clavus (one-time)

Open **Terminal** (Mac) or **Windows Terminal / PowerShell** (Windows).

Paste these lines one at a time, pressing Enter after each:

```bash
git clone https://github.com/castle-queenside/clavus
cd clavus
```

**Mac:**
```bash
pip3 install -e .
```

**Windows:**
```powershell
py -m pip install -e .
```

Then run the setup wizard:
```bash
clavus setup          # Mac
py -m clavus setup    # Windows (if 'clavus' isn't recognized)
```

It'll ask your name and a couple preferences. Just press Enter for defaults.

---

## Step 3: Join the Session

Run this in the terminal, using the URL your collaborator sent:

```bash
clavus join http://your-machine.tailXXXX.ts.net:7891   # Mac
py -m clavus join http://your-machine.tailXXXX.ts.net:7891   # Windows
```

You'll see something like:

```
🎹  Clavus — Join a Collaboration Session
══════════════════════════════════════════

🔍  Checking prerequisites...
    ✅  Clavus 0.8.0-beta
    ✅  Ableton detected
    ✅  Tailscale running (100.x.x.x)
    ✅  Storage ready

🔗  Connecting to your-machine.tailXXXX.ts.net:7891...
    ✅  Connected — relay is online

📦  3 project(s) available:

    1. Ja More Mon Amore       (last push: May 10)
    2. Late Night Idea          (last push: May 9)
    3. Remix WIP                (last push: May 8)

💾  Pulling all 3 projects...

    ✅ Pulled — 12 snapshots, 8 cues total

▶️   Ready to work! Run: `clavus tui` (Windows: `py -m clavus tui`)
```

If anything fails, the error message will tell you exactly what's wrong and how to fix it.

---

## Step 4: Open the Dashboard

```bash
clavus tui          # Mac
py -m clavus tui    # Windows
```

You'll see the Clavus TUI:

```
~▼~  Ja More Mon Amore  3 cues  ●  win
─────────────────────────────────────────
Cues ────────────────────────────────────
  1  [pending]  @1:23  Kick too loud?     Chris
  2  [pending]  @3:45  Vox double here?   Chris
  3  [resolved] @0:12  Intro level check  you

History ─────────────────────────────────
  abc1234  May 10 14:22  Arrangement pass 3
  def5678  May 10 13:15  Bassline tweaks
─────────────────────────────────────────
c:new cue  e:edit  s:snapshot  p:pull  P:push  ?:help  q:quit
```

---

## Step 5: Daily Workflow

Here's what you do in a normal session:

| Step | Key | What it does |
|------|-----|-------------|
| 1. Start | `clavus tui` / `py -m clavus tui` | Open the dashboard |
| 2. Pull latest | `p` | Get your collaborator's changes (auto-saves your work first) |
| 3. Work in Ableton | (switch to Live) | Make your changes |
| 4. Snapshot | `S` | Save a checkpoint of what you've done |
| 5. Push | `P` | Send your changes to your collaborator |

That's it. `p` → work → `S` → `P`.

### Adding cues (timeline comments)

Press `c`, type your note, add a position like `@1:23`. These appear as markers in Ableton after the host runs `:inject`.

### Replying to cues

Arrow keys to select a cue, press `r`, type your reply.

### Opening the project in Ableton

Press `o` — Clavus opens the latest version of the `.als` in Ableton Live.

---

## Keybindings Reference

| Key | Action |
|-----|--------|
| `c` | New cue (timeline comment) |
| `S` | Snapshot (save checkpoint) |
| `p` | Pull latest from collaborator |
| `P` | Push your changes |
| `r` | Reply to selected cue |
| `e` | Edit cue or snapshot message |
| `a` | Assign cue to someone |
| `R` | Resolve / unresolve cue |
| `!` | Resolve sync conflict |
| `i` | Inject cues as Ableton markers |
| `T` | Restore to snapshot |
| `d` | Diff (see what changed) |
| `o` | Open project in Ableton |
| `Tab` | Switch between cues / history |
| `j` / `k` | Move up / down |
| `:` | Command mode (type commands) |
| `?` | Help screen |
| `q` | Quit |

---

## Common Commands

Type `:` then the command:

| Command | What it does |
|---------|-------------|
| `:pull` | Pull latest changes |
| `:push` | Push your changes |
| `:push!` | Force push (overwrite — use only if told to) |
| `:snapshot message` | Snapshot with a message |
| `:inject` | Inject cues as Ableton markers |
| `:project name` | Switch to a different project |
| `:projects` | Pick from a list of projects |
| `:remotes` | Manage connection targets |

---

## Troubleshooting

**"clavus isn't recognized" (Windows)**
`pip install -e .` on Windows binds to the exact path where you ran it. If `clavus` commands don't work, re-run from the repo folder:

```powershell
cd C:\path\to\clavus
py -m pip install -e .
```

Or just use the `py -m clavus` form directly for all commands — it always works regardless of install location.

**"Connection refused" or "Cannot reach"**
- Is the host running `clavus share`?
- Is Tailscale running? Check with `tailscale status`
- If cross-account: did the host share their machine with you?

**"HEAD has moved — pull first"**
Someone pushed while you were working. Press `p` (your work is auto-saved), then push again.

**"⚠" on a cue or snapshot**
Both of you edited the same thing. Press `!` to pick which version to keep.

**Nothing happens when I press `o` to open Ableton**
Ableton wasn't auto-detected. The sync still works fine — just open the `.als` file manually from your Clavus projects folder.

**How do I update Clavus?**
```bash
cd clavus
git pull
pip install -e .    # (Windows: py -m pip install -e .)
```

---

## Need Help?

Ask your collaborator. They're the host — they can see the relay status and diagnose connection issues. If something's broken on your end, the error messages are designed to tell you exactly what to fix.
