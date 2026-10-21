# Clavus Beta Testing Guide

> You're the first real user! This guide walks through every feature systematically.
> Found a bug? The "Report" section at the end tells you how to file it.
>
> Estimated time: **45–90 minutes** depending on how deep you go.

---

## Before You Start

### Installation

```bash
# Download the latest release for your platform
# macOS ARM64 (Apple Silicon):
curl -L https://github.com/castle-queenside/clavus/releases/latest/download/clavus-macos-arm64 -o clavus
chmod +x clavus
mv clavus /usr/local/bin/

# Linux x86_64:
curl -L https://github.com/castle-queenside/clavus/releases/latest/download/clavus-linux-x86_64 -o clavus
chmod +x clavus
sudo mv clavus /usr/local/bin/
```

Verify it works:
```bash
clavus --version
# Expected: clavus 0.1.0
```

### What You'll Need

- [ ] **At least one Ableton Live or Bitwig Studio project** on this machine
- [ ] Optional: a project with **external samples** (samples outside the project folder)
- [ ] Optional: a project with **missing samples** (sample files that were moved/deleted)
- [ ] For sync testing: **two machines on the same network** (or a friend to test with)

---

## Part 1: Installation & First Run

### 1.1 `clavus --version`

**What it does:** Prints the version.

**Test:**
```bash
clavus --version
```

**Expected:**
```
clavus 0.1.0
```

**What to watch for:** Any error about missing libraries (`libssl`, `libcrypto`, etc.)

### 1.2 `clavus --help`

**What it does:** Shows all available commands.

**Test:**
```bash
clavus --help
```

**Expected:** A list of commands including `snapshot`, `log`, `audit`, `collect`, `relay`, `tui`, `cue`, `cues`.

**What to watch for:** Commands that are listed but crash when you try them.

---

## Part 2: Sample Audit

The audit checks whether all audio samples referenced in your project are present and inside the project folder.

### 2.1 Audit a Clean Project

**Setup:** Find a project where all samples are inside the project folder.

**Test:**
```bash
clavus audit "/path/to/YourProject.als"
```

**Expected output (all samples present):**
```
✓ 12 samples present
```

**Expected output (some missing):**
```
⚠ 3 sample(s) missing — run `clavus collect` first
  3 missing  ·  0 outside project
```

### 2.2 Audit with Outside-Project Samples

**Setup:** Find a project that references samples outside its folder (e.g., samples in a shared library).

**Test:**
```bash
clavus audit "/path/to/project.als"
```

**Expected:**
```
⚠ 5 sample(s) outside project folder — run `clavus collect --copy-missing`
  0 missing  ·  5 outside project
```

### 2.3 Audit a Bitwig Project

**Setup:** Find a Bitwig project (`.bwproject` bundle/folder).

**Test:**
```bash
clavus audit "/path/to/YourProject.bwproject"
```

**Expected:** Same behavior as `.als` — shows sample count and warnings.

**What to watch for:**
- Does it find samples correctly?
- Does it handle Bitwig's ZIP format without errors?
- Any path format differences from Ableton?

### 2.4 Audit a Bitwig Project That Was Shared With You

**Setup:** A `.bwproject` that came from another machine — exported from
Bitwig with "copy samples" on. The archive holds a copy of every sample,
but the references inside it still point at the other machine's paths.

**Test:**
```bash
clavus audit "/path/to/SharedProject.bwproject"
```

**Expected:**
```
✓ 24 samples present
  24 samples inside the project archive — run `clavus collect` to extract
```

**What to watch for:** The samples must **not** be reported as missing.
They are present — they are in the archive. If they are reported missing,
the audit does not understand the Bitwig bundle.

---

## Part 3: Collecting Samples

Collect copies external/missing samples into `YourProject_Samples/` and rewrites the `.als` to point to the local copies.

> ⚠️ **Before testing:** Back up your project. Collect modifies the `.als` file's internal sample paths.

### 3.1 First Collect (Samples Outside Project)

**Setup:** A project with samples outside its folder.

**Test:**
```bash
# BEFORE: note the external sample paths
clavus audit "/path/to/project.als"
# Note: shows N outside project

# DO THE COLLECT
clavus collect "/path/to/project.als"

# AFTER: re-audit
clavus audit "/path/to/project.als"
```

