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
✗ 5 outside project:
  /Users/you/Library/Samples/kick.wav
  …
```

**The report is on stdout.** A script that reads stdout must find the
report there, and stderr must stay empty:

```bash
clavus audit "/path/to/project.als" 1>out.txt 2>err.txt
wc -c out.txt err.txt
```

`out.txt` holds the report; `err.txt` is empty. (Only a project that
could not be *read* writes to stderr.)

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

### 3.2b Collect Must Never Overwrite Your Own Files

**Setup:** a project with a sample in `Samples/` that has the same file
name as a sample somewhere else on disk.

**Test:** run `clavus collect` on it.

**Expected:** the file in `Samples/` is **untouched**. The collected
sample gets a name of its own (`lib_kick.wav`), and the project points at
the new file. Two tracks must never end up sharing one file, and your
own audio must never be overwritten.

**What to watch for:** the bytes of the file that was already in the
project. If they changed, the tool destroyed your audio — report it.

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

✓ 24 sample(s) already inside the project bundle
  Paths in the project archive already point at the copies inside it
```

**Verify:**
1. **No** `Samples/` folder appeared next to the `.bwproject` — the
   bundle *is* the project, and it already carried its samples
2. `unzip -l` still lists `project.xml` and the samples
3. Re-running `clavus audit` says the samples are inside the archive
4. Opening the project in Bitwig still finds its samples

**What to watch for:** The message must not claim that files were
copied into a folder. A bundle that carries its own samples has nothing
to collect.

### 3.5 Collect a Bitwig Project Whose Samples Live Outside the Bundle

**Setup:** A `.bwproject` that points at samples in another folder
(an export from another machine, or a bundle whose samples were moved
out). The paths in the bundle must not exist inside the bundle's own
folder.

**Test:**
```bash
clavus collect "/path/to/Outsider.bwproject"
```

**Expected:**
```
  0 missing  ·  2 outside project  ·  0 inside archive

✓ Collected 2 sample(s) into the project bundle
  Paths in the project archive rewritten — reopen the project in Bitwig
```

**Verify:**
1. `unzip -l` now lists the samples **inside** the archive
2. The references in `project.xml` point at the copies inside the
   archive — not at the old absolute paths, and not at a `Samples/`
   folder next to the bundle
3. **No** `Samples/` folder appeared next to the `.bwproject`
4. `clavus audit` now says the samples are inside the archive
5. Opening the project in Bitwig still plays the audio

**What to watch for:** A bundle that points at a sample it does not
carry opens without its audio. If the audit calls the bundle healthy
while the reference points outside it, the collect is wrong.

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

### 4.4b A Damaged Snapshot History

**Setup:** take a snapshot, then damage the record on disk
(`truncate` the `.json` file in the data directory).

**Test:** run `clavus log` and `clavus status`.

**Expected:** both say the history is damaged. Neither may say "No
snapshots yet", and `status` may not print a snapshot count next to an
empty HEAD.

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

**On a machine with no user name** (a container, a cron job, CI), the
author falls back to the host name, and to "unknown" if even that is
missing. It is never the word "cli".

### 5.2b A Cue With a Bad Position

**Test:**
```bash
clavus cue "/path/to/project.als" -t "typo" -p "4.1"
```

**Expected:** An error. A position must be `bars.beats.sixteenths`, so
`4.1` is not one. A cue stored with a position that can never be
resolved to a time is worse than a refusal.

### 5.2c A Cue With a Track

**Test:**
```bash
clavus cue "/path/to/project.als" -t "drop bass at bar 17" -p 17.1.0 --track "Bass"
clavus cues "/path/to/project.als"
```

**Expected:** The listing shows the track:

```
  fc6c8e8d…  [pending  ]  drop bass at bar 17     @   17.1.0  Bass            chris
```

