# Clavus Testing Plan

> Last updated: 2026-10-24
> Status: 🟢 Core suites complete — the AI beta waves' findings are all fixed and covered

## Coverage at a Glance

| Area | Test file | Tests | Status |
|------|-----------|-------|--------|
| Parser ALS | `crates/parser-als/tests/als_parser.rs` | 48 | 🟢 |
| Parser ALS (real sets) | `crates/parser-als/tests/real_world_tests.rs` | 17 | 🟢 |
| Parser Bitwig | `crates/parser-bw/tests/bw_parser.rs` | 21 | 🟢 |
| Sample audit + collect | `crates/core/tests/collect_tests.rs` | 24 | 🟢 |
| Relocation map | `crates/core/tests/relocate_tests.rs` | 10 | 🟢 |
| Relocation map (CLI) | `crates/cli/tests/relocate_tests.rs` | 7 | 🟢 |
| BlobStore / snapshot core | `crates/core/tests/core_tests.rs` | 42 | 🟢 |
| Cue store | `crates/core/tests/cue_tests.rs` | 26 | 🟢 |
| Property-based (core) | `crates/core/tests/property_tests.rs` | 11 | 🟢 |
| Beta regressions (core) | `crates/core/tests/beta_regressions.rs` | 12 | 🟢 |
| Relay sync | `crates/relay/tests/relay_tests.rs` | 11 | 🟢 |
| Relay (real server) | `crates/relay/tests/real_relay.rs` | 27 | 🟢 |
| Sync client (protocol) | `crates/sync/tests/sync_client.rs` | 8 | 🟢 |
| Sync client (hostile server) | `crates/sync/tests/hostile_server.rs` | 19 | 🟢 |
| TUI | `crates/tui/tests/tui_tests.rs` | 46 | 🟢 |
| TUI (beta regressions) | `crates/tui/tests/beta_tui_tests.rs` | 10 | 🟢 |
| TUI (sync round trip) | `crates/tui/tests/sync_blob_tests.rs` | 1 | 🟢 |
| TUI (fourth wave) | `crates/tui/tests/wave4_tests.rs` | 13 | 🟢 |
| TUI (browsing a library) | `crates/tui/tests/workbench_tests.rs` | 24 | 🟢 |
| TUI (mouse and scrolling) | `crates/tui/tests/mouse_tests.rs` | 19 | 🟢 |
| TUI (a library of a thousand) | `crates/tui/tests/scale_tests.rs` | 13 | 🟢 |
| CLI commands | `crates/cli/tests/cli_tests.rs` | 54 | 🟢 |
| CLI (beta regressions) | `crates/cli/tests/beta_cli_tests.rs` | 17 | 🟢 |
| CLI (second beta wave) | `crates/cli/tests/wave2_tests.rs` | 6 | 🟢 |
| CLI (adversary findings) | `crates/cli/tests/adversary_tests.rs` | 5 | 🟢 |
| CLI (ergonomics wave) | `crates/cli/tests/ergonomics_tests.rs` | 9 | 🟢 |
| CLI (fourth wave) | `crates/cli/tests/wave4_tests.rs` | 22 | 🟢 |
| CLI (collect reports) | `crates/cli/tests/collect_tests.rs` | 4 | 🟢 |
| Core unit tests | `crates/core/src/**` | 33 | 🟢 |
| Sync unit tests (blob transfer) | `crates/sync/src/**` | 10 | 🟢 |
| Two machines, one relay | `scripts/two-machine-relay-test.sh` | 3 machines | 🟢 |

**615 tests passing, 0 failed, 0 ignored**, plus a three-machine relay
test that runs the real Linux binary over real TCP.
The palette is Clavus's own, not the terminal's. A panel that sets no
background of its own shows the *terminal's* background through, and
the screen is then half Clavus's warm black and half whatever the
terminal was configured with. Over a remote session, where the
terminal on the other end is a different program with a different
theme, that reads as a patchy screen with different colour
backgrounds. Every screen is now checked for cells left on the
terminal's own background.

The selected row was a full block of the accent green. A bright bar
that travels as the cursor moves is hard to read and draws the eye, so
the row is *raised* instead — a dark green surface, plainly different
from the panel around it, with a `▌` marker in the badge's own column
so the row's text does not move when it is selected. A row that
shifts sideways under the cursor reads as a glitch.


The wave-five batch includes the Live user-library fix: a set that
points at `Samples/kick.wav` means the file in Live's own user library
at `~/Music/Ableton/User Library`, not `~/Music/Samples/kick.wav`.
Looking in the wrong place made a complete set read as broken and
stopped the snapshot. The user library is now a root, and it is tried
before the wider music folder so a set that has both is not reported
as pointing at the wrong file. The roots take the home folder as a
parameter, so the list is checkable without changing the environment
of the machine running the test.

Legend: 🟢 good  🟡 partial  🟠 manual only  🔴 missing

---

## Priority 1: Done ✅

Relay, collect, parser, TUI, cue-store and property-based suites are written
and passing. Every test drives production code — the relay tests drive the
real `run_with_listener` server, and the TUI tests drive the real key
handler in `clavus_tui::keys`.

---

## Priority 1: Done ✅

Relay + collect integration tests are written and passing. See:
- `crates/relay/tests/relay_tests.rs` — 10 tests
- `crates/core/tests/collect_tests.rs` — 9 tests

---

## Priority 2: Remaining Gaps

### 2.1 CLI command tests ✅

The command implementations were moved out of the binary into the
`clavus-cli` crate so they can be driven directly, with output written to
injectable streams. `crates/cli/tests/cli_tests.rs` covers `audit`,
`snapshot`, `log`, `cue`, `cues`, `status`, `restore`, `collect`,
`projects`, `push` and `pull`, plus three end-to-end tests that run the
compiled binary.