**Expected output for `collect`:**
```
⚠ 3 sample(s) outside project folder — run `clavus collect --copy-missing`
  0 missing  ·  3 outside project
✓ Collected 3 sample(s) into project/Samples/
  Paths in .als rewritten — restart Ableton to reload
```

**Verify:**
1. A new `projectname_Samples/` folder appeared next to your `.als`
2. The samples are inside it
3. Opening the project in Ableton and re-scanning shows the same samples

### 3.2 Second Collect (Idempotent)

**Test:**
```bash
clavus audit "/path/to/project.als"
```

**Expected:**
```
✓ 12 samples already present, all inside project folder
```

Running collect twice on a clean project should be a no-op.

### 3.3 Collect a Project with Missing Samples

**Setup:** A project where some referenced samples no longer exist on disk.

**Test:**
```bash
clavus collect "/path/to/project.als"
```

**Expected:**
```
⚠ 2 sample(s) missing — run `clavus collect` first
  2 missing  ·  0 outside project
Collecting 0 sample(s) into project/Samples/
  ⚠ 2 sample(s) could not be copied:
    /old/path/kick.wav — file not found on disk
    /old/path/snare.wav — file not found on disk
```

**What to watch for:** Does it clearly report which files are missing rather than crashing or silently failing?

### 3.4 Collect a Bitwig Project That Carries Its Own Samples

**Setup:** The shared `.bwproject` from test 2.4.

**Test:**
```bash
clavus collect "/path/to/SharedProject.bwproject"
```

**Expected:**
```
  0 missing  ·  0 outside project  ·  24 inside archive

✓ Collected 24 sample(s) into project/Samples/
  Paths in the project archive rewritten — reopen the project in Bitwig
```

**Verify:**
1. A `Samples/` folder appeared next to the `.bwproject`
2. The samples are in it
3. Re-running `clavus audit` now says the samples are present with no
   "inside the project archive" note
4. Opening the project in Bitwig still finds its samples

**What to watch for:** The references inside the archive must be
rewritten to the extracted copies, and the rest of the archive must
survive the rewrite. If Bitwig cannot open the project afterwards, the
rewrite is wrong.

---

## Part 4: Snapshots

A snapshot captures a point-in-time state of your project. Think of it like `git commit`.

### 4.1 First Snapshot

**Setup:** Any project (ideally one with samples already collected/audited).

**Test:**
```bash
clavus snapshot "/path/to/project.als" -m "Initial idea"
```

**Expected:**
```
✓ snapshot a1b2c3d4  ·  2026-10-20 14:30  ·  8 tracks  ·  128 BPM
```

### 4.2 Second Snapshot (After Changes)

**Setup:** Open the project in Ableton, make a small change (e.g., move a marker, rename a track, add a device), save.

**Test:**
```bash
clavus snapshot "/path/to/project.als" -m "Added reverb to vocals"
```

**Expected:**
```
✓ snapshot e5f6a7b8  ·  2026-10-20 14:35  ·  8 tracks  ·  128 BPM
```

### 4.3 Snapshot Blocks on Missing Samples

**Setup:** Temporarily move a sample file that the project references.

**Test:**
```bash
clavus snapshot "/path/to/project.als" -m "Should be blocked"
```

**Expected:** Should **fail** or **warn** that samples are missing.

**What to watch for:** Does it produce a clear error message, or does it silently create a bad snapshot?

### 4.4 A Snapshot of Unchanged Content

**Setup:** Snapshot the project twice in a row without changing anything.

**Test:**
```bash
clavus snapshot "/path/to/project.als" -m "Same content again"
```

**Expected:** The tool says the content is unchanged and that no new
snapshot was created — it does **not** print a "✓ snapshot" line for
work it did not store:
```
= snapshot a1b2c3d4  ·  unchanged — no new snapshot was created
  (already in this project's history as "Initial idea")
  (the message "Same content again" was not recorded — change the project first)
```

**What to watch for:** A "✓" for a snapshot that does not appear in
`clavus log` is a false success. Report it.

### 4.5 View Snapshot Log

**Test:**
```bash
clavus log "/path/to/project.als"
```

**Expected:** A list showing the chain of snapshots with timestamps and messages:
```
2 snapshot(s) in chain — times are local (UTC-07:00)

  e5f6a7b8  2026-10-20 14:35  Added reverb to vocals  [8 tracks · 128 BPM]
  a1b2c3d4  2026-10-20 14:30  Initial idea  [8 tracks · 128 BPM]
```

