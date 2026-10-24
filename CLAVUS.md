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

### A project is a pointer, not a copy
This is the question every user asks first, so it is worth stating
plainly.
**Clavus never imports a project into a database.** There is no import
command, and there is no project database. A project is identified by
*where it is*: its file name, its folder, and a digest of its absolute
path. Your `.als` file stays exactly where it is, and Clavus never
moves, re-saves or duplicates it.

What Clavus stores is **version history**, and that is a copy by
design — you cannot restore yesterday's bytes unless you kept them:

| What | Where it lives | Copied? |
|---|---|---|
| The set itself | where you put it | **no** — Clavus only points at it |
| A snapshot's project bytes | `~/.clavus/blobs/objects/<xx>/<sha256>` | yes — that is the history |
| A snapshot's audio | the same blob store | yes — so a restore brings the sound back |
| A snapshot's note | `~/.clavus/blobs/projects/<key>/snapshots/<hash>.json` | a small manifest |
| The cues (notes) | `~/.clavus/cues/<key>/*.json` | yes — they are Clavus's own |

So a library of a thousand sets is not duplicated a thousand times.
Only the sets you actually snapshot cost anything, and only the bytes
of those.

**This is a change from the version before.** That one *did* import:
it copied each project into `~/Clavus/Projects/<Name>/`, collected the
samples into the copy, and worked there. The evidence survives in a
backup — the recovered library holds copies whose sets still point at
the machine they were made on (`C:/Users/chris/OneDrive/…`), with the
audio beside them, and its own Live backups show the work continued
there afterwards.

That behaviour is recoverable but not reproducible by the current
code: `clavus import-legacy` brings the old *history* across, and the
library itself is copied back with `recover-clavus-library.sh`. The
current design deliberately leaves your set alone. If you want a set
made self-contained — its audio beside it and its paths pointing at
that copy — `clavus collect` does exactly that, in place.

A corollary: **a set on an unmounted drive is unreachable**, and the
project's identity includes its path, so renaming the drive gives the
sets a new identity. The blob store keeps the bytes, but the *pointer*
now names a path that is not there.

### Where a project is looked for
`~/Music`, `~/Documents`, and **`~/Clavus/Projects`**.

The third one is not an invention of the rewrite. The Python version's
default was `Path.home() / "Clavus" / "Projects"` — a library folder of
its own, beside the sets, holding `Set Name/Set Name Project/Set
Name.als`. A machine that was set up with it still has that folder, and
the rewrite had stopped looking there, which made a library that had
been Clavus's own for years invisible.

`clavus --dir <path>` names one directory to work in, and it is the
only place searched — a user who named a directory did not ask for the
whole library. Without it, the standard places are searched, so
`clavus log "My Set.als"` finds the set from anywhere. The name has to
be unique across every place searched: two sets of the same name in
two folders are two projects, and neither is the one that was asked for.

### When the library moves
A set writes down the absolute path of every sample it uses. Rename the
drive, rebuild the machine under a different user name, and every one of
those paths stops resolving — the set then looks like it has lost its
audio, and the audit says so.

```bash
clavus relocate map "/Volumes/Old Disk" "/Volumes/LaCie"
clavus relocate list
clavus relocate unmap "/Volumes/Old Disk"

# When you have a whole library of moved paths and do not know them all:
clavus relocate suggest --root /Volumes/LaCie ~/Music/Ableton/MySet.als
```

The rules live in `~/.clavus/relocations.json` and are asked about
after the path the set names has been tried, so a rule that is out of
date cannot move a sample that was already where the set said it was.

A rule's **start** is the path as the set wrote it, so it may be
relative (`Beat Lab/Samples`) or a Windows path from a set made on a PC
(`C:/Users/Chris/Music`). A rule's **destination** has to be an
absolute path on this machine.

`clavus relocate suggest` works the rules out for you. It audits the
sets you name, looks for each missing sample under the folder you
point it at, and prints the rules that would put them back — one line
each, ready to run. It writes nothing: a rule that sends a sample to
the wrong copy is worse than no rule at all, so the user reads them and
chooses.

On the real studio drive this took 305 samples that the audit called
lost back to where they are. The rest — twelve files — were never
copied to the drive in the first place.

### Coming from the older Clavus
The version before this one kept its library in `~/Clavus/Projects` and
its history in `~/.clavus`. Both are still readable.

