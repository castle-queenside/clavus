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
| **Audio audit** | Before every snapshot: verify all sample references are present and inside the project folder. No silent missing samples. A Bitwig bundle's own samples count as present, and `collect` keeps the bundle self-contained. |
| **Cue markers** | Pin timeline comments at bar positions. Filter by Pending / Resolved / Skipped / Deferred / Archived. Each note records who wrote it. |
| **Relay sync** | Push/pull snapshots over TCP to a collaborator's relay. No cloud. |
| **Diff** | Walk the chain. See what changed: BPM, track count, sample manifest, message. |
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
Esc/q      Back / quit
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
| Ableton parser | ✅ `.als` (gzip or plain XML, Live 9/10/11) |
| Bitwig parser | ✅ `.bwproject` (zip XML) — fixture needed |
| Snapshot store | ✅ Content-addressed blob + metadata, a chain per project |
| Audio audit | ✅ Checks all sample references before snapshot |
| Cue markers | ✅ Pending / Resolved / Skipped / Deferred / Archived |
| Relay sync | ✅ TCP framed JSON, push/pull/connect, isolated per project |
| TUI | ✅ Ratatui, full keyboard nav |
| CI | ✅ GitHub Actions, clippy clean, 512 tests |
| Project isolation | ✅ Two same-named projects never share a chain |
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