**What to watch for:** The times must match your own clock. If they are
in UTC, or if the header does not say which clock they are on, that is a
bug — report it.

---

## Part 5: Cues

Cues are timeline annotations — things to fix, questions, or notes at specific positions.

### 5.1 List Existing Cues

**Test:**
```bash
clavus cues "/path/to/project.als"
```

**Expected:** Shows all cues for this project (empty list is valid).

### 5.2 Add a Cue

**Test:**
```bash
clavus cue "/path/to/project.als" -t "Fix bass level" -p 4.1.1
```

**Expected:**
```
✓ cue 9a8b7c6d  @  4.1.1  (pending)
  Fix bass level
```

**What to watch for:** The cue must name *you* as the author, not a
constant like "cli" — otherwise you cannot tell whose notes are whose
after a sync. If the author says "cli", report it. You can set it
yourself with `--author "Dana"`.

### 5.2b A Cue With a Bad Position

**Test:**
```bash
clavus cue "/path/to/project.als" -t "typo" -p "4.1"
```

**Expected:** An error. A position must be `bars.beats.sixteenths`, so
`4.1` is not one. A cue stored with a position that can never be
resolved to a time is worse than a refusal.

### 5.3 Resolve a Cue

**Test:**
```bash
clavus cue --resolve 9a8b7c6d
```

**Expected:** Cue marked as resolved (no output = success).

### 5.4 Filter Cues

**Test:**
```bash
clavus cues "/path/to/project.als" --filter pending
clavus cues "/path/to/project.als" --filter resolved
```

**Expected:** Only shows cues matching the filter.

---

## Part 6: TUI (Terminal UI)

The TUI gives you a visual overview of all your projects.

### 6.1 Launch the TUI

**Test:**
```bash
clavus tui
```

**Expected:** A dark terminal UI with:
- Project list showing your projects
- For each project: name, track count, BPM, snapshot count
- Key hints at the bottom

### 6.1b The TUI's Project List

**What to watch for:**

- The **track count** and **BPM** must be the project's own, not the
  same numbers for every project. A row that always says "120 BPM" and
  "0 tracks" is a bug.
- Projects in **sub-folders** must appear. A project the CLI can see
  but the TUI cannot is a bug.
- A project that **cannot be parsed** must show an error, not vanish.

### 6.2 Navigate the TUI

**Test each key:**

| Key | Action | Expected |
|-----|--------|---------|
| `↑` / `↓` | Navigate project list | Selection moves |
| `Enter` | Open project detail | Shows snapshots, cues |
| `←` / `→` | Navigate screens | Switches between views |
| `c` | Snapshot hint | Shows `c: snapshot` hint |
| `a` | Add cue | Opens cue entry form |
| `f` | Filter cues | Cycles: all → pending → resolved → skipped → deferred → archived → all |
| `r` | Resolve cue | Marks selected cue resolved |
| `s` | Sync screen | Shows sync/relay status |
| `P` | Push | Pushes to relay |
| `G` | Pull | Pulls from relay |
| `q` / `Esc` | Quit / go back | `Esc` walks back one screen and drops what you typed |

### 6.3 TUI Snapshot List

**Test:**
1. Select a project with snapshots
2. Press `Enter` to view detail
3. Navigate with `↑`/`↓`

**Expected:** Snapshot list shows hash prefix, timestamp, message, track count.

### 6.4 TUI Filter Cycling

**Test:** With a project open showing cues, press `f` repeatedly.

**Expected:** Filter cycles through states, indicator appears, cue count updates.

---

## Part 7: Relay (Sync)

The relay lets multiple machines sync snapshots and cues over a local network.

### 7.1 Start the Relay

**On Machine A:**
```bash
clavus relay --bind 0.0.0.0:4477
```

**Expected:**
```
Listening on 0.0.0.0:4477…
State: ~/.clavus/relay-state
```

The bind address can also be given positionally: `clavus relay 0.0.0.0:4477`

> **Note:** the project file contents travel with the snapshots. They are
> cut into chunks, sent, and checked against their SHA-256 before the relay
> accepts them — so a pulled snapshot can be restored on the other machine,
> byte for byte.

### 7.2 Push from Machine B

**On Machine B:**
```bash
clavus push --relay 192.168.1.X:4477 "/path/to/project.als"
```