### 2.2 Snapshot rendering snapshots 🟡

`insta` snapshot tests of the TUI's rendered output would catch visual
regressions in the Omarchy theme. The key-handler logic is already covered.

### 2.3 Real Bitwig fixture 🟡

`fixtures/test_project.bwproject` is generated by `fixtures/gen_bw_fixture.py`
and matches the schema the parser expects, but it has not been validated
against a project saved by Bitwig Studio itself. The parser should be
re-checked against a real file when one is available.

### 2.4 Sync blob transfer ✅

The project bytes travel with the snapshots. They are cut into 3 MiB
chunks, base64 encoded inside the framed JSON, and reassembled by the
receiver. The whole blob must hash to the name it was sent under, or it
is refused. A pulled snapshot can therefore be restored on the other
machine, byte for byte.

- `sync/src/blob_transfer.rs` — the chunking and the hashing
- `relay/tests/real_relay.rs` — the relay stores and serves the bytes
- `cli/tests/wave2_tests.rs` — a machine pulls and restores
- `tui/tests/sync_blob_tests.rs` — the TUI pulls and restores

---

## Priority 3: Real-World Validation

### 3.1 Manual Test Checklist

Run these manually with a real Ableton project:

```
ENVIRONMENT CHECKS
□ Project with 0 samples (baseline — should pass cleanly)
□ Project with 10+ samples inside project folder (audit should pass)
□ Project with 3 samples outside project (collect needed)
□ Project with missing samples (audit should warn, snapshot should BLOCK)
□ Project with Unicode characters in path (/Users/chris/Projects/🎵/)
□ Project with very long path (>200 chars)
□ .als file is actually gzip (not plain XML)

COLLECT COMMAND
□ collect: copies files, rewrites .als, audit passes after
□ collect: idempotent (run twice — second run does nothing)
□ collect: fails gracefully if destination dir is read-only

SNAPSHOT LIFECYCLE
□ snapshot: saves blob + meta + HEAD ref
□ snapshot: blocks if samples missing (ensure_clean called)
□ log: shows chain with correct hashes and timestamps
□ log: shows correct parent-child relationships

RELAY SYNC
□ relay starts, persists state, restart recovers state
□ push/pull: snapshots sync between two clavus instances
□ conflict: divergent HEAD → both clients notified

TUI
□ Project list: correct track count, BPM, snapshot count
□ Snapshot list: navigation with ↑↓, selection resets on refresh
□ Cue list: filter cycling with 'f'
□ Sync connect: 's' key → connection state shown
□ Push/Pull: 'P'/'G' work when connected, hint when not
```

### 3.2 Property-Based Testing ✅

Implemented with `proptest` in `crates/core/tests/property_tests.rs`:

- Chain length always matches the number of distinct snapshots
- Walking the chain from any point terminates and is a prefix of the full chain
- Restoring any snapshot reproduces the original bytes exactly
- Identical content never creates a duplicate snapshot
- Blobs round-trip unchanged; tampering is always detectable
- Audit classification is exhaustive and mutually exclusive

Still worth adding: arbitrary XML that is valid LiveSet structure, and
arbitrary DAGs of snapshots for conflict detection.

### 3.3 The two-machine relay test ✅

The unit tests exercise the real relay code, but in one process. What
they cannot show is that the *release binary* moves real bytes between
two real machines over real TCP. That test now exists:

```bash
bash scripts/two-machine-relay-test.sh
```

It starts three Linux machines in Docker — one relay, three clients —
using the musl build that ships to the studio, and it checks:

| Machine | What it holds | What must happen |
|---------|---------------|------------------|
| A — the studio | the set and its audio | two snapshots, a note, a push that sends the project bytes *and* the audio |
| B — a collaborator | the same set at a **different path and depth** | a pull that brings the chain, the note and the project bytes, and a restore that leaves the set byte-for-byte A's |
| C — a collaborator | the set and **no audio at all** | a pull whose audio arrives over the wire and restores byte-for-byte A's |

Machine B is the case the identity rules exist for: the same project,
kept somewhere else entirely. Machine C is the case blob transfer
exists for: the audio has to actually cross the network.

The fixture is generated by `fixtures/gen_relay_fixture.py` and the
compression is deliberate — gzip writes the time it was made into its
header, so two compressions of the same text are *different files*,
and a collaborator is only recognised when the bytes are truly
identical.

The test runs in CI (`two-machine-relay` in `.github/workflows/rust-ci.yml`)
on both Linux architectures. Locally it takes about a minute.

---

## Bugs Found By These Tests

The CLI suite was written to hunt for failure modes, and it found several
real ones. Each now has a regression test:

| Bug | Symptom | Fix |
|-----|---------|-----|
| False all-clear on a corrupt project | `audit` printed "✓ 0 samples present" for a file that was not a Live set | `validate_als_xml()` rejects non-Live and malformed XML before auditing |
| UTF-8 panic in `cues` | Listing a cue whose text had a multi-byte character at byte 22 panicked | Truncation is now character-based |
| Unknown status silently matched nothing | `cues --status open` printed "No cues." and exited 0 | `CueStatus::parse()` returns `None` for unknown names and `list()` errors |
| `collect` exited 0 after failing | A sample missing from disk was reported but the command still succeeded | `collect` now exits non-zero and explains what to do |
| `snapshot` claimed a new snapshot for unchanged content | The user's message was silently dropped but "✓ snapshot" was printed | The unchanged case is detected and reported honestly |
| `audit` printed "1 samples" | Grammar | Count-aware pluralisation |
| **Path traversal via a cue id** | A remote peer could write JSON anywhere on disk by sending a cue with an id like `../../pwned` | `CueStore::validate_cue_id()` rejects ids that are not plain alphanumeric |
| **Path traversal via a snapshot hash** | A remote peer could write outside the blob store, and a one-character hash panicked the client | `BlobStore::is_valid_hash()` requires plain lowercase hex |
| **Path traversal via a ref name** | A snapshot's `parent` is used as a ref name, so a crafted parent could read outside the refs directory | `BlobStore::is_valid_ref_name()` rejects separators and `..` |
| The relay served malformed snapshots | The relay stored any hash it was given | The relay rejects snapshots with an invalid hash or parent |
| **Cues never reached the other machine** | The push sent no cues and the pull counted them without storing them, so a collaborator's notes were lost | Both the CLI and the TUI now push and import cues |
| `restore` could write anywhere | Restoring a pulled snapshot to its recorded path would use a path chosen by the relay | The recorded path must name a `.als` or `.bwproject` file |
| **Every project shared one HEAD** | The blob store is shared, so whichever project was snapshotted last owned the chain — `log`, `push` and `restore` for the other project showed the wrong history | Each project's HEAD lives at `<project>/HEAD`; the old shared ref is only used by the project that owns it |
| The relay mixed up projects | Cues were grouped by track name and every snapshot was offered to every client | Cues carry a project name; the relay indexes snapshots per project and only serves a client its own project |
| A repeated push duplicated cues | The relay appended the cue each time, so every client got N copies | The relay replaces a cue with the same id |
| `projects --dir` was ignored | The command always scanned the standard locations, even though its own message told the user to pass `--dir` | An explicit directory is scanned; the standard locations are the fallback |
| A symlink loop hung the project scan | The recursive walk followed symlinks and could revisit a directory forever | Each directory is canonicalised and visited at most once; depth is capped |
| A short hash crashed the display code | Hashes and cue ids were sliced with `[..8]`, which panics on a shorter string — and a hostile relay controls both | `clavus_core::short()` truncates without panicking; incoming data is validated before it is used |
| A hostile relay could store a malformed snapshot | The client delivered whatever the server sent, unchecked | The client drops snapshots with a bad hash or parent and cues with a bad id |
| A pre-epoch file timestamp crashed `projects` | `duration_since(UNIX_EPOCH).unwrap()` panicked on a file older than 1970 | The timestamp falls back to `????-??-?? ??:??` |
| The TUI synced under a fixed name | The TUI always said "clavus-tui" in its Hello, so the relay filed every project under one name and served the wrong history | The TUI sends the selected project's name, and tells the relay when the selection changes |
| One project could be restored over another | A snapshot lookup fell back to "every snapshot we know about", and the blob store is shared — so `restore Beta.als <alpha-hash>` wrote Alpha's contents into Beta.als | A lookup only considers snapshots whose file name says they belong to that project |
| The relay stored hashes no client could file | The relay accepted a hash of any length, but a client can only file a 64-character digest | The relay refuses a hash or parent that is not exactly 64 hex characters |
| A Bitwig archive's samples looked missing | A `.bwproject` exported from Bitwig keeps a copy of every sample, but `project.xml` still points at the original machine's absolute paths — so the samples looked lost | The audit sees what the archive holds and reports those samples as present; `collect` extracts them and rewrites the reference inside the archive |
| The CLI and the TUI disagreed about the same folder | `clavus projects` looked only at the top level of each location while the TUI always looked into sub-folders, so a set in `~/Music/Archive/2013` was visible in the TUI and invisible to the CLI | The command scans into sub-folders by default, the same way the TUI does; `--no-recursive` asks for the top level alone |
| A test run could fail for no reason | The parser tests wrote to fixed names in the system temp dir, so two runs happening at once wrote the same file and deleted it out from under each other | Every throwaway file gets a name built from the process id, the clock and a counter |
| Live's own backup copies were listed as projects | Live keeps a `Backup/` and an `Autosave/` folder beside every set. A real drive held 2,690 `.als` files, of which 1,445 were these copies and 1,245 were the sets — so more than half the list was the same projects again under new names | A file inside a `Backup/` or `Autosave/` folder is a copy, not a project; the CLI and the TUI both skip them |
| A user-folder sample was looked for at the root of the filesystem | A Live 9 set stores its sample path relative to the machine's *user folder*, and the chain begins with an empty element standing for that folder's root. Joined naively the path began with `//`, and `//` is absolute on Unix, so the sample was looked for at `/Samples/kick.wav` | The empty element is dropped, and the path is looked for beside the project and in the usual places a user folder lives |
| Live's own factory library was reported as missing | 60 of the 200 missing references on a real drive were `Ableton Live 11 Suite.app`'s own drum kits. Calling those "missing" said the user's audio was lost and stopped the snapshot | A sample inside an application bundle is Live's own content: it is named, and it does not stop a snapshot |
| The original Clavus library became invisible | The Python version kept its sets in `~/Clavus/Projects`. The rewrite scanned only `~/Music` and `~/Documents`, so a library that had been Clavus's own for years was gone | `~/Clavus/Projects` is a standard location again, for the CLI and the TUI |
| A project could only be named from the folder it sat in | `clavus log "My Set.als"` worked only if the path existed as given, so it needed an absolute path or a `--dir` | The standard places are searched when no `--dir` is given, and the name must be unique across all of them |
| The name lookup could walk forever | It had no depth cap and no guard against a folder that links to its own ancestor, and it now walks the whole library | The walk is capped and visits each directory once, as the project scan does |
| An old Clavus store could not be brought across | The older version kept its library in `~/Clavus/Projects` and its history in `~/.clavus`. The rewrite had no way to read either, so a history years in the making was unreachable | `clavus import-legacy` reads the old store and writes the history, the notes and the bytes into the current one |
| A migrated blob could not be replaced or deleted | The old store's files are often read-only, and a backup's access list can forbid deleting them. A plain copy keeps both, and a blob that cannot be replaced breaks the store the next time those bytes are written | The bytes are written to a fresh file, so the destination has the permissions and no access list |
| A migrated note could not be read | The older version wrote `pending` in lower case; this one writes `Pending`, and the reader only knew one spelling | The spelling is matched without regard to case |
| A dry run left a store behind | The store's directories were created before the migration knew whether it was a dry run | The store is opened for writing only when something is actually written |
| The wrong file of two with the same name was used | A library can hold `Me And You/Me And You.als` and `Me And You/Me And You Project/Me And You.als`. The old store named the right one, and the part of its path after the library folder is still true | The old path is mapped onto where the sets live now, and the name is only a fallback |
| The audit depended on where the command was run | A relative sample reference was resolved against the project *and* against the process's current directory. A file with the same name in the working directory was picked up instead of the copy inside the project, and the report said the sample was outside the project — so two runs of the same command could disagree | A reference is resolved against the project only. A bundle's relative reference points inside the bundle, so the bundle is asked before the disk |
| A renamed drive looked like a lost library | A set writes down the absolute path of every sample. When the drive was renamed and the machine was rebuilt under a different user name, those paths stopped resolving and 327 samples on a real drive were reported missing | The user says what moved where — `clavus relocate map <old> <new>` — and the audit asks that map before it calls a sample missing. On the same drive, 305 of those 327 were found again |

