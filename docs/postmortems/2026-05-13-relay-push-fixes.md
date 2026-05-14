# Postmortem — May 13, 2026 (Afternoon)

## Session Summary

Debugged and fixed relay push failures. Two bugs found in the relay's push-snapshots
endpoint, plus stale `last_remote_head` data on the Mac side.

## Bugs Found & Fixed

### Bug 1: `force=True` ignored by conflict check (web.py)

**The problem:** The `SyncPushSnapshotsBody` model accepted a `force` field, but
`_sync_push_snapshots_impl()` never checked it. The optimistic lock (conflict
detection) ran unconditionally whenever `expected_parent` was set, even when
`force=True` was passed.

**Impact:** Force push was silently ignored. Any push with a stale `expected_parent`
would always return 409 Conflict, no matter what. Users who pulled changes from
another machine couldn't push back without manual relay HEAD manipulation.

**Fix:** Added `not body.force and` guard before the conflict check:
```python
if not body.force and body.expected_parent is not None:
```

Commit: (perf-improvements, uncommitted)

### Bug 2: `max()` on empty list crash (web.py)

**The problem:** When `force=True` was sent with an empty `snapshots` list
(e.g. to just sync HEAD), the HEAD update code called `max(body.snapshots, ...)`
which raised `ValueError: max() arg is an empty sequence`.

**Impact:** Force push with empty snapshots returned HTTP 500 Internal Server Error.
The TUI would show "❌ push failed: Failed to push snapshots" because 500 is neither
200 nor 409.

**Fix:** Added a guard to use `body.expected_parent` as the new HEAD when force=True
with no snapshots, or skip HEAD update when there's nothing to update.

Commit: (perf-improvements, uncommitted)

### Stale `last_remote_head` (data issue, not code)

**The problem:** The Mac's `proj.last_remote_head` was `1101a83a6722...` but the
relay HEAD was `23e0bc03b8b0`. This mismatch triggered conflict detection on
every push. The stale `last_remote_head` was set from a previous push before
the pull-based `last_remote_head` update was implemented.

**Note:** `pull_from_remote()` already updates `last_remote_head` correctly
(lines 981-983 of sync.py). This was just old stale data.

**Fix:** Manually synced `last_remote_head` to match relay HEAD.

## Active State

- **Relay:** Running on Mac, port 7891, proxied via Tailscale to port 7890
- **Tailscale URL:** `http://slow-hands-studio-1.tail46b8d9.ts.net:7890`
- **Direct URL:** `http://localhost:7891`
- **Push verified:** Works from both local and Tailscale-proxied endpoints
- **Force push:** Works correctly (bypasses conflict detection)
- **Windows:** Last seen 4h ago; needs to re-pull code to get relay fixes

## Known Gaps

1. **Windows still has old code** — the push from Windows TUI would hit the
   same bugs because the relay is now fixed (force=True works) but Windows
   doesn't know to use `force=True`. After pulling latest code, push should
   work normally.
2. **`last_remote_head` gets truncated** — the relay's HEAD is stored as a
   12-char truncated hash in `~/.clavus/refs/HEAD`. When the client computes
   `expected_parent`, it should match this length to avoid false conflicts.
   Currently the client sends whatever it has (could be 12 or 64 chars),
   and the relay compares against its stored HEAD which is 12 chars.
   This works because the comparison is string equality — both sides need
   the same format.
3. ~~TUI inject: @work(exclusive=False) caused UnicodeDecodeError on Windows~~
   Fixed in adf19af, 2a8b0ed, 20f5fd7 — all pipe decode() calls now use
   `'utf-8'` with `errors='replace'`; async subprocess env includes
   `PYTHONIOENCODING=utf-8`; inject auto-snapshots work.

## How to Verify

```bash
# On Mac
python3 -m clavus push slow-hands-studio-1

# Force push (bypass conflict check)
# For TUI: select remote, then use force option
```

From Windows:
```
# After pulling latest code
py -m clavus pull                 # sync down
py -m clavus push <remote-name>   # push back
# Or in TUI: p (pull), P (push)
```