**Expected:**
```
Connecting to 192.168.1.X:4477...
  relay has 3 snapshot(s), 1 cue(s)
Pushing the local chain — 3 snapshot(s), 1 cue(s)...
✓ Pushed the local chain — 3 snapshot(s) and 1 cue(s) sent
```

**What to watch for:** The count is the whole local chain, not the
number that were new to the relay. A push that fails must exit
non-zero — a script that runs `clavus push` has to be able to tell.

### 7.3 Pull from Machine A

**On Machine A (or another machine):**
```bash
clavus pull --relay 192.168.1.X:4477 "/path/to/project.als"
```

**Expected:**
```
Connecting to 192.168.1.X:4477...
  ↳ abc12345  added reverb
  HEAD → abc12345
  ✎ 9a8b7c6d…  Fix bass level
✓ Pulled 1 new snapshot(s)
  1 of them came with the project bytes — they can be restored here.
✓ Imported 1 new cue(s)
```

**What to watch for:**

- A pull must **never** move your HEAD backwards. If you had a newer
  local snapshot and the pull replaced it, your work is gone — report it.
- On a **first** pull, the head must land on the **newest** snapshot, not
  the oldest. Check `clavus log` — every snapshot of the chain must be
  there, and the newest one must be the head.
- A pull against a relay that does not answer must **fail**, not print
  "✓ up to date".
- The relay must not hand you another project's snapshots or cues.
- **Every** pulled snapshot must bring its own bytes. A pull that says
  "the relay has no bytes for this snapshot" for a snapshot the relay
  really has is a bug — report it.
- Two projects with the same file name in different folders are two
  projects. A pull of `Album2/Track.als` must not bring the history or
  the notes of `Album1/Track.als`.

### 7.4 Conflict Detection

**Setup:** Two users make snapshots simultaneously on different machines.

**Test:** Both push at the same time without pulling in between.

**Expected:** At least one client should see a **conflict warning** (not silent overwrite).

**What to watch for:** Does the system detect divergent chains? Does it warn the user rather than silently overwriting?

**And then:** pull on the machine that was refused, so it has the other
side's history, and push again. The push must now succeed.

**What to watch for:** a push that was *refused* must not have been
stored by the relay anyway. After the refusal, a third machine that pulls
the project must see the accepted snapshot and **not** the refused one. If
the refused push turned up anyway, the relay silently overwrote the
history — report it.

### 7.5 Restore a Snapshot That Was Pulled

**On the machine that pulled:**

```bash
clavus restore "/path/to/project.als" HEAD
```

**Expected:** the file is replaced by the version the collaborator pushed,
byte for byte.

**What to watch for:**

- The restore must work. A pull that brought the snapshot but not the
  bytes leaves you with a history you cannot use.
- The restored file must be the *collaborator's* file, not your own — check
  the tempo or the track list, or compare the bytes.
- Take a snapshot afterwards. The chain must keep both versions, and the
  log must show the new one on top.

---

## Part 8: Project Discovery & Status

### 8.1 `clavus projects`

Scans `~/Music` and `~/Documents` for Ableton/Bitwig projects.

**Test:**
```bash
clavus projects
clavus projects --recursive
```

**Expected:** A list of project names, last-modified times, and full paths.

### 8.2 `clavus status`

A one-command overview of a project.

**Test:**
```bash
clavus status "/path/to/project.als"
```

**Expected:**
```
MyProject  (Ableton Live)
──────────────────────────────────────────────────
✓ 12 samples accounted for
  12 on disk in the project folder
📷  3 snapshot(s)  (HEAD: a1b2c3d4 — added reverb)
🎯  4 cue(s)  (2 pending, 2 resolved)
```

**What to watch for:** `clavus status` and `clavus audit` must agree on
the numbers. For a `.bwproject`, the samples inside the archive are
counted separately from the samples on disk — a bundle whose samples
are all inside it should not say "0 samples present".

### 8.3 `clavus restore`

Restores a project to a previous snapshot.

**Test:**
```bash
# Restore to the latest snapshot
clavus restore "/path/to/project.als"

# Restore to a specific snapshot (hash prefix works)
clavus log "/path/to/project.als"
clavus restore "/path/to/project.als" a1b2c3
```

**Expected:**
```
Restoring MyProject to snapshot a1b2c3d4...
✓ Restored to: a1b2c3d4
  added reverb
  Restart Ableton/Bitwig to load the restored file.
```