---

## The AI Beta Wave

Six AI beta agents (Ableton producer, Bitwig producer, collaborator,
adversary, CLI ergonomics, TUI navigator) ran the compiled binary
against isolated sandboxes. Every finding was reproduced by hand before
a line of code changed, and every fix has a regression test.

| Finding | Symptom | Fix | Test |
|---------|---------|-----|------|
| A1 | `audit` and `status` gave a clean bill of health to a Bitwig bundle whose `project.xml` was corrupt or missing | The bundle is rejected before it is audited | `core/tests/beta_regressions.rs` |
| A2 | A `.als` truncated by one byte audited as "0 samples present" | The document must end in `>` | `core/tests/beta_regressions.rs` |
| A3 | A dead relay printed "✓ Already up to date" and exited 0 | A missing answer is an error | `cli/tests/beta_cli_tests.rs` |
| B1 | A pull rewound the local HEAD to an older snapshot and dropped the newer one | A pull only advances to a descendant of the local HEAD | `cli/tests/cli_tests.rs` |
| B2 | The relay merged two projects with the same file name | Every project has its own chain, cues and relay bucket | `relay/tests/real_relay.rs` |
| B3 | Restoring an older snapshot dropped the newer ones from the history | The history is kept; the restore says so | `cli/tests/cli_tests.rs` |
| B4 | `audit` and `status` reported different sample counts for the same bundle | Both use one summary | `cli/tests/beta_cli_tests.rs` |
| B5 | The error message told the user to run `clavus collect --copy-missing` — a flag that does not exist | The hint names a command that works | `core/tests/beta_regressions.rs` |
| B6 | `clavus cue` accepted "abc" as a position and reported success | A position must be `bars.beats.sixteenths` | `cli/tests/cli_tests.rs` |
| B7 | `clavus tui` exited 101 with no message when the terminal had no size | The tool says why it cannot start | `tui/src/ui.rs` |
| B8 | The TUI's cue form swapped the message and the position | The message is asked for first | `tui/tests/beta_tui_tests.rs` |
| B9 | A `q` could not be typed in a text field | The text field takes the key | `tui/tests/beta_tui_tests.rs` |
| B10 | The TUI showed 120 BPM and 0 tracks for every project | The real values are parsed | `tui/tests/beta_tui_tests.rs` |
| B11 | The TUI listed only the top folder | The scan is recursive, with a depth cap | `tui/tests/beta_tui_tests.rs` |
| B12 | The TUI could not talk to a relay on another machine | The relay address is editable | `tui/tests/beta_tui_tests.rs` |
| B13 | The cue filter stopped after "skipped" | Every status can be filtered on | `tui/tests/beta_tui_tests.rs` |
| B14 | Every cue was authored by the constant "cli" | The author is the user's name, or `--author` | `cli/tests/beta_cli_tests.rs` |
| B15 | A push dropped the cues of a project with no snapshots | The cues are collected before the early return | `cli/tests/beta_cli_tests.rs`, `relay/tests/real_relay.rs` |
| B16 | A push to a firewalled address froze for 75 seconds | The connect gives up after ten seconds | `sync/tests/sync_client.rs` |
| B17 | The relay's per-project order was rebuilt from a hash map, so it changed on every restart | The order is rebuilt from the parent links | `relay/tests/real_relay.rs` |
| B18 | The snapshot log printed UTC times with no label | The times are local, and the offset is named | `core/src/time.rs` |
| B19 | The TUI's idle phrase never changed | The phrases rotate | `tui/tests/beta_tui_tests.rs` |
| B20 | `Esc` did not walk back one level in the TUI | `Esc` goes back and drops the text | `tui/tests/beta_tui_tests.rs` |

---

## The Second Beta Wave

A second round of AI beta agents ran against the compiled binary with
the first wave's fixes in place. Four findings came back from the
collaborator persona. Each was reproduced by hand, then written down as
a failing test before a line of code changed.

| Finding | Symptom | Fix | Test |
|---------|---------|-----|------|
| W1 | A pull handed over the snapshots and notes of a different project that merely had the same file name | A project is identified on the wire by its file name *and* the folder it lives in | `cli/tests/wave2_tests.rs`, `relay/tests/real_relay.rs` |
| W2 | The relay said it had no bytes for a snapshot it did have, and only the first snapshot's bytes ever arrived | The "the blob is finished" message of one snapshot no longer fails the fetch of the next | `cli/tests/wave2_tests.rs` |
| W3 | A fresh collaborator's pull put the head on the *oldest* snapshot, so the newer ones were missing from the log and the status | The head goes on the newest snapshot of the pulled history — the one that is nobody's parent | `cli/tests/wave2_tests.rs`, `cli/tests/cli_tests.rs` |
| W4 | A push that reported a conflict and exited non-zero had in fact already been stored by the relay, which also moved the project's head | A refused push is not stored, and the head does not move | `cli/tests/wave2_tests.rs`, `relay/tests/real_relay.rs` |

