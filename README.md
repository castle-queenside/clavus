# Clavus — Version Control for DAW Projects

**Single 4MB binary. No cloud. No plugins. No accounts.**

Snapshots, cues, and sync for Ableton Live and Bitwig Studio. Works solo or with a collaborator over LAN.

```bash
# Install: just download the binary
curl -L https://github.com/castle-queenside/clavus/releases/latest/download/clavus -o clavus
chmod +x clavus

# Snapshot your session
./clavus snapshot ~/Music/Ableton/MySet.als -m "intro section done"

# Open the TUI
./clavus tui --dir ~/Music/Ableton
```

> ⚠️ **Status: Rust rewrite in progress.** The Python version (`main` branch) is production-ready. This `rust` branch is the next generation — pure Rust, 4MB binary, same features. See [#13](https://github.com/castle-queenside/clavus/pull/13).

---

## The Problem

Music production versions look like this:

```
Final_v3_Chris_REALFINAL_MASTERED_USE_THIS.als
Final_v3_Chris_REALFINAL_MASTERED_USE_THIS_OLD.als
Final_v3_Chris_REALFINAL_MASTERED_USE_THIS_OLDFASTER.als
```

- No history. No diff. No rollback.
- Collaboration means WeTransfer and praying.
- Nobody knows what changed between versions.

## The Solution

Clavus watches your project files and maintains a content-addressed snapshot chain — like git, but built for DAW workflows.

```
┌─────────────────────────────────────────────────────────────────────┐
│                         clavus tui                                  │
├────────────────────────────────────────────────────────────────────┤
│ projects          ALS  Bass Session      3·a1b2c3d4  128  12  2d │
│                   ALS  Lead Idea        1·f4e5d3c2  120   8  5d │
│                   BW   Bridge Module    —            100  24  1h  │
├────────────────────────────────────────────────────────────────────┤
│  HASH       MESSAGE                   BPM  TRACKS  SAMPLES  WHEN  │
│  a1b2c3d4   intro section done         128  12     47      2d   │
│  f4e5d3c2   added bass layer           128  11     44      4d   │
│  3d2c1b0a   initial session            120   8     31      1w   │
├────────────────────────────────────────────────────────────────────┤
│  ↑↓ navigate  ·  Enter open  ·  s sync  ·  q quit                │
└────────────────────────────────────────────────────────────────────┘
```

**What you get:**

| Feature | What it does |
|---------|-------------|
| **Snapshot** | Save a content-addressed checkpoint. SHA256 of `.als`/`.bwproject` bytes. Duplicates detected automatically. |
| **Audio audit** | Before every snapshot: verify all sample references are present and inside the project folder. No silent missing samples. A Bitwig bundle's own samples count as present, and `collect` keeps the bundle self-contained. A path that has moved is looked up through a map you write, so a renamed drive is not a lost library. |
| **Your set is yours** | `collect` is the only command that writes to a set, and it rewrites the paths inside it. `clavus collect --dry-run` says what it would do and changes nothing. Everything else — audit, log, status, snapshot, push, pull, restore — leaves the set alone. |
| **Cue markers** | Pin timeline comments at bar positions. Filter by Pending / Resolved / Skipped / Deferred / Archived. Each note records who wrote it. |
| **Relay sync** | Push/pull snapshots over TCP to a collaborator's relay. No cloud. |
| **Diff** | Walk the chain and see what changed at each step: tempo, track count, every track that was added, removed or renamed, and the sample manifest. `clavus diff --live` compares the set on disk with your last snapshot, so you can see what you have done since. A track is only reported as renamed when the evidence is clear — otherwise it is reported as a removal and an addition, so a lost track is never hidden. |
| **Restore** | Roll back to any snapshot — restore the raw project bytes from the blob store. A snapshot only ever restores within its own project. |
| **Project isolation** | Every project keeps its own chain, cues and history. One project's work never lands in another's — not even when two projects share a folder *name*. |

---

## Quick Install

### Binary (recommended)

```bash
# macOS/Linux — download the latest release
curl -L https://github.com/castle-queenside/clavus/releases/latest/download/clavus -o clavus
chmod +x clavus

# Verify
./clavus --version
```

### From source

```bash
git clone https://github.com/castle-queenside/clavus
cd clavus/rust
cargo build --release
./target/release/clavus --help
```

### Requirements

- **macOS** or **Linux** (Windows: coming soon)
- An **Ableton Live** or **Bitwig Studio** project
- No Python, no plugins, no accounts

---

## CLI Reference

```bash
# Snapshot a project (audits samples first)
./clavus snapshot ~/Music/Ableton/MySet.als -m "bridge section done"

# List the snapshot chain
./clavus log ~/Music/Ableton/MySet.als

# See what changed — the chain, one step at a time
./clavus diff ~/Music/Ableton/MySet.als

# See what you have changed since your last snapshot
./clavus diff --live ~/Music/Ableton/MySet.als

# Compare any two snapshots
./clavus diff ~/Music/Ableton/MySet.als --from a1b2c3d4 --to e5f6a7b8

# Add a cue marker (the position is bars.beats.sixteenths)
./clavus cue ~/Music/Ableton/MySet.als -t "drop bass at bar 17" -p 17.1.0 --track "Bass"

# List cues
./clavus cues ~/Music/Ableton/MySet.als

# Audit samples (check for missing files)
./clavus audit ~/Music/Ableton/MySet.als

# Run the sync relay (your machine is the server)
./clavus relay 127.0.0.1:4477

# Push to / pull from a relay
./clavus push --relay 192.168.1.10:4477 ~/Music/Ableton/MySet.als
./clavus pull --relay 192.168.1.10:4477 ~/Music/Ableton/MySet.als

# Bring an older Clavus store into this one
./clavus import-legacy --store ~/.clavus --projects ~/Clavus/Projects

# Say where a path a set points at has moved to
./clavus relocate map "/Volumes/Old Disk" "/Volumes/LaCie"
./clavus relocate list

# Work out which rules would put the missing samples back
./clavus relocate suggest --root /Volumes/LaCie ~/Music/Ableton/MySet.als

# See what collecting would do, before it touches your set
./clavus collect --dry-run ~/Music/Ableton/MySet.als

# Open the TUI
./clavus tui --dir ~/Music/Ableton
```

---

## TUI Keybindings

```
↑↓         Navigate lists
Enter/o    Open project
l          Cue list
n          Snapshot list
s          Sync screen
a          Add cue (in CueList)
r          Resolve cue
f          Cycle filter (Pending → Resolved → Skipped → Deferred → Archived → all)
c          Connect relay (in Sync screen)
r          Relay address (in Sync screen)
P          Push
G          Pull
/          Filter the project list
S          Cycle the project sort (recent → name → history → open notes)
Esc/q      Back / quit
```

### The mouse

Clavus is keyboard-first, and the keyboard still does everything. The
mouse is a second way in, not the only way — nothing is reachable by
mouse that is not reachable by keyboard.

| Gesture | What it does |
|---------|--------------|
| Wheel | Moves the selection, and the list scrolls to keep it on screen |
| Click a row | Selects it |
| Double click a row | Opens it — the same as `Enter` |
| Drag with the button held | The row under the pointer becomes the selected one |
| Click the scrollbar | Jumps the window to that part of the list |

The mouse is told what is on the screen by the renderer, which records
a map of the rows it drew. The layout is computed in one place, so the
map can never disagree with what is on screen.

Two details matter and are easy to get wrong:

- **The capture is turned off again on every way out** — the normal
  exit, the error paths, and the panic hook. A terminal left
  reporting mouse events prints escape sequences into your shell after
  the program has gone.
- **The terminal is not asked to report every movement.** crossterm
  turns that on along with everything else, and it is the wrong thing
  for a full-screen program: every twitch of the mouse would arrive
  as an event, and every event draws a frame. Presses, releases and
  the movement that happens while a button is held are enough.


### Browsing a library

A library of a thousand sets needs two questions answered at a glance:
*which one do I want*, and *which ones am I actually working on*.

The second question is what the list is built around. Every row carries
the shape of that project's own history — one bar per slice of the
project's life, tallest where the work clustered — the number of notes
still open on it, and how long ago the file was last edited. A strip
above the table names the set under the cursor, so the selection is
never a guess.

The first question has two answers. `/` filters the list as you type,
across project names and folders, and shows what matched rather than
silently hiding the rest. `S` cycles the order: most recent, by name, by
the length of the history, or by the notes still open. The sort in
force is always named in the footer, so the order is never a mystery.

The footer is three regions of fixed width — what the list is doing on
the left, a message in the middle, and the relay's state on the right.
A message of any length is truncated rather than allowed to shove the
rest of the bar sideways. That is the whole point of it: a
notification that moves the layout is a notification nobody can read.

```
│ ▸ Tin Roof Sessions  8 snapshots  ○1 open  edited 9m                  │
│TYPE  NAME              FOLDER           SNAPS      BPM  TRACKS WORK OPEN  AGE│
│ ALS  Midnight Drive    sets/Midnight    6·489c61e  128  9     ▄▁▄▁▄▁▄█ ○2   9m │
│ ALS  Tin Roof Sessions sets/Tin Roof    8·95bbe5b  128  12    ▄▁▄▁▄▄██ ○1   9m │
│ ALS  Harbour Lights    sets/Harbour     5·ef3f314  128  7     █▁▁█▁███ —    9m │
│ recent  /roof            ↑↓ navigate · Enter open · S sort · q quit  ○ offline│
```

---

## Sync Architecture

No cloud account needed. The relay is a TCP server that runs on someone's machine.

```
  Your Mac Studio                Friend's Laptop
        │                              │
    clavus tui                     clavus tui
        │                              │
        └────────── TCP relay ──────────┘
                  (clavus relay)
                 127.0.0.1:4477
             or a LAN IP + port
```

One person runs `clavus relay`. Others run `clavus tui`, press `c` to connect, then `P` to push and `G` to pull. The relay stores snapshots, cues, and manifests — not audio files.

---

## Design

**Omarchy-inspired dark aesthetic** — warm blacks (`#0b0c0d`), layered panel surfaces, soft green accent (`#74e08c`), amber markers. No cold blue GitHub-dark. The TUI should feel like a piece of studio software, not a code editor.

**Audio-first philosophy** — Clavus audits your samples before every snapshot. It won't let you save a version with missing audio files. This is the core insight that separates it from generic file backup tools.

**Local-first, no cloud** — your project files never leave your machine unless you explicitly push to a relay. The relay is a tiny TCP server you control.

---

## Project Status

| | |
|---|---|
| Binary | ✅ 4MB standalone Rust binary |
| Ableton parser | ✅ `.als` (gzip or plain XML) — reads real Live 8/9/10/11/12 sets |
| Bitwig parser | ✅ `.bwproject` (zip XML) — fixture needed |
| Snapshot store | ✅ Content-addressed blob + metadata, a chain per project |
| Audio audit | ✅ Checks all sample references before snapshot |
| Cue markers | ✅ Pending / Resolved / Skipped / Deferred / Archived |
| Relay sync | ✅ TCP framed JSON, push/pull/connect, isolated per project |
| TUI | ✅ Ratatui, full keyboard nav |
| CI | ✅ GitHub Actions, clippy clean, 678 tests |
| Project isolation | ✅ Two same-named projects never share a chain |
| Linux | ✅ Static musl builds for x86_64 and arm64, verified on real Linux |
| Relay | ✅ Verified between three real Linux machines over TCP |
| Windows | 🔜 Linux binary first |

---

## Contributing

MIT licensed. Contributions welcome.

```bash
git clone https://github.com/castle-queenside/clavus
cd clavus/rust
cargo test --all
cargo clippy --workspace --all-targets -- -D warnings
```

To check that the release binary really moves bytes between machines —
three Linux machines in Docker, one relay between them:

```bash
bash scripts/two-machine-relay-test.sh
```

See [CLAVUS.md](CLAVUS.md) for full architecture documentation.

---

## Alternatives

| Project | What it is |
|---------|-----------|
| [stemctl](https://github.com/diveydoescode/stemctl) | Cloud-first, branching, PRs for DAW projects. Rust + Tauri. |
| [ableton-live-version-control](https://github.com/colinmcardell/ableton-live-version-control) | Plain git wrapper for `.als`. Shell. |
| [ableton-cookbook-mcp](https://github.com/teamallnighter/ableton-cookbook-mcp) | AI-assisted Ableton workflow. Version history, rack analysis. |

**What Clavus does differently:** local-first, no account, no cloud, audio sample audit, Bitwig support, full TUI with spectrum analyzer. The relay is just a TCP server you run — you control it.

---

## License

MIT — see [LICENSE](LICENSE).
