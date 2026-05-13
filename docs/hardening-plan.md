# Clavus Hardening — Lessons from May 11, 2026

## Five hours, seven commits, one parsing bug.

Every failure mode had a common thread: **silent failure with no visible error.** The bugs weren't hard to fix — they were hard to *find* because nothing surfaced them.

---

## 1. Command Dispatch: Kill the if/elif Chain

**Problem:** `_do_command()` is a 50-line if/elif chain. `:pull all` matched `"pull" in ("pull", "push")` and fell into subprocess code that crashed on an unimported `time` — silently, because the worker was never reached.

**Fix:**
```python
# Replace if/elif with a dispatch table
COMMANDS = {
    "project":  ("switch", True),   # needs arg
    "projects": ("list_projects", False),
    "pull":     ("pull", False),
    "push":     ("push", False),
    "push!":    ("force_push", False),
    "pull-all": ("pull_all", False),
    "init":     ("init_project", True),
    ...
}

# Exact-match first, then fallback
def _do_command(self, text: str):
    parts = text.strip().split(maxsplit=1)
    cmd = parts[0].lower()
    arg = parts[1] if len(parts) > 1 else ""
    
    handler = COMMANDS.get(cmd)
    if handler:
        method_name, needs_arg = handler
        if needs_arg and not arg:
            self._status(f":{cmd} requires an argument")
            return
        getattr(self, f"_run_{method_name}")(arg) if needs_arg else getattr(self, f"_run_{method_name}")()
        return
    
    # Fallback: multi-word aliases
    # ...
```

**Why:** Exact match first. No ambiguous substring matching. `:pull all` won't match `"pull"` — it doesn't exist as a single key.

---

## 2. @work Decorator: Audit Every Async Handler

**Problem:** `action_force_push` was `async` without `@work`. Called from sync `_do_command`, the coroutine was created, never awaited, silently dropped. User sees "nothing happens."

**Fix:** Add a lint rule or pre-commit check:
```bash
# Find async defs in _do_command dispatch without @work
grep -B1 "async def" clavus/tui.py | grep -v "@work"
```

Better: never call async methods directly from sync context. Use a wrapper:
```python
def _call_async(self, coro_func, *args, **kwargs):
    """Safe async dispatch from sync context. Works with or without @work."""
    result = coro_func(*args, **kwargs)
    # If it returned a coroutine (no @work), schedule it
    if asyncio.iscoroutine(result):
        asyncio.ensure_future(result)
```

**Why:** Catches both patterns. Doesn't matter if someone forgets `@work`.

---

## 3. Error Display: Dedicated, Context-Independent

**Problem:** Six attempts to make footer sticky errors visible. All failed because:
- `set_timer()` unreliable in `@work` workers
- CSS `display:none` not yet applied when widget written
- `_render()` not a Textual lifecycle hook — `App.refresh()` never calls it
- `call_after_refresh` races with worker output
- Toast guard blocks sticky error check

**Fix:** Add a dedicated error surface that doesn't share CSS with the footer:

```python
# Option A: Log file (simplest, most reliable)
def _worker_error(self, msg: str):
    with open(Path.home() / ".clavus" / "errors.log", "a") as f:
        f.write(f"{time.strftime('%Y-%m-%d %H:%M:%S')} {msg}\n")
    self._status(f"❌ error — see ~/.clavus/errors.log")

# Option B: Dedicated banner (visible in all CSS states)
# Add to compose(): Static("", id="error-banner")
# CSS: #error-banner { dock: top; height: auto; background: red; }
# Never display:none, never shares #footer CSS

# Option C: Push a modal Screen (unmissable)
def _show_fatal_error(self, msg: str):
    self.push_screen(ErrorScreen(msg))
```

**Why:** The footer is too contested — CSS hiding, toast timers, `call_after_refresh`, `_render` lifecycle. Errors need their own space.

---

## 4. State Scoping: Per-Project, Not Per-Remote

**Problem:** `remote.last_head` was one global value per remote. Switching projects left `last_head` pointing at the wrong project's HEAD → 409 Conflict on every push.

**Fix (already applied):** `ClavusProject.last_remote_head`. But audit for others:
- `remote.last_sync` — should this be per-project?
- `self._peer_name` — already one value, but validated per-operation?
- Any other global state in `ClavusApp` that's project-specific?

**Audit checklist:**
```python
# Things to check for per-project scoping:
# - self._peer_name (OK — one active remote, validated each push/pull)
# - self._peer_reachable (OK — transient, probed on connect)
# - self._last_sync (OK — cosmetic, display-only)
# - self._sync_status (OK — transient)

**Relay HEAD race condition — reviewed May 13, 2026:**
- `push_to_remote` sends `expected_parent = proj.last_remote_head or remote.last_head`
- On 409 Conflict, the 409 handler auto-updates `proj.last_remote_head` to the relay's actual head, so the next push is clean
- The only window for overwriting a newer relay HEAD requires: push A sends → push B sends → A gets 409 with B's new head → A auto-assimilates B's head. Correctly handled.
- `force=True` bypasses the lock intentionally (manual override)
- **Conclusion: Already handled. No code changes needed.**
```

---

## 5. Defensive Imports

**Problem:** `time` not imported in the subprocess branch of `_do_command`. The function has inline imports scattered throughout. Different branches have different imports available.

**Fix:** Move all imports to top of function or top of file. Never `import` inside a conditional branch.
```python
# Top of _do_command (or top of file)
import subprocess, sys, time, asyncio, pathlib
```

**Why:** `UnboundLocalError` from a missing import is a crash that happens in a branch you're not looking at.

---

## 6. Diagnostic Mode

**Problem:** Five hours of debugging because errors were invisible. Every hypothesis was wrong because we couldn't see what was actually happening.

**Fix:** Add a `--debug` flag that:
- Logs every command dispatch to a file
- Prints worker start/stop/error to stderr
- Shows event loop timing (when CSS applied vs when worker ran)
- Dumps `_toast_timer` state on every footer update

```python
# clavus tui --debug
# Writes ~/.clavus/debug.log with:
# [22:14:01.234] dispatch: cmd='pull' arg='all' → branch='subprocess'
# [22:14:01.235] subprocess: running 'clavus pull all'
# [22:14:01.236] UnboundLocalError: cannot access local variable 'time'
# [22:14:01.237] worker _run_pull_all: NEVER REACHED
```

---

## 7. Testing: Add a Smoketest Script

**Problem:** No way to verify the TUI is functional without manual testing. Every change risked breaking silent dispatch paths.

**Fix:** A headless smoketest that simulates colon commands:
```python
# tests/smoke_commands.py
# Runs clavus tui in headless mode, sends :commands, checks output
# Catches: dispatch failures, import errors, async scheduling bugs
```

---

## Priority Order

| Priority | Item | Effort | Impact |
|----------|------|--------|--------|
| **P0** | #3 Error display — log file fallback | 30 min | Every future bug visible |
| **P0** | #2 @work audit — find all bare async handlers | 15 min | No more silent coroutine drops |
| **P1** | #1 Command dispatch table | 1 hr | No more parsing ambiguity |
| **P1** | #5 Defensive imports | 15 min | No more UnboundLocalError |
| **P2** | #6 Diagnostic mode | 2 hr | 10x faster debugging |
| **P2** | #7 Smoketest | 1 hr | Catch regressions before user does |
| **P3** | #4 State scoping audit | 30 min | Already fixed, verify |

**Do P0 today.** The rest can ship over the next few sessions.