### How a project is identified on the wire

The wire name of a project is its file name with its extension, so an
Ableton set and a Bitwig bundle with the same stem stay apart, and two
machines holding the same project agree on the name.

The wire name alone is not enough: `Album1/Track.als` and
`Album2/Track.als` are two different sets. A client therefore says which
folder it is in, and the relay files the project under
`<file name>::<folder path>`. A colon inside either part is escaped, so
a file called `A.als::B` in the folder `C` cannot be the same project
as `A.als` in the folder `B::C`.

When a client names a file the relay has never seen in that folder, the
relay falls back to a project the client already has history from, and
then to the only project with that file name. If the relay holds several
projects with that name and none of them can be identified, it serves
**none** of them: handing over the wrong project's history is worse than
handing over nothing.

A relay written before this change keeps its history: the old heads and
notes are moved to the project's new key when the relay starts.

### The blob transfer protocol

```
push:  BlobPush { project, hash, size }
       BlobChunk { project, hash, index, data }   (3 MiB raw, base64 in the frame)
       BlobEnd   { project, hash }
       → BlobStored { hash }      the relay has the whole blob, hash verified

pull:  BlobRequest { project, hash }
       → BlobChunk { … }          one chunk at a time
       → BlobDone { hash }        no more chunks
       → BlobMissing { hash }     the relay does not have those bytes
```

The relay only acknowledges a blob once every chunk has arrived and the
whole blob hashes to the name it was sent under, so a peer cannot make
the relay file bytes under someone else's name. The CLI sends the bytes
*before* the snapshot metadata, so the relay owns the bytes by the time
it learns about the snapshot. The pull fetches the bytes before it saves
the metadata, and says which of the pulled snapshots can be restored here
and which cannot.

---

## The Adversary's Findings

The adversary persona of the second wave looks for data loss, crashes
and anything that writes outside the directory it should. It found
three real defects, each reproduced by hand before it was fixed.

| Finding | Symptom | Fix | Test |
|---------|---------|-----|------|
| A1 | `collect` overwrote a sample that was already in the project, destroying the user's own audio and leaving two tracks sharing one file | A name that is already taken in the project is not written over. The sample gets a name of its own, and a file with the very same bytes is reused rather than copied | `cli/tests/adversary_tests.rs` |
| A2 | A pull against a relay that only acknowledged printed both "✗ nothing was checked" and "✓ up to date", and exited 0 | Only an answer that carries the project's history counts as an answer. Anything else leaves the pull waiting, and a pull that checked nothing fails | `cli/tests/adversary_tests.rs` |
| A3 | A snapshot record that could not be parsed made `log` say "No snapshots yet" and `status` say "1 snapshot(s) (HEAD: )" | A record that exists but cannot be read is reported as corrupt, by both commands | `cli/tests/adversary_tests.rs` |

A corrupt note is reported the same way: `clavus cues` says the note is
corrupt instead of showing an empty list.

## The Third Beta Wave: The CLI Ergonomics Persona

The third wave ran two personas against the compiled binary: one that
worked the command line the way a script would, and one that looked for
data loss. They filed nine and six findings. Every one was reproduced by
hand — twice — before a line of code changed, and every fix has a
regression test in `crates/cli/tests/ergonomics_tests.rs`.

| Finding | Severity | Symptom | Fix |
|---------|----------|---------|-----|
| E1 | critical | Two projects whose folders share a *name* shared one snapshot chain, one note list and one relay identity: `notes/p1/studio/Song.als` and `notes/p2/studio/Song.als` produced a single entry in the data directory | The storage key is the file name and a digest of the whole path, so no two projects can share a chain. The wire identity carries the folder's *path*, not just its name |
| E2 | critical | `collect` on a `.bwproject` copied a sample *next to* the bundle and pointed the reference inside the bundle at the copy it did not carry — the project opened without its audio, and the audit called it healthy | A bundle stays self-contained: the bytes go into the archive and the reference points at the copy inside it |
| E3 | major | `clavus audit` wrote its whole report to stderr and left stdout empty — exactly when there was something to say | The audit's result goes to stdout, like the clean report and like `status`. Errors stay on stderr |
| E4 | minor | `collect` on a bundle whose samples were already inside it claimed "Collected 3 samples into project/Samples/" — a folder that was never created | The message says what actually happened: the samples are already inside the bundle |
| E5 | minor | A note was signed with the literal word "cli" when `USER` and `LOGNAME` were unset — the case a container, a cron job or CI always hits | The fallback names the host, or says "unknown". It is never "cli" |
| E6 | minor | A pull printed "the relay had nothing new for this project" and then imported a new note | The summary says "no new *snapshots*" and names the notes it imported |
| E7 | minor | Ctrl-C did nothing in the TUI: the terminal is in raw mode, so the interrupt arrives as an ordinary key | Ctrl-C quits, from any screen, including a text field |
| E8 | minor | `clavus cues` never showed the cue's track, although `--track` was stored | The track is a column of the listing |
| E9 | minor | `restore` and `log` treated a directory as a project and said "no snapshots yet" | A directory is refused with the same clear message `audit` gives |
| E10 | minor | `collect` replaced a symlinked project file with a regular file, so the user's project became a copy | A symlinked project is written *through* the link |
| E11 | minor | Two projects whose names contain `::` mapped to the same relay key, so one project's push was refused as a conflict with the other's history | The parts of a relay key are escaped, so a colon cannot forge another project's identity |
| E12 | minor | A pull warned that "the relay's history does not extend yours" when the relay merely re-offered the snapshot the client already had | Re-offering what we have is not a divergence; the pull says so |

