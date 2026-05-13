# Postmortem: Steven Collaboration Test — 2026-05-11

**Trigger:** Steven (Windows, first-time Clavus user) joined for a live collaboration test. Fresh install → couldn't pull snapshots. Investigation uncovered 8 bugs across sync, TUI, and CLI.

**Participants:** Chris (macOS), Steven (Windows), Hermes (agent)

---

## Timeline

| Time | Event |
|------|-------|
| Setup | Steven cloned repo, `pip install -e .`, ran `clavus join` — Tailscale connected |
| Attempt 1 | Pull returned no snapshots. No error visible. |
| Attempt 2 | Pull showed spinner then nothing. Silent failure. |
| Debug | Checked relay logs, Tailscale status, project indices — data existed on relay |
| Fix 1 | **MagicDNS not resolving on Windows** — `ipconfig getifaddr` was macOS-only |
| Fix 2 | **`root_als` set to empty string on pull** — couldn't open projects |
| Fix 3 | **Global HEAD ref blocked per-project heads** — pulled wrong snapshot |
| Fix 4 | **ListView crash** — `Label is not a ListItem` from racing `@work` workers |
| Fix 5 | **Silent snapshot save failure** — `write_text()` had no try/except |
| Fix 6 | **TUI freeze during sync** — sync httpx calls blocked event loop |
| Fix 7 | **Cue created in wrong project** — used `projects[0]` instead of active |
| Fix 8 | **`:pull-all` parsing bug** — "pull all" (two words) hit subprocess branch |
| Stepped away | Steven had to leave. Continued hardening without him. |
| Final smoke | Mac↔Win push/pull validated. All fixes merged to `main`. |

---

## Bug #1: MagicDNS Not Resolving on Windows

**Symptom:** `clavus share` on Windows output a raw Tailscale IP (100.x.x.x) instead of MagicDNS hostname. Steven's join worked but relay discovery was fragile.

**Root cause:** `_get_tailscale_url()` in `clavus/web.py` called `ifconfig` / `ipconfig getifaddr` — macOS-only utilities. Windows has neither.

**Fix:** Switched to `tailscale status --json`, which works identically on both platforms. Parses `DNSName` field (e.g. `chrispc.tail46b8d9.ts.net`).

**Commit:** `93e24cb` — `share: use Tailscale MagicDNS hostname instead of raw IP`

**Files changed:** `clavus/web.py`, `clavus/tui.py` (`_tailscale_ip`)

---

## Bug #2: Pulled Projects Couldn't Open

**Symptom:** After `clavus pull`, `clavus open` / `o` key returned "project not found."

**Root cause:** Three interlocking bugs:

1. **`root_als` gate** — `_do_open` checked `self.root_als` which was empty string after pull
2. **`root_als` not set on pull** — `_run_pull_all` and `_do_pull` never populated it
3. **Global HEAD comparison** — `pull_from_remote` compared relay head against a single global `HEAD` ref, so projects B, C, D all matched project A's head and got skipped

**Fix:** Removed `root_als` gate in `_do_open` (computes path from project), set `root_als` during pull, and switched `pull_from_remote` to per-project `proj.head` comparison.

**Commits:** `7c14e19`, `5fb0620`, `4bdfc56`

**Files changed:** `clavus/tui.py`, `clavus/sync.py`

---

## Bug #3: ListView Crash — `Label is not a ListItem`

**Symptom:** TUI crashed with `AssertionError: Label is not a ListItem` during project switching or remote browsing.

**Root cause:** Three `@work(exclusive=False)` workers all mutated the same `ListView` widget tree simultaneously:
- `_run_list_projects` — clears + rebuilds project list
- `_run_list_remotes` — clears + rebuilds remote list  
- `_run_switch_project` — switches cues/history panes

Race: Worker A calls `list_view.clear()` → removes all children. Worker B iterates `list_view.children` to remove its `ListItem` → finds a `Label` leftover from worker C's intermediate state → crash.

**Fix:** Changed all three workers to `@work(exclusive=True)` so only one runs at a time. Added picker guard in `_clear_project_list` — bails if projects/remotes picker is active.

**Commit:** `817f058` (steven-hardening merge)

**Files changed:** `clavus/tui.py`

---

## Bug #4: Silent Snapshot Save Failure

**Symptom:** Pull appeared to succeed but no snapshots landed. No error in TUI, no crash.

**Root cause:** In `pull_snapshot_blobs()`, `meta_path.write_text()` had no `try/except`. If the write failed (disk full, permissions, path issue), the loop continued silently. Invalid hashes (wrong length) were skipped without warning. The head save (`set_index`) wasn't verified after write.

**Fix:** Wrapped `write_text()` in try/except with error logging. Added hash validation before saving. Added verify step after `set_index` to confirm head persisted.

**Commit:** `817f058` (steven-hardening merge)

**Files changed:** `clavus/sync.py`

