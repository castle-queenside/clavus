# :pull-all Bug — Full Chronology

**Date:** May 11, 2026  
**Symptom:** `:pull-all` on Windows TUI flashes an error that's invisible (< 1 second), leaving no way to diagnose why it fails. Works correctly when called from a macOS Python script outside the TUI.

---

## Root Cause Stack

Three independent bugs found + one unresolved:

### Bug 1: Coroutine never scheduled (FIXED)
**Commit:** `c2e916c` → `02e2180` → reverted in `c2e916c`  
`_run_pull_all()` is `@work` decorated. The `@work` decorator handles scheduling when called from sync context. My initial "fix" of wrapping in `asyncio.create_task()` was wrong — it double-wrapped and broke the `@work` mechanism. Reverted to plain `self._run_pull_all()` call. Matches working `_run_list_projects` pattern.

### Bug 2: `@work(exclusive=False)` needed (FIXED)
**Commit:** `c2e916c`  
Original was `@work` (defaults to `exclusive=True`). Changed to `@work(exclusive=False)` to match `_run_list_projects`.

### Bug 3: `action_force_push` never executed (FIXED)
**Commit:** `b2240a0`  
`:push!` had the same bug — `async` without `@work`, called from sync `_do_command`. Coroutine created, never scheduled. Added `@work` decorator.

### Bug 4: Cross-project push conflicts (FIXED)
**Commit:** `caa97e9`  
`last_head` stored per-remote (one global value). Switching projects caused `expected_parent` to be wrong project's head → 409 Conflict on every push. Fixed: `ClavusProject.last_remote_head` per-project tracking.

### Bug 5: Error display invisible on Windows (UNRESOLVED)
The error IS being generated and `_show_sticky()` IS being called. The text IS being written to the `#footer-status` Static widget. But it's not visible to the user.

---

## Attempted Fixes for Error Display (all failed on Windows)

### Attempt 1: 30-second timer
`_show_sticky` called `_footer_toast(msg, 30.0)` → `set_timer(30.0, ...)`.
- **Why it failed:** `set_timer()` is unreliable inside `@work` workers (known Textual bug, documented in memory). Timer never fires or fires immediately.

### Attempt 2: Sentinels (`_toast_timer = object()`)
`_show_sticky` set `_toast_timer` to a dummy `object()` that blocks `_update_footer()` from overwriting.
- **Why it failed:** `_update_footer` is called from `_render()`, which is a CUSTOM method — NOT a Textual lifecycle hook. `App.refresh()` doesn't know about `_render()` and never calls it. So the sentinel blocks nothing useful.

### Attempt 3: `_sticky_error` attribute
`_show_sticky` sets `self._sticky_error = msg`. `_update_footer()` checks it and displays.
- **Why it failed:** Same as #2 — `_update_footer` is only called explicitly, never by Textual's refresh cycle. The string sat in memory unread.

### Attempt 4: Direct widget write + refresh
`_show_sticky` calls `status.update(msg)` + `status.refresh()` directly on the `#footer-status` Static widget.
- **Why it failed:** CSS timing. `on_input_submitted` → `_hide_input()` calls `remove_class("input-mode")`, which hides `#footer-status` via `display: none`. This CSS change takes effect on the NEXT refresh. But the `@work` worker runs in the same event loop iteration, BEFORE the CSS is applied. The widget is still `display: none`. Writing to an invisible widget is a no-op visually.

### Attempt 5: Toast guard reordering
Moved `_sticky_error` check before the toast timer guard in `_update_footer`.
- **Why it failed:** `_update_footer` still isn't called by Textual's refresh cycle. The ordering doesn't matter if the function never runs.

### Attempt 6: Force `self.refresh()` before widget write
`_show_sticky` calls `self.refresh()` first (to apply CSS), then writes to widget.
- **Commit:** `89dd752`
- **Why it failed:** Even with the forced refresh, the CSS application and widget render happen asynchronously on Windows. The write still lands before the widget is visually ready, or the subsequent `finally: self.refresh()` in `_run_pull_all` triggers a re-render that clears/overwrites the content.