The head of a project on the relay also only moves *forward* now. A
collaborator whose own head is behind used to drag the relay's head back
to an older snapshot, and the next client to pull was offered the older
history as if it were the newest.

### How a project is identified

Three names, each answering a different question:

| Name | Question | Built from |
|------|----------|-----------|
| **the storage key** | *which chain is this?* | the file name and a digest of the whole path — `Song.als__a1b2c3d4e5f6` |
| **the wire name** | *which project is this?* | the file name with its extension — `Song.als` |
| **the folder path** | *which one, when two share a name?* | the folder's path below the home directory — `music/p1/studio` |

The storage key is a digest of the path because a folder's *name* is not
enough: `p1/studio/Song.als` and `p2/studio/Song.als` share both the
folder name and the file name, and they are two projects. The digest
makes the collision impossible.

The folder path is relative to the home directory, so two machines that
keep the same tree agree on it, and the user's name is not part of it.
A project that lives outside the home directory keeps the last two
folders of its path — `media/p1/studio` — because the name of one
folder was not enough: `external/p1/studio` and `external/p2/studio`
kept one identity between them, and the relay served one the other's
history and notes. The relay tells two projects of the same name apart
by their bytes and by their history as well.

Two folders of the same name at the *same depth* are two projects a
user keeps side by side, and one must never be served the other's
history however identical their bytes are. Two folders of the same name
at *different depths* are a collaborator's copy of a set, and the bytes
of the file are what prove it. `folders_are_siblings` draws that line,
and two relay regressions cover both directions.

### The Fourth Beta Wave: The TUI Navigator

The wave-four TUI persona drove the TUI through its own code paths and
through the real binary in a pty, and filed fourteen findings. Every
one was reproduced by hand before a line of code changed, and every fix
has a regression in `crates/tui/tests/wave4_tests.rs` that drives the
real application state and the real key handler.

| Finding | Severity | Symptom | Fix |
|---------|----------|---------|-----|
| B1 | critical | Two projects outside the home whose folders shared a name had one relay identity: the relay served one the other's history and notes | The folder path keeps the last two components of the path |
| B2 | critical | A pull replaced the HEAD with the relay's branch, so the user's own snapshot dropped out of the chain | The HEAD only moves to a descendant of the local HEAD — the rule the CLI already used, now shared by both |
| B3 | major | A window smaller than the TUI needs killed the app with exit code 101 and no message at all | The layout refuses to draw below `60x10` and says so; the panic hook lets the message through |
| B4 | major | Any panic was completely silent — the hook replaced the one that prints it | The hook restores the terminal and then calls the default hook |
| B5 | major | The cue and snapshot lists drew no selection, so the arrow keys moved a cursor nobody could see | The selected row is highlighted |
| B6 | major | A note made in the TUI was signed "tui", so a collaborator saw one person as two authors | The TUI signs it the way the CLI does |
| B7 | major | An unreadable data directory was shown as "0 snapshots" | The TUI and `clavus log` both name the problem |
| B8 | minor | The project list was never refreshed, so its counts disagreed with the CLI | It is refreshed as the user moves through it |
| B9 | minor | Notes could not be pushed without a snapshot, though the CLI pushes them on their own | The TUI does too |
| B10 | minor | The sync screen kept saying "connected" after the relay had died | The connection is noticed and the screen says it was lost |
| B11 | minor | Esc from the sync screen landed on an empty project screen | It goes back to the screen the user came from |
| B12 | minor | The relay field was prefilled, so typing a new address produced a nonsense one | The field starts empty |
| B13 | minor | Two projects with the same file name were indistinguishable in the list | The list shows the folder each project lives in |
| B14 | minor | A directory named `Folder.als` was listed as a project and failed to open | A directory is not a project, however it is named |

### The Beta Harness

The harness lives outside the repository, in `~/agent-state/beta-lab/`:

- `playbook.md` — the rules the agents follow
- `charters/` — one charter per persona
- `seed.sh` — creates an isolated sandbox per agent
- `run.sh` — runs the agents in parallel with a time limit
- `jsonl_to_text.py` — renders Pi's JSONL events into readable logs
- `findings/` — the findings each agent filed
- `sandboxes/` — the isolated data directories

Every agent runs the **compiled binary** in its own `CLAVUS_DATA`
sandbox. The repository is never touched.

---

## The Mouse and the Scrolling It Needed ✅

Clavus is keyboard-first, and the keyboard still does everything. The
mouse is a second way in, not the only way: nothing is reachable by
mouse that is not reachable by keyboard. The tests in
`crates/tui/tests/mouse_tests.rs` drive the real application state,
the real renderer and the real mouse handler — nineteen of them.

| Gesture | What it does |
|---------|--------------|
| Wheel | Moves the selection; the list scrolls to keep it on screen |
| Click a row | Selects it |
| Double click a row | Opens it — the same as `Enter` |
| Drag with the button held | The row under the pointer becomes the selected one |
| Click the scrollbar | Jumps the window to that part of the list |

**The mouse is told what is on the screen by the renderer**, which
records a map of the rows it drew. The layout is computed in one
place, so the map can never disagree with what is on screen, and a
test can check the map against the drawn rows rather than against a
hand-written copy of the layout.

**The list did not scroll at all.** This was found by driving the real
binary in a pty with forty-five sets and pressing the down arrow
twenty-five times: the cursor walked off the bottom of the screen and
the list never moved. A thousand sets do not fit in a terminal, so
the renderer now works out how many rows fit, scrolls the window to
keep the cursor on the screen, and draws a scrollbar down the right
edge. The scrollbar is a control, not a picture — clicking it jumps
the window, and the cursor goes with it.