---

## Bug #5: TUI Freeze During Sync

**Symptom:** TUI locked up completely during `clavus pull` or `clavus push`. No keypresses registered. Appeared hung.

**Root cause:** `_do_pull` and `_do_push` called sync `httpx.Client` directly from async context. The blocking HTTP calls starved the Textual event loop — no messages processed until the network call returned.

**Fix:** Wrapped all blocking I/O in `asyncio.to_thread()`:
- `pull_from_remote()` 
- `pull_snapshot_blobs()`
- `push_to_remote()`
- `push_snapshot_blobs()`
- All `httpx.Client` HTTP calls

**Commit:** `817f058` (steven-hardening merge)

**Files changed:** `clavus/tui.py`

---

## Bug #6: Cue Created in Wrong Project

**Symptom:** `clavus cue "note"` from CLI created the cue in `projects[0]` (first project in index) instead of the active project.

**Root cause:** `add_cue_command` in `clavus/cues.py` hardcoded `projects[0]` instead of reading the active project from `clavus project`.

**Fix:** Read active project from `get_active_project()`.

**Commit:** Committed during session, merged to main

**Files changed:** `clavus/cues.py`

---

## Bug #7: `:pull-all` Parsing Bug

**Symptom:** `:pull-all` ran without error but did nothing on Windows.

**Root cause:** The string `"pull all"` (typed as two words) hit the subprocess command branch in `_do_command()` instead of routing to `_run_pull_all_async`. Was treated as `clavus pull all` which failed silently.

**Fix:** Added explicit handling for `"pull all"` → route to `_run_pull_all`. Stripped "all" from args before passing to push/pull subprocess branch.

**Commit:** `074d457`, `3a2f656`

**Files changed:** `clavus/tui.py`

---

## Peripheral Fixes (Same Session)

| # | Issue | Fix |
|---|-------|-----|
| P1 | Ping timeouts too long (30s) — remote felt dead | `fast_ping()` with 3s timeout |
| P2 | 3 sequential `check_blobs` calls on pull | Single combined call |
| P3 | Welcome screen showed on autoload | Reordered `on_mount` — connect before welcome |
| P4 | `o` key didn't work for pulled projects | Removed `root_als` gate |
| P5 | No `--project` flag on `clavus share` | Added project-scoped relay |
| P6 | Cue inject race condition with `@work` | `exclusive=True` |
| P7 | `:push!` never executed — `async` without `@work` | Added `@work` decorator |

---

## What Went Well

- Tailscale MagicDNS worked flawlessly once configured correctly
- Content-addressed blob sync was solid — no data corruption
- Cross-platform `.als` restore worked (Suite/Intro version mismatch is Ableton's problem, not Clavus's)
- The `@work` audit surfaced the race condition before it hit more users
- `--debug` flag and `errors.log` file made diagnosis possible

---

## What Went Wrong

- **Silent failures everywhere.** Snapshot save, pull, push, command parsing — all failed without user-visible errors. The TUI toast system actively hid errors (auto-clear, `set_timer` broken in workers, `_update_footer` clobber). This is the #1 UX problem.
- **Fresh install was hostile.** Steven hit MagicDNS, root_als, HEAD ref, and ListView crash in the first 10 minutes. A collaborator shouldn't need to debug Clavus to use it.
- **No Windows CI.** Every Windows bug was found manually. PowerShell vs Bash quoting, `ipconfig` vs `ifconfig`, case-insensitive filesystem — all platform-specific.
- **`@work` footguns.** `exclusive=False` default, `set_timer()` broken in workers, `async` without `@work` silently skipped — easy to write code that looks right but doesn't run.

---

## Open Issues

| # | Issue | Priority |
|---|-------|----------|
| 1 | Toast/error system needs overhaul — too many silent failure paths | High |
| 2 | No Windows CI — every bug found manually | High |
| 3 | Fresh-install experience needs hardening (setup wizard, first-pull guardrails) | Medium |
| 4 | `@work` decorator should default to `exclusive=True` or warn on missing decorator | Medium |
| 5 | Steven never got a successful pull — need follow-up session | Medium |
| 6 | `:pull-all` error visibility on Windows (CSS `display:none` still active when worker writes to `#footer-status`) | Low |

---

## Lessons

1. **Every I/O path needs error handling.** File writes, HTTP calls, index updates — no silent failures allowed. If a snapshot doesn't land, the user must know why.
2. **Test fresh installs.** Dogfood the `git clone && pip install -e . && clavus setup` flow on both platforms before every collaborator session.
3. **Workers need guardrails.** `@work` is powerful but dangerous. Consider a linter rule: no `@work` without `explicit=True`, no `async` command handler without `@work`.
4. **Cross-platform smoke test before collab.** 5 minutes of pre-flight on both machines would have caught MagicDNS, root_als, and HEAD ref before Steven joined.