**What to watch for:** Open the project in Ableton after restoring — does it look like
the state described by that snapshot's message?

### 8.4 `clavus snapshot --collect`

Auto-collects samples before snapshotting instead of failing.

**Test:**
```bash
clavus snapshot "/path/to/project.als" -m "with auto collect" --collect
```

**Expected:**
```
→ Running `clavus collect` to copy samples into project folder...
✓ Collect complete — proceeding with snapshot
✓ snapshot 9f8e7d6c  ·  2026-10-20 15:30  ·  8 tracks  ·  128 BPM
```

### 8.5 Two Projects in One Data Directory

Every project keeps its own snapshot chain, its own cues and its own
history. This test checks that one project never bleeds into another.

**Setup:** Two projects in the same folder.

**Test:**
```bash
clavus snapshot "/path/to/Alpha.als" -m "alpha work"
clavus snapshot "/path/to/Beta.als"  -m "beta work"
clavus snapshot "/path/to/Alpha.als" -m "alpha again"

clavus log "/path/to/Alpha.als"
clavus log "/path/to/Beta.als"
```

**Expected:**
```
$ clavus log Alpha.als
2 snapshot(s) in chain
  …  alpha again  <- …  [8 tracks · 128 BPM]
  …  alpha work   [8 tracks · 128 BPM]

$ clavus log Beta.als
1 snapshot(s) in chain
  …  beta work  [3 tracks · 140 BPM]
```

**What to watch for:** Alpha's log must show two snapshots and Beta's
must show one. If either shows the other's history, the projects are
sharing a chain.

### 8.6 A Snapshot From One Project Must Not Restore Over Another

**Test:**
```bash
clavus log "/path/to/Alpha.als"          # note a short hash
clavus restore "/path/to/Beta.als" a1b2c3d4
```

**Expected:** An error — *"No snapshot found matching 'a1b2c3d4'"* — and
`Beta.als` is **not** modified.

**What to watch for:** This must not silently write Alpha's contents
into Beta.als.

---

## Part 9: Edge Cases

### 9.0 Two Projects With the Same Name

**Setup:** Two different projects that happen to share a file name:

```
~/Music/AlbumA/Track.als
~/Music/AlbumB/Track.als
```

**Test:**
```bash
clavus snapshot "~/Music/AlbumA/Track.als" -m "album A"
clavus snapshot "~/Music/AlbumB/Track.als" -m "album B"
clavus log "~/Music/AlbumA/Track.als"
clavus log "~/Music/AlbumB/Track.als"
clavus cue "~/Music/AlbumA/Track.als" -t "only for A" -p 1.1.1
clavus cues "~/Music/AlbumB/Track.als"
```

**Expected:**

- The two logs show **different** chains, each with its own snapshot.
- The cue added to AlbumA does **not** appear for AlbumB.
- `clavus restore "~/Music/AlbumB/Track.als" <album A's hash>` must be
  **refused** — it must not write Album A's bytes over Album B.

**What to watch for:** If one project's history shows up under the
other, or a restore writes the wrong file, that is a critical bug.
Report it with both paths.

### 9.0b A Shared Bitwig Archive

**Setup:** A `.bwproject` exported from Bitwig Studio. The archive
holds a copy of every sample, but `project.xml` still points at the
original machine's absolute paths.

**Test:**
```bash
clavus audit "/path/to/Bundle.bwproject"
clavus collect "/path/to/Bundle.bwproject"
clavus audit "/path/to/Bundle.bwproject"
```

**Expected:**

- The first audit says the samples are **inside the project archive**,
  not missing.
- `collect` keeps the bundle **self-contained**: the reference inside
  `project.xml` is rewritten to the sample's path *inside* the
  archive, and the sample is not copied out beside the project.
- The second audit is clean.

**What to watch for:** If `collect` copies the samples into a `Samples/`
folder next to the bundle, or if the bundle stops opening in Bitwig,
that is a bug. The bundle has to stay a working Bitwig project.


### 9.1 Unicode in Project Names

**Setup:** A project with Unicode characters in the path or filename.

**Test:**
```bash
clavus audit "/path/to/プロジェクト.als"
clavus snapshot "/path/to/プロジェクト.als" -m "テスト"
```

**Expected:** Works without path encoding errors.