**The cue list's arrow keys were moving through the wrong list.** The
cue filter changes which notes are drawn, but the keys walked the
unfiltered list while the highlight was computed on the filtered one,
so the cursor could land on a note that was not on screen. The cue and
snapshot lists now have the same filter-aware navigation the project
list has always had.

**Two things about the terminal that are easy to get wrong.**

- The capture has to be undone on **every** way out of the program:
  the normal exit, the error paths, and the panic hook. A terminal
  left reporting mouse events prints escape sequences into the shell
  after Clavus has gone.
- crossterm asks the terminal to report **every movement** along with
  presses and releases. For a full-screen program that is the wrong
  thing: every twitch of the mouse arrives as an event, and every
  event draws a frame. The "any movement" mode is turned off again
  straight away. Presses, releases, and the movement that happens
  while a button is held — which is how a list is dragged — are
  enough.

## Browsing a Library of Projects ✅

A user with a thousand sets needs two questions answered at a glance:
*which one do I want*, and *which ones am I actually working on*. The
list now answers both, and the tests in
`crates/tui/tests/workbench_tests.rs` drive the real application state
and the real key handler — twenty-four of them.

What the list shows:

| Column | What it answers |
|--------|------------------|
| `SNAPS` | How much history this set has |
| `WORK` | The shape of that history — one bar per slice of the project's own life, tallest where the work clustered |
| `OPEN` | How many notes are still open on it |
| `AGE` | How long ago the file itself was last edited |
| `FOLDER` | Where the set lives, so two sets with the same name can be told apart |

and a strip above the table naming the set under the cursor, so the
selection is never a guess.

**The history shape had to be made honest.** The first version divided
the project's own lifetime into eight slices and put each snapshot in
the slice it belonged to. A producer takes six snapshots in one
afternoon, and six snapshots inside six seconds land in six different
slices — so the chart was a row of bars that said nothing at all. The
axis now has a floor of one day: work done in one sitting reads as
one burst, and work spread over months reads as a spread. The
difference between the two is the thing worth seeing.

**A bug that only a real terminal could find.** crossterm reports an
uppercase letter with the shift modifier set, because that is what the
keyboard physically did: typing `S` arrives as `Char('S')` plus
`SHIFT`. Every key test in the suite built its events by hand with no
modifiers, so the capital-letter shortcuts were alive in the tests and
dead in the program — `S` to sort, `P` to push and `G` to pull on the
sync screen all did nothing, and no capital letter could be typed into
a note or a search box. The keys are now normalised once, at the top
of the handler, and the shift modifier is only dropped when no other
modifier is held, so Ctrl-Shift-C is not read as a capital C.

The regression tests for this use the shape a terminal actually sends
(`Char('S')` with `SHIFT`), not the shape the old tests invented.

**The footer was rebuilt rather than patched.** The old footer was one
line of text with a message spliced into it, so a long message shoved
the key hints and the relay's state sideways. The footer is now three
regions of fixed width: what the list is doing on the left, a message
in the middle, and the relay's state on the right. A message of any
length is truncated rather than allowed to move the layout.

**Narrow terminals.** The columns drop optional fields as the terminal
gets narrower, but the history shape is never squeezed — it is the
column worth keeping. A terminal too small to draw at all says so
rather than panicking, and a title measured in bytes rather than
characters used to panic on a name with an accent in it.

---

## Testing Against a Real Studio Library ✅

Every parser test until now used a synthetic fixture. A studio drive
with a real library is a better teacher: 1,245 sets, written by Live 8,
9, 10, 11 and 12 across ten years, with none of them written to be
easy to parse.

**What the real library found.** Every single one of those 1,242 sets
Clavus could read parsed as *one track, 120 BPM, no devices, no track
names* — and 120 BPM is the default, so the number was not a
measurement but a constant. Five separate causes, all of them invisible
against a synthetic fixture:

1. **Skipping an element consumed the rest of the document.** The
   routine that skipped past a subtree only counted the closing tags
   that shared the target's name. Every ordinary element inside the
   subtree pushed the count up and it never came back down, so the
   skip ran to the end of the file. Every real set has a device chain
   with ordinary elements inside it, so every real set lost everything
   after its first device chain.

2. **The track parser never ended.** It counted nesting depth to know
   when a closing tag ended the track, but it counted *every* element
   and compared against zero — and the count was one when the track's
   own closing tag arrived. So the first track swallowed the rest of
   the file. It now counts only elements with the same name as the
   track, because Live 8 nests a `<Track>` inside every `<Mixer>`.

3. **The tempo was read from the wrong place.** The project's tempo
   lives in the master track. Live 10 and later write
   `<Tempo><Manual Value="92"/></Tempo>`; Live 8 and 9 write the same
   shape; older sets write the tempo as an automation curve. None of
   those were read, so the default survived. Worse, Live 11 and 12 give
   *every track* its own tempo, so the last one read would have won
   — and a tempo read inside a track or a scene is not the project's
   tempo at all.

4. **The device list was the whole document.** Live keeps the devices
   in a `<Devices>` list, inside a `<DeviceChain>` that is itself
   nested inside the track's `<DeviceChain>`. Reading every element in
   the chain as a device produced a list of every element in the file.

5. **Live writes different shapes in different years.** A track's name
   is `<Name Value="Kick"/>` in Live 8 and
   `<Name><EffectiveName/><UserName/></Name>` from Live 9. Live 12
   renamed `<MasterTrack>` to `<MainTrack>`. MIDI keyboard tracks are
   `<KeyTrack>` and carry no name at all.

**What it looks like now.** 1,242 of the 1,245 sets are readable, and
every one of those reports its real track count (5 to 26), its real
tempo (1,186 of them really are 120 BPM — the rest are 91.4 to 128),
its real track names, and its real device names. The remaining three
are rejected with a sentence that says what to do about them, which is
described next. The library list in the TUI shows those numbers, and
they are the numbers the sets actually contain.