```bash
clavus import-legacy --store ~/.clavus --projects ~/Clavus/Projects
```

The old store's snapshot records carry the same fields this one's do,
so the history comes across as history — the same chain, the same
notes, the same bytes — rather than as a summary of itself. Each
record is re-pointed at where the project lives now, because the old
one named a path on the machine it was written on.

The projects folder is how the sets are found. The old store names
each project by its old path, and the part of that path after
`Clavus/Projects` is still true of the library, so the file is found
where the old store said it was. A project whose file cannot be found
is named and left behind; a record that cannot be read is named too.
Nothing is dropped in silence.

`--dry-run` says what would happen and writes nothing at all.

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

A sample reference is resolved against the **project**, never against the
process's current directory. A relative path in a Live set means "next to
the set", and the folder a command happens to be run from is not part of
the set. Resolving it against both made the audit depend on where it was
run, and two runs of the same command could disagree. A `.bwproject` is a
bundle, so its own relative reference points *inside the bundle* — the
bundle is asked before the disk, because a file of the same name sitting
next to the bundle is not the sample the project uses.

A reference that no longer resolves is looked up in the **relocation
map** before it is called missing. A set writes down the absolute path of
every sample it uses; when a drive is renamed or a library moves, those
paths stop resolving and the set looks like it has lost its audio. The
map is the user's own list of "this old path is this new path", kept in
`~/.clavus/relocations.json` and edited with `clavus relocate`.

The map is asked **last**, after the path the set names has been tried,
so a rule that is out of date cannot move a sample that was already
where the set said it was. A rule matches only on a path-component
boundary, so `/Volumes/Set` never rewrites `/Volumes/Set Two`. Clavus
does not search the disk for a file with the same name: on a real drive
151 of the references had more than one match, and a search would
sometimes pick the wrong copy of the same audio.

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

### Reading a real Live set
Live has changed the shape of its files every few years, and a parser
that only knows one shape reports confident nonsense for the others.
The rules Clavus follows:

- **The project's tempo lives in the master track** — `<Tempo><Manual
  Value="92"/></Tempo>` in Live 8, 9, 10, 11 and 12, or an automation
  curve in the oldest sets. It is read from `<MasterTrack>`, or from
  `<MainTrack>` in Live 12. A tempo inside a *track* is that track's
  own tempo, and a tempo inside a *scene* is that scene's: neither is
  the project's.
- **The devices live in a `<Devices>` list**, inside a `<DeviceChain>`
  that is nested inside the track's own `<DeviceChain>`. Everything
  else in a chain is structure — mixer, routing, automation lanes —
  and reading it as devices produced a list of every element in the
  file.
- **A track's name has two shapes**: `<Name Value="Kick"/>` in Live 8,
  and `<Name><EffectiveName/><UserName/></Name>` from Live 9. The
  user's name wins when they gave one. Live 8 often stores no name at
  all, so Clavus shows what Live's own interface shows: `1-MIDI`.
- **`<KeyTrack>` is a MIDI keyboard track** and carries no name; it is
  called `MIDI Keyboard`.
- **Live wrote some sets in its own binary format** before gzip. That
  format is a serialisation, not XML, so Clavus cannot read it — and
  says so plainly instead of reporting a decoding failure.

Everything above was learned by reading 1,245 real sets off a studio
drive — 1,242 of them readable, and finding that every one of those
parsed as one track, 120 BPM and no devices. The tests are in
`crates/parser-als/tests/real_world_tests.rs`.

### Browsing a library
A list of a thousand sets has to answer two questions at a glance:
which one do I want, and which ones am I actually working on. The
second one is what the row is built around:

- `WORK` — the shape of the project's own history, one bar per slice
  of its life, tallest where the work clustered.
- `OPEN` — how many notes are still open on it.
- `AGE` — how long ago the file itself was last edited.
- A strip above the table naming the set under the cursor.

The history shape **has a floor of one day on its axis**
(`spark::MIN_SPAN`). Without it, six snapshots taken in one afternoon
land in six different slices of a six-second span, and the chart is a
row of bars that says nothing. With it, one sitting reads as one
burst and a few months of work reads as a spread. The difference is
the thing worth seeing.

An empty slice is drawn as the shortest bar in the quietest colour,
not as a space, so the column reads as one chart with a baseline and a
project with no history at all reads as a flat line.

The first question has two answers: `/` filters the list as you type,
across names and folders, and `S` cycles the order — recent, name,
history, open notes. The order in force is always named in the footer.

**The footer is three regions of fixed width** — what the list is doing
on the left, a message in the middle, the relay's state on the right.
A message of any length is truncated rather than allowed to move the
layout. A notification that shoves the rest of the bar sideways is a
notification nobody can read.

### The mouse
Clavus is keyboard-first and the keyboard still does everything. The
mouse is a second way in.

- The **wheel** moves the selection.
- A **click** picks the row under the pointer.
- A **double click** opens the set — the same as `Enter`.
- A **drag** with the button held makes the row under the pointer the
  selected one.
- Clicking the **scrollbar** jumps the window to that part of the
  list.

Nothing is reachable by mouse that is not reachable by keyboard.

### The renderer owns the layout, and the mouse is told about it
The renderer records a map of the rows it drew — `App.hits` — and the
mouse handler looks a click up in that map. The layout is computed in
one place, so the map can never disagree with what is on screen, and a
test can check the map against the drawn rows instead of against a
hand-written copy of the layout. The map is rebuilt every frame: the
layout moves when the terminal is resized, and a stale map would send
a click to a row that has moved.

### A list that does not fit has to scroll
The renderer works out how many rows fit, and `scroll_to_show()` keeps
the cursor on the screen. The renderer is the authority because it is
the only thing that knows how big the terminal is.

Without it, the list was drawn from the top and clipped: the arrow
keys still moved the cursor, so it walked off the bottom of the
screen and the user was left with a selection they could not see and
a list that never moved. A scrollbar down the right edge says where
the window is, and clicking it jumps the window.

The same bug was in the cue list, in a different form: the cue filter
changes which notes are drawn, but the arrow keys walked the
unfiltered list while the highlight was computed on the filtered one.

### The mouse capture is undone on every way out
The normal exit, the error paths and the panic hook all turn the
capture off. A terminal left reporting mouse events prints escape
sequences into the shell after the program has gone.

crossterm asks the terminal to report **every movement** along with
presses and releases, and the "any movement" mode is turned off again
straight away: for a full-screen program every twitch of the mouse
would arrive as an event, and every event draws a frame. Presses,
releases, and the movement that happens while a button is held — which
is how a list is dragged — are enough.

### Keys are normalised before they are matched
crossterm reports an uppercase letter with the shift modifier set,
because that is what the keyboard physically did: `S` arrives as
`Char('S')` plus `SHIFT`. `keys::normalise()` drops the shift
modifier from a character key — but only when no other modifier is
held, so Ctrl-Shift-C is not read as a capital C.

Without this, every capital-letter shortcut was alive in the tests and
dead in the program, because the tests built their key events by hand
with no modifiers. `S` to sort, `P` to push and `G` to pull did
nothing, and no capital letter could be typed into a note or a search
box. **A key test must use the shape a terminal actually sends.**

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
- The project list can be filtered and sorted, shows the shape of each
  project's history, the notes still open on it and how long ago it
  was last edited, and names the set under the cursor
- A long list scrolls, with a scrollbar, and the cursor never walks
  off the screen
- The mouse works: the wheel scrolls, a click picks, a double click
  opens, a drag picks, and the scrollbar is a control
- Capital-letter shortcuts work, and capital letters can be typed into
  a note or a search box
- 627 tests passing, clippy clean, CI green
- The `.als` parser reads the real library: 1,242 of 1,245 sets from
  Live 8 to Live 12 report their real track count, tempo, track names
  and devices. The other three are in Live's old binary format and say
  so instead of failing obscurely
- The library list opens in 0.33 seconds on a 1,245-set drive, with
  the numbers arriving in the background
- The Linux musl build runs on real Linux, and the release binary moves
  project bytes, audio and notes between three real machines over TCP —
  `scripts/two-machine-relay-test.sh`

### Blocked ⛔
| Item | Blocker |
|------|---------|
| Bitwig schema | The committed fixtures are realistic but synthetic — a real export from Bitwig Studio is still needed to confirm the schema |
| Windows binary | Cross-compile target TBD — need studio machine distro |

## Pending Work

### High Priority
1. **Real Bitwig export** — user action needed, to confirm the schema
2. **Ableton Live round trip** — user action needed: install Live, open
   a set from each major version, save, snapshot, restore
3. **PR #13 review** — ready for review
4. **Two real machines** — the containers prove the protocol; a push and
   pull between two physical machines is still worth doing

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