### 9.2 Very Long Paths

**Setup:** A project nested several directories deep.

**Test:**
```bash
clavus audit "/path/with/very/long/nested/directories/project.als"
```

**Expected:** No truncation errors or path length issues.

### 9.3 Large Number of Samples

**Setup:** A project with 100+ samples.

**Test:**
```bash
clavus audit "/path/to/project.als"
clavus collect "/path/to/project.als"
```

**Expected:** Completes without crashing or running out of memory.

### 9.4 Mixed DAW Projects

**Setup:** Have both Ableton (`.als`) and Bitwig (`.bwproject`) projects.

**Test:** Run audit on each.

**Expected:** Both formats handled correctly.

---

## Part 10: File Formats

### 10.1 `.als` File Integrity

**Test:**
```bash
# Verify the .als is gzip-compressed (not plain XML)
file "/path/to/project.als"
# Expected: gzip compressed data

# Confirm it decompresses
gunzip -k -c "/path/to/project.als" | head -1
# Expected: <?xml version="1.0" encoding="UTF-8"?>
```

**What to watch for:** Does Clavus handle both gzip-compressed and plain `.als` files correctly?

### 10.2 `.bwproject` ZIP Structure

**Test:**
```bash
# Verify it's a ZIP
file "/path/to/project.bwproject"
# Expected: Zip archive

# List contents
unzip -l "/path/to/project.bwproject" | head
# Expected: Lists files including project.xml
```

---

## Report Template

When you find a bug, please file it at https://github.com/castle-queenside/clavus/issues/new

**Copy this template:**

```markdown
## Bug: [short description]

### What I was doing
[Steps to reproduce]

### What happened
[What actually occurred]

### What I expected
[What should have happened]

### Environment
- OS: [macOS/Linux/Windows]
- clavus version: [output of `clavus --version`]
- DAW: [Ableton Live version / Bitwig version]
- Project type: [.als / .bwproject]
- Sample count: [approximate]
- Is project on network drive? [yes/no]

### Log output
```
[paste relevant log output with RUST_LOG=debug if possible]
```

### Files (optional)
[If the project is shareable and not confidential]
```

---

## Test Checklist

Mark each as you go:

```
INSTALLATION
[ ] clavus --version works
[ ] clavus --help shows all commands

SAMPLE AUDIT
[ ] audit: clean project passes
[ ] audit: missing samples shows warning
[ ] audit: outside-project samples shows warning
[ ] audit: Bitwig .bwproject works

COLLECT
[ ] collect: copies external samples
[ ] collect: rewrites .als paths
[ ] collect: idempotent (second run is no-op)
[ ] collect: missing files reported clearly

SNAPSHOTS
[ ] snapshot: saves successfully
[ ] snapshot: blocks on missing samples
[ ] log: shows chain with correct hashes
[ ] log: shows timestamps and messages

CUES
[ ] cues: lists existing cues
[ ] cue: adds new cue
[ ] cue --resolve: marks resolved

TUI
[ ] tui: launches and renders
[ ] tui: navigation with arrow keys
[ ] tui: filter cycling with 'f'
[ ] tui: snapshot detail view
[ ] tui: quit with 'q'

RELAY
[ ] relay: starts and listens
[ ] push/pull: works between two machines
[ ] conflict: detected when simultaneous edits

DISCOVERY & STATUS
[ ] projects: finds .als and .bwproject files
[ ] status: shows samples, snapshots, cues
[ ] restore: restores to a specific snapshot
[ ] snapshot --collect: auto-collects before snapshotting
[ ] restore: refuses another project's snapshot
[ ] two projects: separate chains and separate cues

EDGE CASES
[ ] Unicode in project name
[ ] Very long paths
[ ] Large sample count (100+)
[ ] Mixed .als and .bwproject projects

FILE FORMATS
[ ] .als is gzip (not plain XML)
[ ] .bwproject is ZIP with project.xml
[ ] .bwproject with embedded samples: audit does not call them missing
[ ] .bwproject collect: extracts the embedded samples and rewrites the reference
```

---

## Open Questions

If you run into behavior you can't explain, note it here:

1. **What surprised you?** Was anything confusing or unexpected?
2. **What was easiest?** What worked immediately without friction?
3. **What was hardest?** What took the most time or caused the most confusion?
4. **What do you wish it did?** Feature requests, however small.
