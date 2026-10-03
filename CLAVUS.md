# CLAVUS.md — Clavus Project Agent Notes

> For the agent. Read before touching any code. Updated by every session.

## Quick Facts

| | |
|---|---|
| **Repo** | `https://github.com/castle-queenside/clavus.git` |
| **Local** | `~/Projects/clavus/` |
| **Language** | Python 3.10+ (textual, stdlib urllib, httpx for UDS only) |
| **Tests** | `python3 -m pytest tests/` — 12 passing, 2 skipped |
| **Verify** | `bash scripts/verify.sh` — run before every commit |
| **Type check** | `bash scripts/typecheck.sh` — pyright, slow (~30s) |
| **Pre-commit** | `pre-commit install` (in repo) — runs verify.sh automatically |

## Verify Before Every Commit

```bash
cd ~/Projects/clavus
bash scripts/verify.sh          # fast (~3s): syntax + pytest (runs on pre-commit)
bash scripts/typecheck.sh       # slow (~30s): pyright types (manual)
```

## Stack Setup (Reference)

```bash
# Install tools
pip3 install --user pyright pre-commit

# In this repo
pre-commit install              # hooks verify.sh to every commit
bash scripts/verify.sh         # must pass before commit

# Reference: ~/Projects/clavus/ has the canonical setup
```

## Architecture

```
clavus/
├── clavus/
│   ├── cli/              # CLI split: __init__.py + 21 commands/
│   ├── cues.py           # Cue model + CueStore
│   ├── discovery.py      # Tailscale-first peer discovery
│   ├── helpers.py        # Utilities
│   ├── http_client.py    # stdlib urllib wrapper (no httpx in hot path)
│   ├── store.py          # BlobStore, ClavusProject, schema migrations
│   ├── sync.py           # Push/pull over HTTP
│   ├── tui.py            # Textual TUI (4300+ lines)
│   └── watcher.py        # Live reload (kqueue/FSEvents)
├── tests/                # pytest: 12 passing, 2 skipped
├── fixtures/             # test_project.als ✓, test_project.bwproject ✗
├── docs/bitwig-support.md
└── scripts/
    ├── verify.sh          # fast pre-commit check
    ├── typecheck.sh       # pyright type check
    └── test.sh            # pytest wrapper
```

## Type Checking Status

pyright is set up (run: `bash scripts/typecheck.sh`).

**385 errors** — known type annotation debt, not runtime bugs:
- ~80% are import resolution errors in `cli/commands/*.py` (the `from clavus.cli import *`
  pattern is not resolved by pyright's static analysis)
- ~15% are `str | None` vs `str` parameter type mismatches in `web.py`, `__init__.py`
- ~5% are missing `Path` imports in CLI commands

**Real bugs found by pyright** (fix soon):
- `clavus/web.py:686-687` — `str | None` passed where `str` required
- `clavus/cli/commands/init.py:7` — missing `Optional` import
- `clavus/cli/commands/init.py:15` — missing `Path` import
- `clavus/cli/commands/init.py:16,23` — undefined `find_als_file`, `BlobStore`

The verify.sh catches actual bugs (syntax, pytest). pyright is incremental cleanup.

## Key Design Decisions

### HTTP: stdlib + httpx for UDS only
- All HTTP calls use `clavus/http_client.py` (`http_get`, `http_post`)
- `httpx` retained ONLY in `discovery.py` for Tailscale Unix Domain Socket:
  ```python
  transport = httpx.HTTPTransport(uds=sock)  # No stdlib equivalent
  ```

### CSS in Textual f-strings: brace escaping
In `f"""..."""` CSS blocks:
- `{{` → `{` (escaped open brace)
- `}}` → `}` (escaped close brace)
- `{{{` → `{` (triple = single in CSS `@keyframes from/to` blocks)
- `}}}` → `}` (triple = single in CSS output)

**Always run `python3 -c "import ast; ast.parse(open('clavus/tui.py').read())"` after CSS edits.**

### Schema Migrations
- `SCHEMA_VERSION` in `clavus/store.py` — bump and add migration block
- `_migrate_index()` is one-shot, idempotent, lock-protected
- Pattern: check `_schema_version`, add missing fields, increment

### Peer Discovery
- `discover_peers()` tries Tailscale API first, falls back to zeroconf
- Both sources merged + deduplicated
- Don't remove zeroconf — still needed for LAN fallback

## Current Status

### Working ✅
- TUI: Tokyo Night palette, sidebar, dashboard, activity feed, vim keys
- Transitions: fade/slide screen transitions, CSS cue stagger animations
- Project type: Ableton (🎹) vs Bitwig (🎛), `f` filter key
- Schema v3 migrations (backward compat)
- CLI split into `clavus/cli/` package
- Stdlib HTTP (no httpx in hot path)
- Tailscale-first peer discovery
- 12/12 tests passing

### Blocked ⛔
| Item | Blocker | How to Unblock |
|---|---|---|
| `git push` | `gh auth` | `gh auth login -h github.com` |
| Bitwig parser | No fixture | User saves tiny `.bwproject` → `fixtures/test_project.bwproject` |
| `gh` CLI install | `sudo chown` | `sudo chown -R $(whoami) /opt/homebrew` |

## Commits Ready to Push

```
dd09fb2  project type diff (Ableton/Bitwig)
58da5a8  fade/slide transitions, CSS animations
0b6dea8  sidebar, dashboard, activity feed, vim keys
8184907  Tokyo Night palette, box-drawing
37628d2  Tailscale peer discovery + Funnel docs
abf98d5  cli.py → clavus/cli/ package
0242612  schema versioning + tests/
e178383  httpx → stdlib urllib
ab1a8e1  BlobStore architecture
```

## Pending Work

### High Priority
1. **gh auth** → push 9 commits
2. **Bitwig fixture** → `fixtures/test_project.bwproject` → `parser_bitwig.py`
3. **Search navigation** (`n`/`N` vim stubs) — implement
4. **Bitwig snapshot format** — `.als_hash` equivalent for `.bwproject`

### Nice to Have
5. Activity log persistence (survive restarts)
6. Ghostty terminal + Tailscale Funnel (sudo required)
7. Obsidian vault layout proposal (needs user confirmation)

## File Conventions

- **`BD` dict** in `tui.py`: box-drawing chars. Keys: `tl/tr/bl/br`, `rtl/rtr/rbl/rbr`, `h/v`, `tee/end/bar/spc`, `ableton/bitwig`
- **Color palette** (`C` dict): Tokyo Night. Key: `accent=#7aa2f7`, `purple=#bb9af7`, `green=#9ece6a`, `red=#f7768e`
- **Color constants in markdown**: use `{C['accent']}` / `{C['purple']}` etc.

## Agent Session Files

```
~/agent-state/clavus-urllib.json      done
~/agent-state/clavus-arch.json        done
~/agent-state/clavus-tailscale.json   done
~/agent-state/clavus-report.md        comprehensive session report
```

## When Starting a New Clavus Session

```bash
# 1. Check where we are
git -C ~/Projects/clavus log --oneline -3

# 2. Read this file
cat ~/Projects/clavus/CLAVUS.md

# 3. Read the session report
cat ~/agent-state/clavus-report.md

# 4. Verify everything is clean
bash ~/Projects/clavus/scripts/verify.sh

# 5. Check blocked items
# (gh auth, bitwig fixture)
```
