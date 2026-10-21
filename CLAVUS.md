# CLAVUS.md — Clavus Rust Rewrite Agent Notes

> Read before touching any code. Updated every session.

## Quick Facts

| | |
|---|---|
| **Repo** | `https://github.com/castle-queenside/clavus.git` |
| **Rust worktree** | `~/Projects/clavus-rust/` |
| **Python worktree** | `~/Projects/clavus/` (`main` branch) |
| **PR** | [#13](https://github.com/castle-queenside/clavus/pull/13) — Rust rewrite |
| **Binary** | `~/Projects/clavus-rust/rust/target/release/clavus` (4.2MB) |
| **Verify** | `cargo clippy --all -- -D warnings` |
| **CI** | `.github/workflows/rust-ci.yml` — Rust CI on push to `rust` |

## Verify Before Every Commit

```bash
cd ~/Projects/clavus-rust/rust
cargo clippy --all -- -D warnings
cargo test --all
```

## Architecture

```
rust/
├── Cargo.toml              # workspace
├── crates/
│   ├── core/               # blob store, snapshot, cues, sample audit
│   ├── parser-als/         # .als parser (gzip XML → Project)
│   ├── parser-bw/          # .bwproject parser (zip XML → Project)
│   ├── relay/              # TCP relay server (accepts clients, broadcasts)
│   ├── sync/               # SyncClient (TCP framed JSON)
│   ├── tui/                # Ratatui app (screens, theme, key handling)
│   └── cli/                # command implementations (testable, stream-injectable)
└── src/main.rs             # argument parsing + dispatch + TUI event loop
```

The command implementations live in `clavus-cli` rather than in the binary so
they can be driven directly from tests, with output written to injectable
streams rather than straight to stdout.

## Core Types

- `Project` — parsed DAW project: name, bpm, time signature, tracks, markers, sample refs
- `Snapshot` — content-addressed checkpoint: hash, parent, timestamp, bpm, track_count, sample_hashes, sample_paths
- `Cue` — timeline comment: position, text, status, track, author, id
- `SampleRef` — audio sample reference: path, hash, resolved_path, size_bytes, relpath_from_project

## Key Design Decisions

### Content-addressed storage
- Raw `.als` bytes are the blob. SHA256 = snapshot hash.
- Duplicate saves detected automatically — same bytes = same hash.
- `BlobStore` handles raw bytes; `SnapshotStore` handles metadata + refs.
- The blob store is shared by every project, so each project has its own
  HEAD at `<project>/HEAD`. A single shared HEAD meant that whichever
  project was snapshotted last owned the chain, and the other project's
  `log`, `push` and `restore` showed the wrong history.

### Bitwig archives
A `.bwproject` is a ZIP archive holding `project.xml` and, usually, a copy
of every sample the project uses. The references inside `project.xml`
still point at the original machine's absolute paths, so on another
machine those samples look missing even though they are in the archive.
`clavus_core::bitwig_archive` reads and rewrites the archive: the audit
knows which samples the archive holds, and they are **present**, not
missing. `collect` keeps the bundle **self-contained** — it rewrites the
reference inside `project.xml` to the sample's path *inside* the archive
rather than copying a duplicate out beside the project. A bundle that
already points at its own internal sample needs no repair.

### Project identity
`clavus_core::project::ProjectId` is the identity of a project, and it
carries four names:
- `key` — the storage key: the project's folder and file name, made safe
  for use as a directory name (`Album1__Track.als`). It names the
  project's HEAD, its snapshot records and its cues.
- `name` — the display name: the file's stem (`Track`).
- `file_name` — the file name with its extension (`Track.als`).
- `sync_name()` — the name on the wire: the file name with its extension,
  because two machines holding the same project have it at different
  paths and must still agree, while an `.als` and a `.bwproject` with the
  same stem stay apart.

The identity is path-aware on purpose. A shared file name is not a shared
project: `Album1/Track.als` and `Album2/Track.als` are two different sets
with two different histories, and a snapshot of one must never be written
over the other.

### Untrusted remote data
Snapshots, cues, hashes and ref names all arrive from remote peers.
Everything that becomes a file name is validated before it is used:
- Hashes must be plain lowercase hex (a SHA-256 digest).
- Cue ids must be alphanumeric, `-` or `_`.
- Ref names may not contain `..` or escape the refs directory.
- The relay rejects malformed snapshots and cues rather than serving them.
- The relay only ever serves a client the history of the project that
  client named in its Hello.

### Times
`clavus_core::time` formats every time the tool shows. The times are
**local** — the system's own time zone rules do the work, so daylight
saving needs no table of our own — and the offset is named once per
list, so a reader knows whose clock they are looking at.

### Audio audit before snapshot
`SampleAuditReport.ensure_clean()` is called before every `SnapshotStore::save()`.
- Blocks snapshot if any sample is missing from disk.
- Distinguishes "collected" (inside project) vs "outside_project" vs "missing".
- This is the core differentiation from generic file backup tools.

### Sync protocol (TCP framed JSON)
- 4-byte BE length prefix per message.
- `SyncClient` connects, sends `Hello`, then receives `Subscribed`.
- `Pull` / `Push` / `Synced` commands. Background mpsc receiver thread.
- `SyncState` in TUI holds `cmd_tx` (Sender<SyncCmd>) for background thread.

### TUI state machine
`App.screen: Screen` drives rendering. Screens: `ProjectList`, `ProjectDetail`, `CueList`, `CueAdd`, `SnapshotList`, `Sync`.

Navigation state per screen: `selected_project`, `selected_cue`, `selected_snapshot`.

### Theme (Omarchy-inspired)
- BG: `#0b0c0d`, PANEL_0: `#0e1012`, PANEL_1: `#121517`
- LINE: `#23282b`, ACCENT: `#74e08c`, WARM: `#e9b455`
- Spacing: 4pt grid (`sp1()` = 4, `sp2()` = 8, etc.)
- Cliamp spectrum: 10-band LED bars, activated when relay connected

## Verify Workflow

```
cargo clippy --all -- -D warnings   # fast
cargo test --all                    # fast
cargo build --release              # only before push
git add -A && git commit && git push
```

## Current Status

### Working ✅
- `.als` parser (gzip or plain XML, quick-xml) — Live 9/10/11 path handling
- Bitwig `.bwproject` parser (zip XML) — **needs a real export to validate**
- Snapshot store (content-addressed blobs + metadata, a HEAD per project)
- Audio sample audit (checks refs before snapshot; sees inside a bundle)
- Cue markers (Pending / Resolved / Skipped / Deferred / Archived)
- TUI: all screens, keyboard nav, Omarchy theme, editable relay address
- Cliamp spectrum analyzer (sync screen)
- Relay sync server + client, isolated per project
- Project identity: path-aware, so two same-named projects never mix
- 391 tests passing, clippy clean, CI green

### Blocked ⛔
| Item | Blocker |
|------|---------|
| Bitwig schema | The committed fixtures are realistic but synthetic — a real export from Bitwig Studio is still needed to confirm the schema |
| Windows binary | Cross-compile target TBD — need studio machine distro |
| Blob transfer | Sync carries metadata only, so a pulled snapshot cannot be restored on another machine yet |

## Pending Work

### High Priority
1. **Real Bitwig export** — user action needed, to confirm the schema
2. **Linux musl binaries** — build and test on the studio machine
3. **Blob transfer** — decide whether v0.1.0 ships with metadata-only sync
4. **PR #13 review** — ready for review

### Nice to Have
5. HTML timeline visualization (like ableton-cookbook's historian)
6. Track-level diff (show "Track 'Bass': added Serum" vs raw hash diff)
7. Sample collection (`cmd_collect` stub: copy missing samples into project folder)
8. TUI snapshot trigger (press `c` in ProjectDetail → runs audit + save)

## Commit Convention

```
feat(scope): description
fix(scope): description
refactor(scope): description
chore(scope): description
```

Short, imperative, lowercase after colon. One logical change per commit.

## Agent Session Files

```
~/agent-state/clavus-design.json     # design + theme decisions
~/agent-state/clavus-rust-coord.json  # sprint coordination state
```

## When Starting a New Clavus Session

```bash
# 1. Check where we are
git -C ~/Projects/clavus-rust log --oneline -3

# 2. Read this file
cat ~/Projects/clavus-rust/CLAVUS.md

# 3. Check blocked items
# (Bitwig fixture — needs user action)

# 4. Run verify
cd ~/Projects/clavus-rust/rust
cargo clippy --all -- -D warnings
```