**Three sets cannot be read, and say so.** Live wrote some sets in its
own binary format before gzip; it is a serialisation, not XML, so
there is nothing to decompress into text. Three sets on this drive are
in it. Clavus names the problem instead of reporting a decoding
failure: *"this set uses Live's old compressed format, which Clavus
cannot read; open it in Live and save it again to convert it."* Three
sets out of 1,245 is not worth a binary decoder; a clear sentence is.

**The regression tests** live in
`crates/parser-als/tests/real_world_tests.rs` — seventeen of them,
each built from the shape a real set of that Live version has. They
are the tests that would have caught all five of the causes above.

**The library also had to become openable.** The first version of the
list resolved every folder to its real path and parsed every project
before the first screen appeared: 13 seconds for the command line and
**74 seconds** for the TUI on this drive. The list now appears in
**0.33 seconds**, with the numbers arriving in the background as they
are read. The tests for that are in `crates/tui/tests/scale_tests.rs`.

---

## The Library That Moved 🧭

A set writes down the absolute path of every sample it uses. This
drive's sets were made over ten years, on machines that were renamed,
on a drive that was renamed, and in a home folder that belonged to
somebody else. Every one of those paths stopped resolving, and the
audit said the audio was gone.

**What the drive actually said.** 234,167 sample references across the
1,245 sets. 222,269 are relative and resolve against the set. The
rest are absolute, and they name four places that no longer exist:

| What the set points at | References | What it is now |
|---|---|---|
| `/Volumes/LaCie/Create ` | 4,764 | still there — the drive kept the folder |
| `/Users/chriscarr/Splice` | 3,764 | `/Volumes/LaCie/new age projects back ups` |
| `/Users/chriscarr/Library/Mobile Documents/…` | 868 | this machine's iCloud library |
| `/Users/chriscarr/Music`, `/Users/chriscarr/Downloads` | 72 | genuinely gone — 12 files |

**What Clavus does about it.** It keeps a map the user writes:

```bash
clavus relocate map "/Users/chriscarr/Splice" "/Volumes/LaCie/new age projects back ups"
clavus relocate map "/Users/chriscarr/Library/Mobile Documents/com~apple~CloudDocs" \
                   "/Users/chris/Library/Mobile Documents/com~apple~CloudDocs"
clavus relocate list
```

The map is asked **last**, after the path the set names has been tried.
A rule that is out of date cannot move a sample that was already where
the set said it was. A rule only matches on a path-component boundary,
so `/Volumes/Set` never rewrites `/Volumes/Set Two`.

**What it is worth, on the real drive.** Across the 38 sets that point
at the old home folder:

| | Missing | Outside the project |
|---|---|---|
| Without the map | **327** | 31 |
| With the map | **22** | 57 |

305 samples that the audit called lost are found again. The 26 that
moved from "missing" to "outside the project" are audio that is present
and simply has not been collected into the set yet — which is what
`clavus collect` is for. The 22 that stay missing are the Live
recordings and the stems that were never copied to the drive.

**Why Clavus does not go looking.** It could search the drive for a file
with the same name — 242,843 files, and for 151 of the references there
is more than one match. A search would be slow, and it would sometimes
pick the wrong copy of the same audio. The map is one line per move,
it is visible, and it can be corrected. Clavus does not guess.

**The tests** are in `crates/core/tests/relocate_tests.rs` (10) and
`crates/cli/tests/relocate_tests.rs` (7). They drive the real map, the
real audit and the real collect.

---

## CI Integration

```yaml
# .github/workflows/test.yml
name: Tests
on: [push, pull_request]
jobs:
  test:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Run all tests
        run: cargo test --all
      - name: Clippy
        run: cargo clippy --all -- -D warnings
      - name: Build release
        run: cargo build --release
```

---

## Test Fixtures

All test fixtures live in `~/Projects/clavus-rust/fixtures/`:

| File | Source | Purpose |
|------|--------|---------|
| `test_project.als` | `gen_fixture.py` | Minimal valid .als — 8 tracks, 5 markers, 128 BPM, no samples |
| `test_project.bwproject` | **User needed** | Any real Bitwig project |

**To generate a realistic .als fixture:**

```bash
cd ~/Projects/clavus-rust/fixtures
# Option A: use gen_fixture.py (already has 8 tracks)
python3 gen_fixture.py

# Option B: export a real Ableton project as .als and test with it
# (most valuable — tests real Ableton XML structure)
```

---

## Running Tests

```bash
cd ~/Projects/clavus-rust/rust

# All tests
~/.cargo/bin/cargo test --all

# One crate
~/.cargo/bin/cargo test -p clavus-core

# With output
~/.cargo/bin/cargo test --all -- --nocapture

# Specific test
~/.cargo/bin/cargo test -p clavus-parser-als test_sample_ref

# Fuzz once (not via cargo-fuzz)
~/.cargo/bin/cargo test -p clavus-parser-als test_fuzz

# Coverage
~/.cargo/bin/cargo install cargo-llvm-cov
~/.cargo/bin/cargo llvm-cov --all
```

### The two-machine relay test

```bash
cd ~/Projects/clavus-rust

# Three Linux machines in Docker, one relay between them.
# Needs Docker; nothing is written outside a temporary directory.
bash scripts/two-machine-relay-test.sh

# Use a binary you already have, and pick the container's architecture.
CLAVUS_TEST_BIN=rust/target/x86_64-unknown-linux-musl/release/clavus \
CLAVUS_TEST_PLATFORM=linux/amd64 \
  bash scripts/two-machine-relay-test.sh
```

It builds the Linux musl binary, so it also proves the build that ships
to the studio runs on Linux.