**What to watch for:** A note that says "drop the bass here" is
useless without the track it belongs to.

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
  - **This one was a real bug, and it was in every set.** The parser
    was reading one track, 120 BPM and no devices out of every real
    Live set, because skipping a device chain swallowed the rest of
    the document and the track parser never ended. If every row in
    your library says 120 BPM, that is the bug talking — the sets are
    not all at 120.
- Projects in **sub-folders** must appear. A project the CLI can see
  but the TUI cannot is a bug.
- A project that **cannot be parsed** must show an error, not vanish.
- Two projects with the **same file name** must be told apart. The
  list shows the folder each one lives in; two rows both called "Song"
  with nothing to tell them apart is a bug.
- The **snapshot count** must match `clavus log`. Take a snapshot in
  another terminal while the TUI is open, move through the list, and
  the count must change.
- A **directory** whose name ends in `.als` or `.bwproject` must not
  be listed as a project. The CLI does not list it; the TUI must not
  either.

### 6.1i Browsing a Library of Sets

**Test:** Put five or six sets in a folder, give them a few snapshots
each, and start the TUI on it.

**Expected:**

- Every row carries the **shape of that project's own history** — a
  row of small bars, oldest on the left, tallest where the work
  clustered. A set you worked on in one afternoon shows one solid burst
  at the right-hand end; a set you keep coming back to shows a spread.
  A row of evenly-spaced identical bars for every project, no matter
  how differently you worked on them, is a bug.
- The **open-note count** shows how many notes are still open on each
  set. A set with two open notes must say so.
- The **age** column shows how long ago the file itself was last
  edited.
- A **strip above the table** names the set under the cursor, with its
  snapshot count, its open notes and its age. The selection must never
  be a guess.
- The **selected row** is visibly marked with a `▌` in the left-hand
  column, and the text does not move sideways when the cursor moves
  onto it.

**Test:** press `/` and type part of a set's name.

**Expected:** The list narrows as you type. The bottom-left corner
shows what you have typed. The row that matches is still selected. A
search that matches nothing says so plainly rather than showing an
empty list.

**Test:** press `Esc` while the filter is open.

**Expected:** The filter is cleared and the whole list comes back. The
text you typed is thrown away, not kept for next time.

**Test:** press `S` a few times.

**Expected:** The order cycles, and the bottom-left corner **names the
order in force** — "recent", "name", "history", "open notes". Sorting
by history puts the set with the most snapshots first; sorting by open
notes puts the set with the most open notes first.

**Test:** type a long message — open a project, add a note with a long
text, and watch the bottom bar.

**Expected:** The message is truncated. The key hints on the right and
the relay's state on the far right **do not move**. A message that
pushes the rest of the bar sideways is a bug.

**Test:** make the terminal narrow, about 70 columns, and then narrower.

**Expected:** Optional columns drop away one at a time. The **history
shape is never squeezed** — it is the column worth keeping. Below the
minimum size the TUI says so instead of drawing nonsense.

**Test:** press `S` and `P` and `G` on the sync screen.

**Expected:** They work. These are capital letters, and a terminal
reports a capital letter with the shift modifier held; if the TUI
matches on the unmodified letter only, none of them will do anything.

### 6.1j The Mouse

Clavus is keyboard-first and the keyboard still does everything. The
mouse is a second way in, and nothing is reachable by mouse that is
not reachable by keyboard.

**Test:** start the TUI with a good number of sets and use the mouse
only.

**Expected:**

- The **wheel** moves the selection, and the list scrolls so that the
  selected row stays on the screen. A long list must scroll — if the
  cursor walks off the bottom and the list does not move, that is a
  bug.
- A **single click** on a row selects it. The row must be visibly
  marked, and the strip above the table must name it.
- A **double click** on a row opens that set, exactly as `Enter` does.
- A **click on the scrollbar**, on the right-hand edge, jumps the
  window to that part of the list. The selection goes with it — the
  cursor must never be left on a row that is not on the screen.
- **Dragging** with the button held moves the selection to the row
  under the pointer.
