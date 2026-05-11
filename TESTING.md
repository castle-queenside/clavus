# Testing — Clavus

Living test matrix. Mark ✅ (pass), ❌ (fail), ⚠️ (flake), 🔲 (untested). Add date + notes.

## Collaboration (two-machine)

| # | Test | Mac↔Win | Win↔Win | Mac↔Mac | Notes |
|---|------|:---:|:---:|:---:|-------|
| C1 | Push snapshots → peer pulls | 🔲 | 🔲 | 🔲 | |
| C2 | Peer pushes → you pull | 🔲 | 🔲 | 🔲 | |
| C3 | Both edit same cue → push/pull → ⚠ appears | 🔲 | 🔲 | 🔲 | |
| C4 | Resolve cue conflict with `!` → push → peer pulls resolved | 🔲 | 🔲 | 🔲 | |
| C5 | Both edit same snapshot message → ⚠ appears on snap | 🔲 | 🔲 | 🔲 | New feature — never live-tested |
| C6 | Resolve snapshot message conflict with `!` | 🔲 | 🔲 | 🔲 | New feature |
| C7 | Peer pushes snap → you `o` to open in Ableton | 🔲 | 🔲 | 🔲 | Cross-machine open |
| C8 | Peer pushes snap → you `T` restore → .als lands correctly | 🔲 | 🔲 | 🔲 | |
| C9 | Stem import → push → peer pulls → WAV appears | 🔲 | 🔲 | 🔲 | |
| C10 | Stem push/pull dedup (same WAV doesn't transfer twice) | 🔲 | 🔲 | 🔲 | |
| C11 | Push → peer edits → peer pushes → you pull (roundtrip) | 🔲 | 🔲 | 🔲 | Full cycle |
| C12 | Rapid push/edit/push (optimistic locking: 409 rejection) | 🔲 | 🔲 | 🔲 | |
| C13 | Network drop mid-push → retry → clean state | 🔲 | 🔲 | 🔲 | |
| C14 | Relay restart while clients connected → clients recover | 🔲 | 🔲 | 🔲 | |
| C15 | Cross-account Tailscale (shared node, MagicDNS) | 🔲 | 🔲 | 🔲 | Steven session partially validated |

## TUI

| # | Test | macOS | Windows | Notes |
|---|------|:---:|:---:|-------|
| T1 | `c` new cue → appears in list → Ableton marker injection | ✅ 5/10 | 🔲 | |
| T2 | `C` snapshot → history updates | ✅ 5/10 | 🔲 | `C` binding removed — use `S` |
| T3 | `S` snapshot + auto-push | ✅ 5/10 | 🔲 | |
| T4 | `e` edit cue text → persists | ✅ 5/10 | 🔲 | |
| T5 | `e` edit snapshot message → persists after reload | ✅ 5/10 | 🔲 | |
| T6 | `!` resolve cue conflict (ConflictScreen modal) | 🔲 | 🔲 | |
| T7 | `!` resolve snapshot message conflict (SnapConflictScreen) | 🔲 | 🔲 | |
| T8 | `o` open HEAD in Ableton | ✅ 5/10 | 🔲 | Fixed missing Path import |
| T9 | `o` from history pane → open selected snapshot | ✅ 5/10 | 🔲 | |
| T10 | `T` restore to snapshot | ✅ 5/10 | 🔲 | Destructive edits reverted! |
| T11 | `d` diff selected snapshot | ✅ 5/10 | 🔲 | Fixed 10-char hash → full_hash |
| T12 | `p` pull → auto-snapshot → history updates | ✅ 5/10 | 🔲 | No data to pull, handled gracefully |
| T13 | `P` push → relay receives | ✅ 5/10 | 🔲 | Sync shown in header |
| T14 | `Tab` switch cues ↔ history pane | ✅ 5/10 | 🔲 | |
| T15 | `j`/`k` navigation, scrolling | ✅ 5/10 | 🔲 | |
| T16 | `?` help screen (all bindings visible) | ✅ 5/10 | 🔲 | Scrollable, all bindings |
| T17 | `:` command mode → `:snapshot msg`, `:pull`, `:push` | ✅ 5/10 | 🔲 | |
| T18 | `:project <name>` switch projects → cues/history reload | ✅ 5/10 | 🔲 | |
| T19 | `:init <path>` from TUI → project loads | ✅ 5/10 | 🔲 | |
| T20 | `:browse` navigation → `:init` from browser | 🔲 | 🔲 | |
| T21 | Header dot: green ● (connected), yellow ○ (remote, no data), dim ○ (no remote) | ✅ 5/10 | 🔲 | |
| T22 | Freeze detection warning on `S` | ✅ 5/10 | 🔲 | Soft warning added: ⚠️ N frozen tracks |
| T23 | Long cue text / snapshot message → no truncation crash | ✅ 5/10 | 🔲 | Graceful cutoff |
| T24 | TUI survives corrupt meta file (orphaned 10-char hash) | 🔲 | 🔲 | |

## CLI

| # | Test | macOS | Windows | Notes |
|---|------|:---:|:---:|-------|
| L1 | `clavus setup` wizard | 🔲 | 🔲 | |
| L2 | `clavus init <path>` → project created | ✅ 5/10 | 🔲 | |
| L3 | `clavus tui` opens dashboard | ✅ 5/10 | 🔲 | ⧩ logo restored |
| L4 | `clavus share` starts relay | 🔲 | 🔲 | |
| L5 | `clavus join <url>` adds remote + pulls | 🔲 | 🔲 | |
| L6 | `clavus find` discovers peers | ✅ 5/11 | 🔲 | No servers (expected), clean message |
| L7 | `clavus remote add/list/remove` | ✅ 5/11 | 🔲 | List/add works |
| L8 | `clavus push` / `clavus pull` | ✅ 5/11 | 🔲 | Cannot reach (expected), clean error |
| L9 | `clavus snapshot "msg"` | 🔲 | 🔲 | |
| L10 | `clavus backup` → `clavus backups` → `clavus restore-store` | 🔲 | 🔲 | |
| L11 | `clavus repair` fixes corrupted index | ✅ 5/11 | 🔲 | Healthy — 2 projects |
| L12 | `clavus doctor` health check | ✅ 5/11 | 🔲 | 2 projects, 73 blobs, healthy |
| L13 | `clavus stem import/push/pull/list` | 🔲 | 🔲 | |
| L14 | `clavus open` launches Ableton with HEAD | ✅ 5/11 | 🔲 | 13 tracks, 23 samples, launched |
| L15 | `clavus restore <hash>` restores snapshot | 🔲 | 🔲 | |

## Edge Cases & Error Handling

| # | Test | macOS | Windows | Notes |
|---|------|:---:|:---:|-------|
| E1 | Empty project (no snapshots) → snapshot → push | 🔲 | 🔲 | |
| E2 | Empty relay → join → "No projects found" message is clear | 🔲 | 🔲 | |
| E3 | Pull with no remotes configured → clear error | ✅ 5/11 | 🔲 | ✗ in header + footer error |
| E4 | Push with no remotes → clear error | ✅ 5/11 | 🔲 | ✗ in header + footer error |
| E5 | Corrupted .als file → snapshot fails gracefully | 🔲 | 🔲 | |
| E6 | Missing Ableton → `clavus open` fails gracefully | 🔲 | 🔲 | |
| E7 | Very large .als (200+ tracks, 10MB+) → snapshot performance | 🔲 | 🔲 | |
| E8 | Project with non-ASCII characters in name/path | 🔲 | 🔲 | |
| E9 | Multiple remotes → push to all, pull from all | 🔲 | 🔲 | |
| E10 | `clavus share` port conflict → clear error | 🔲 | 🔲 | |

## Platform-Specific

| # | Test | Status | Notes |
|---|------|:---:|-------|
| P1 | Windows: TUI renders correctly (Windows Terminal) | 🔲 | |
| P2 | Windows: `os.startfile()` opens .als in Ableton | 🔲 | |
| P3 | Windows: OneDrive Files On-Demand → .als accessible | 🔲 | |
| P4 | Windows: `py -m pip install -e .` works from fresh clone | 🔲 | |
| P5 | macOS: `open` command launches Ableton | ✅ 5/10 | |
| P6 | macOS: `tailscale serve` survives sleep/wake | 🔲 | |
| P7 | Linux: install + `clavus tui` runs (no DAW needed) | 🔲 | |
| P8 | Cross-platform: Mac snapshot → Windows restore → opens in Ableton | 🔲 | |
| P9 | Cross-platform: Windows snapshot → Mac restore → opens in Ableton | 🔲 | |

## Test Sessions

| Date | Who | Platform | Tests run | Results |
|------|-----|----------|-----------|---------|
| 5/10/26 | Chris + Hermes | macOS | T1-T5, T8-T19, T21-T23, L2-L3, L12, P5 | 25+ ✅. Bugs fixed: assign/unassign fingerprint, missing Path import, 10-char hash → full_hash in diff, 📸→● emoji purge, ⧩ logo restored, inject→auto-snapshot, freeze soft warning, archive/delete prefill. Spinner animation fixed. |

---

**Legend:** ✅ Pass  ❌ Fail  ⚠️ Flake / intermittent  🔲 Untested

**How to use:** Before a release, run through 🔲 items. Mark ✅/❌ with date. File bugs for ❌. After each test session, add a row to Test Sessions.