---

## Execution Flow (Windows timing)

```
1. User types :pull-all, presses Enter
2. on_input_submitted fires
3.   _hide_input() → remove_class("input-mode") [CSS scheduled, not applied]
4.   _hide_input() → call_after_refresh(_update_footer) [callback scheduled]
5.   _do_command("pull-all")
6.     _run_pull_all() → @work schedules coroutine, returns immediately
7. on_input_submitted returns
8. --- Event loop iteration ---
9. [Windows: CSS not yet applied, #footer-status is display:none]
10. _run_pull_all worker executes:
11.   _status("⬇ pull-all: starting...") → writes to hidden widget
12.   _status("⬇ probing relay...") → writes to hidden widget
13.   SyncClient fails → _show_sticky("❌ cannot reach...")
14.     self.refresh() [tries to apply CSS — may not complete synchronously]
15.     status.update(error_msg) [writes to possibly-still-hidden widget]
16.   return from function
17.   finally: self.refresh() [applies CSS — widget becomes visible]
18.     _render() [custom method, NOT called by Textual — does nothing]
19. --- call_after_refresh callback fires ---
20. _update_footer() runs — sees _sticky_error, displays it
21. But toast timer from step 11 is still active → toast guard blocks
22. Fixed: sticky check moved before toast guard
23. But widget content may already be overwritten by step 17's render
```

---

## Key Technical Details

- **`_render()` is NOT a Textual lifecycle method.** It's a custom method called explicitly by our code. `App.refresh()` and `Screen.refresh()` do NOT invoke it.
- **`#footer.input-mode #footer-status { display: none; }`** — This CSS rule hides the footer status when the command input is active. The `remove_class` takes effect on the next refresh cycle, not synchronously.
- **`set_timer()` fails in `@work` workers** — documented in memory. Timers may not fire or may fire incorrectly in worker context.
- **`call_after_refresh()` schedules ONE callback** after the next refresh. Used by `_hide_input` to restore the footer, creating a race with worker output.
- **The function WORKS on macOS** when called from a plain Python script outside the TUI. All 10 projects pull correctly. The bug is purely about error DISPLAY in the TUI on Windows.

---

## What Would Actually Fix This

1. **Modal screen** — push a `Screen` with the error instead of writing to footer. Most reliable.
2. **Log to file** — write error to `~/.clavus/pull-all.log` and tell user to check it.
3. **Dedicated error ListView** — add a persistent error pane that doesn't share CSS with the footer.
4. **Skip the footer entirely** — use `self.notify()` or `self.bell()` or `print()` to stderr.
5. **Sync the function** — remove `@work`, make `_run_pull_all` a sync function that runs the HTTP calls in a thread. Would eliminate the event loop timing issue.

---

## Files Modified This Session

```
clavus/tui.py      — _run_pull_all, _show_sticky, _update_footer, _footer_toast,
                     _do_command (pull-all dispatch), action_force_push, __init__
clavus/sync.py     — push_to_remote (last_remote_head), pull_from_remote
clavus/store.py    — ClavusProject.last_remote_head field
clavus/web.py      — (relay restarted with latest code, no changes needed)
TESTING.md         — matrix updated
```

## Commits This Session

```
6951815 docs: update testing matrix — May 11 evening session
89dd752 fix: force self.refresh() in _show_sticky before widget write
102ce8e fix: _show_sticky writes DIRECTLY to footer widget + reorder priority
ace9bc7 fix: _show_sticky bypasses toast system entirely — write to _sticky_error
1cda82f fix: _show_sticky now uses sentinel, not set_timer (broken in @work workers)
b2240a0 fix: :push! never ran (missing @work) + sticky pull-all errors
c2e916c fix: revert asyncio.create_task on _run_pull_all (already @work), add debug
bd15b96 fix: replace :pull-all sentinel with real timer + safety net for stuck toasts
caa97e9 fix: per-project last_remote_head — stop cross-project push conflicts
02e2180 fix: :pull-all never ran — coroutine not scheduled (missing asyncio.create_task)
```