- Clicking on the **title bar or the footer** must do nothing. It must
  not select a row.

**Test:** use the mouse and the keyboard together, and check that
neither one breaks the other.

**Expected:** The arrow keys still work, `S` still sorts, `/` still
filters, `q` still quits.

**Test:** start the TUI, quit it, and then type in the shell.

**Expected:** The shell behaves normally. A terminal left reporting
mouse events prints escape sequences into the prompt, and the shell
looks broken — that is a bug, and it is the one to watch for here.
Report it with the exact sequence your terminal was left in.

**Test:** make the list long enough to scroll, and look at the
scrollbar.

**Expected:** It only appears when there is something to scroll. It
shows roughly where in the list the window is.

### 6.1c Leaving the TUI

**Test:** Start the TUI and press **Ctrl-C**.

**Expected:** The TUI leaves the alternate screen and the shell comes
back. The same happens with `q` from the project list.

**What to watch for:** The TUI is in raw mode, so Ctrl-C arrives as an
ordinary key. If the app carries on, the interrupt was swallowed and
the program looks hung. Report it.

### 6.1d A Window That Is Too Small

**Test:** Start the TUI and shrink the terminal to a few rows and
columns — or resize it while the TUI is running.

**Expected:** The TUI says the window is too small and what size it
needs. It must not die with a bare exit code and no message, and it
must not leave the terminal in the alternate screen.

**What to watch for:** Anything the TUI prints on the way out has to
be readable. A panic that says nothing is a bug.

### 6.1e A Relay That Goes Away

**Test:** Connect the TUI to a relay (`s`, then `c`), then kill the
relay process and wait a few seconds.

**Expected:** The TUI says the connection was lost. A screen that
still shows "connected" — with the filled circle and the relay
address — is a bug: the user would believe their work was synced when
it was not.

### 6.1f Pushing Notes Without a Snapshot

**Test:** Make a note on a project that has no snapshot, open the sync
screen and press `P`.

**Expected:** The notes are sent. The CLI does this too, and a user
who only writes notes must be able to share them.

### 6.1g Typing a Relay Address

**Test:** On the sync screen press `r` and type an address.

**Expected:** The field starts **empty**, so what you type is the
address. If the old address is still in the field, what you type is
appended to it and the connection fails with a nonsense address.

### 6.1h Esc From the Sync Screen

**Test:** Open the sync screen from the project list, press `Esc`.

