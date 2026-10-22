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
| **Verify** | `cargo clippy --workspace --all-targets -- -D warnings` |
| **CI** | `.github/workflows/rust-ci.yml` — Rust CI on push to `rust` |

## Verify Before Every Commit

```bash
cd ~/Projects/clavus-rust/rust
cargo clippy --workspace --all-targets -- -D warnings
cargo test --all
```

> The workspace flag matters: `--all-targets` alone lints only the root
> package, so the member crates' tests were never checked.

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
carries five names:
- `key` — the storage key: the file name and a digest of the whole
  absolute path (`Track.als__23559124b783`). It names the project's
  HEAD, its snapshot records and its cues.
- `name` — the display name: the file's stem (`Track`).
- `file_name` — the file name with its extension (`Track.als`).
- `folder_path()` — the folder's path below the home directory
  (`music/Album1`), or the last two folders of the path for a project
  outside the home directory (`media/p1/studio`).
- `sync_name()` — the name on the wire: the file name with its extension,
  because two machines holding the same project have it at different
  paths and must still agree, while an `.als` and a `.bwproject` with the
  same stem stay apart.

The identity is path-aware on purpose. A shared file name is not a shared
project: `Album1/Track.als` and `Album2/Track.als` are two different sets
with two different histories, and a snapshot of one must never be written
over the other. A folder's *name* is not enough either:
`notes/p1/studio/Track.als` and `notes/p2/studio/Track.als` share both
the folder name and the file name — only the path digest tells them
apart.

The paths are normalised before they are hashed, so the same file given
as a relative path and as an absolute path is one project, and symlinks
are not followed.

On the wire the identity is the file name *and* the folder's path, and
the relay files a project under `<file name>::<folder path>`. A colon
inside either part is escaped, so a file name cannot forge another
project's identity. A client says which folder it is in when it says
hello. When the relay cannot tell which project a client means —
several projects with that file name, and none of them identified by the
client's history — it serves none of them rather than guessing. A relay
written before this change keeps its history: the old heads and notes are
moved to the project's new key when it starts.

The folder path is what keeps two same-named projects apart, and it has
to work outside the home directory too. A project on an external drive
keeps the last two folders of its path, because the name of one folder
was not enough: `external/p1/studio/Song.als` and
`external/p2/studio/Song.als` both kept "studio" and shared one
identity, and the relay served one the other's history and notes.

Two folders of the same name at the **same depth** are two projects a
user keeps side by side, and one must never be served the other's
history however identical their bytes are. Two folders of the same name
at **different depths** are a collaborator's copy of a set on another
machine, and the bytes of the file are what prove it.
`project::folders_are_siblings` draws that line, and the relay
combines it with the client's file hash.

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

The selected cue and snapshot are **highlighted**. A cursor nobody can
see is not a cursor, and the footer says "↑↓ navigate".

The TUI refuses to draw below `60x10` and says so. Below that the
layout underflowed and the app died with a bare exit code; the panic
hook restores the terminal and then calls the default hook, so a panic
is never silent.

`App.sync_opened_from` records the screen the sync screen was opened
from, so `Esc` returns to it. The TUI and the CLI share the same rules
through `clavus_core`: `default_author()` for a note's author, and
`snapshot::head_after_pull()` for where a pull leaves the HEAD.

### Theme (Omarchy-inspired)
- BG: `#0b0c0d`, PANEL_0: `#0e1012`, PANEL_1: `#121517`
- LINE: `#23282b`, ACCENT: `#74e08c`, WARM: `#e9b455`
- Spacing: 4pt grid (`sp1()` = 4, `sp2()` = 8, etc.)
- Cliamp spectrum: 10-band LED bars, activated when relay connected

## Verify Workflow

```
cargo clippy --workspace --all-targets -- -D warnings   # fast
cargo test --all                                         # fast
cargo build --release                                    # only before push
git add -A && git commit && git push
```

> The pre-commit hook stashes unstaged changes before it runs, so a
> commit's tree must stand on its own. If a change depends on another,
> they belong in the same commit.

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
- Project identity: a path digest locally, so no two projects can ever
  share a chain; the file name and the folder's *path* on the wire, so
  two same-named projects never mix
- Blob transfer: the project bytes travel with the snapshots, hash-verified,
  so a pulled snapshot can be restored on the other machine
- `collect` keeps a Bitwig bundle self-contained: the samples go into
  the archive and the references point at the copies inside it
- The relay's head only moves forward, and a refused push is not stored
- A store that cannot be read is an error, never an empty history
- The TUI survives any terminal size, shows its selection, and notices
  a relay that has gone away
- 478 tests passing, clippy clean, CI green

### Blocked ⛔
| Item | Blocker |
|------|---------|
| Bitwig schema | The committed fixtures are realistic but synthetic — a real export from Bitwig Studio is still needed to confirm the schema |
| Windows binary | Cross-compile target TBD — need studio machine distro |

## Pending Work

### High Priority
1. **Real Bitwig export** — user action needed, to confirm the schema
2. **Linux musl binaries** — build and test on the studio machine
3. **PR #13 review** — ready for review
4. **Two real machines** — exercise a relay push and pull with a
   collaborator on another machine

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
cargo clippy --workspace --all-targets -- -D warnings
cargo test --all
```