**Expected:** You go back to the project list. Open it from a project
instead, and `Esc` takes you back to that project. The back step must
return you to the screen you came from.

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
  "✓ up to date". The same goes for a relay that answers with something
  that is not the project's history: nothing was checked, so the pull
  must fail and say so.
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
clavus projects --no-recursive
```

**Expected:** A list of project names, last-modified times, and full paths.

**What to watch for:** The scan looks into sub-folders by default — the
same way the TUI does. Your sets are usually in
`~/Music/Archive/2013`, not in `~/Music` itself, and a command that
found none of them was looking in the wrong place. `--no-recursive`
scans only the top level.

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

### 9.0c Two Projects in Folders of the Same Name

**Setup:** Two different projects whose *parent folders* share a name —
the case a folder name alone cannot tell apart:

```
~/Music/notes/p1/studio/Song.als
~/Music/notes/p2/studio/Song.als
```

**Test:**
```bash
clavus snapshot "~/Music/notes/p1/studio/Song.als" -m "P1 song"
clavus snapshot "~/Music/notes/p2/studio/Song.als" -m "P2 song"
clavus log "~/Music/notes/p1/studio/Song.als"
clavus log "~/Music/notes/p2/studio/Song.als"
clavus cue "~/Music/notes/p1/studio/Song.als" -t "only for P1" -p 1.1.1
clavus cues "~/Music/notes/p2/studio/Song.als"
```

**Expected:**

- Each log shows **its own** snapshot and its own message. A snapshot
  of P2 must not be reported as "already in this project's history".
- The note added to P1 does **not** appear under P2.
- `ls ~/.clavus/blobs/projects` shows **two** entries for them, not one.

**What to watch for:** This is the same defect as 9.0, one level
deeper: a folder's *name* is not a folder's *path*. Report it with
both paths.

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


### 9.0d A Folder Is Not a Project

**Test:**
```bash
clavus log /path/to/my/projects
clavus restore /path/to/my/projects
```

**Expected:** An error that says the path is a directory, not a project
file. It must **not** say "no snapshots yet" — that would be a report
about a project that was never there.

### 9.0e Two Projects Outside the Home Directory

**Setup:** The same case as 9.0c, but on a disk outside the home
directory — an external drive, a network share:

```
/Volumes/Studio/external/p1/studio/Song.als
/Volumes/Studio/external/p2/studio/Song.als
```

**Test:**
```bash
clavus snapshot "/Volumes/Studio/external/p1/studio/Song.als" -m "P1 song"
clavus snapshot "/Volumes/Studio/external/p2/studio/Song.als" -m "P2 song"
clavus log "/Volumes/Studio/external/p1/studio/Song.als"
clavus log "/Volumes/Studio/external/p2/studio/Song.als"
```

**Expected:** Each log shows **its own** snapshot and its own message.
The two projects are two projects.

**What to watch for:** This case used to be broken even after 9.0c was
fixed, because a project outside the home kept only the *name* of its
folder — both of them were "studio" — and they shared one identity.

### 9.0f An Unreadable Data Directory

**Test:**
```bash
chmod 000 ~/.clavus
clavus log "/path/to/project.als"
chmod 755 ~/.clavus
```

**Expected:** An error that says the Clavus data directory cannot be
read, and a non-zero exit code.

**What to watch for:** "No snapshots yet" here is a lie: the snapshots
are there, and the directory is simply closed. A tool that reports an
empty history when it cannot read the history is worse than one that
fails.

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
[ ] audit: the report is on stdout (stderr stays empty)
[ ] audit: Bitwig .bwproject works

COLLECT
[ ] collect: copies external samples
[ ] collect: rewrites .als paths
[ ] collect: idempotent (second run is no-op)
[ ] collect: missing files reported clearly
[ ] collect: a .bwproject stays self-contained (samples inside the zip)
[ ] collect: no Samples/ folder is created next to a bundle
[ ] collect: a symlinked project is still a symlink afterwards

SNAPSHOTS
[ ] snapshot: saves successfully
[ ] snapshot: blocks on missing samples
[ ] log: shows chain with correct hashes
[ ] log: shows timestamps and messages

CUES
[ ] cues: lists existing cues
[ ] cues: shows the track of a cue
[ ] cue: adds new cue
[ ] cue --resolve: marks resolved

TUI
[ ] tui: launches and renders
[ ] tui: navigation with arrow keys
[ ] tui: the selected cue and snapshot are visibly highlighted
[ ] tui: filter cycling with 'f'
[ ] tui: snapshot detail view
[ ] tui: quit with 'q'
[ ] tui: quit with Ctrl-C
[ ] tui: a window that is too small says so, and does not die silently
[ ] tui: the project list shows the folder, so two same-named projects differ
[ ] tui: the project list refreshes its counts after an external snapshot
[ ] tui: notes can be pushed without a snapshot
[ ] tui: the relay field starts empty
[ ] tui: Esc from the sync screen goes back where you came from
[ ] tui: a dead relay is noticed, not shown as connected

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
[ ] two projects in same-named folders: separate chains and separate cues
[ ] two same-named projects outside the home: separate chains and separate cues
[ ] a folder of projects is refused, not reported as an empty project
[ ] an unreadable data directory is an error, not an empty history

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

## Part 11: A Real Library of Sets

The workflow above uses small, tidy fixtures. This part is for the
real thing: a drive of hundreds of sets, made over years, on several
machines.

### 11.1 Pointing Clavus at a drive

```bash
clavus --dir /Volumes/YourDrive projects
clavus --dir /Volumes/YourDrive tui
```

**Expected:** The list of sets on the drive. A set that Live keeps a
`Backup/` or `Autosave/` copy of is **not** listed twice — those copies
are not projects of their own.

**What to watch for:** A drive of a few hundred sets usually holds more
copies than sets. On one drive, 2,690 `.als` files were 1,245 sets and
1,445 of Live's own copies.

### 11.2 A set made on another machine

```bash
clavus audit "/Volumes/YourDrive/Some Set/Some Set Project/Some Set.als"
```

**Expected:** The report names the samples it cannot find. A set that
was made on a machine with a different user name, or with a drive
mounted under a different name, will point at paths that do not exist
here — that is the truth, and the report should say so.

**What to watch for:** A set whose samples live in the machine's Ableton
*user folder* rather than beside the set. The path starts with an empty
element, and the sample should be found in the user folder — and
reported as **outside the project**, so `clavus collect` can bring it
in. If it is reported as *missing* when the file is sitting in
`~/Music`, that is a bug.

### 11.3 A set that uses Live's own sounds

```bash
clavus audit "/Volumes/YourDrive/Some Set/Some Set Project/Some Set.als"
```

**Expected:** A drum from `Ableton Live 11 Suite.app` is named as
coming from Live's own library. It is **not** reported as missing, and
it does **not** stop the snapshot — it comes back whenever Live is
installed.

### 11.4 Taking the whole library

```bash
clavus --dir /Volumes/YourDrive status "/Volumes/YourDrive/Some Set/Some Set Project/Some Set.als"
```

**What to watch for:** Clavus does not copy your sets anywhere. The set
stays where it is; only a snapshot's bytes are kept, in the data
directory, and only for the sets you actually snapshot. Check the size
of the data directory afterwards:

```bash
du -sh ~/.clavus
```

---

## Part 12: Coming from the Older Clavus

If you used Clavus before this version, your library was in
`~/Clavus/Projects` and your history was in `~/.clavus`. Both are still
readable, and the history can be brought across rather than left
behind.

### 12.1 Seeing what would come across

```bash
clavus import-legacy --store ~/.clavus --projects ~/Clavus/Projects --dry-run
```

**Expected:** A list of the projects that would be brought across,
with how many snapshots and how many notes each one has. Anything that
could not be found is named, with the reason. Nothing is written.

### 12.2 Bringing it across

```bash
clavus import-legacy --store ~/.clavus --projects ~/Clavus/Projects
```

**Expected:** The same list, and then the history is there:

```bash
clavus log "My Set.als"
clavus cues "My Set.als"
```

**What to watch for:** The notes are the part that cannot be
recreated. Check that every note you remember is there, with its text
and its author. The old version wrote a note's status in lower case
(`pending`); the current one writes `Pending`. Both are read.

### 12.3 Where the sets are found

If the library has moved since the old version, say where it is now
with `--projects`. The old store names each project by its old path,
and the part after `Clavus/Projects` is still true of the library, so
the right file is found even when two files share a name.

**What to watch for:** A project listed under *Left behind* means its
file was not found. The reason says so; nothing was dropped in
silence.

### 12.4 The bytes

A snapshot's project bytes and audio are brought across too, so a
restore works:

```bash
clavus restore "My Set.als" HEAD
```

**What to watch for:** The imported files are ordinary files you own.
They can be read, replaced and deleted. A backup's files are often
read-only, and that must not follow the bytes into your store.

---

## Open Questions

If you run into behavior you can't explain, note it here:

1. **What surprised you?** Was anything confusing or unexpected?
2. **What was easiest?** What worked immediately without friction?
3. **What was hardest?** What took the most time or caused the most confusion?
4. **What do you wish it did?** Feature requests, however small.
