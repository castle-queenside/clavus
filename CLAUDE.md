# CLAUDE.md — Clavus Development Guide

## Project Overview

**Clavus** is a local-first DAW project versioning and collaboration tool. It runs on macOS, Windows, and Linux, supports Ableton Live projects (more DAWs on roadmap), and syncs via Tailscale relay without cloud services or accounts.

**Key directories:**
- `clavus/` — main Python package
  - `cli.py` — all CLI commands (`clavus <subcommand>`)
  - `tui.py` — Textual TUI (~4500 lines, the core UI)
  - `config.py` — `ClavusConfig` (user settings, JSON on disk)
  - `store.py` — `BlobStore` (content-addressed object store, SQLite)
  - `sync.py` — relay sync, push/pull logic, Remote management
  - `discovery.py` — mDNS/Tailscale peer discovery
  - `ableton.py` — `.als` parsing, marker injection
  - `backup.py` — store backup / restore logic
- `docs/` — architecture notes, postmortems, collaborator quickstart
- `fixtures/` — test `.als` generator (`gen_fixture.py`)
- `install.sh` / `install.ps1` — one-command installers

## Critical Conventions

### Windows Encoding Bug (May 2025)
**All async pipe `.decode()` calls MUST use `.decode('utf-8', errors='replace')`.** Bare `.decode()` crashes on Windows cp1252 with emoji output.

Affected locations in `tui.py`:
- `_run_pull_all_async` — decode stdout/stderr with errors='replace'
- `inject` subprocess call
- `_worker_error` display (already fixed: display:none in CSS)

**Also:** Subprocess env must include `PYTHONIOENCODING=utf-8` on Windows.

### Python Path (macOS)
Python 3.14 is installed from python.org as `python3`, NOT `python`. Scripts in `/Library/Frameworks/Python.framework/Versions/3.14/bin/`. `python` command does NOT exist.

### Tailscale Relay
- Default relay port: `7891`
- Tailscale serve maps it: `tailscale serve --bg --http 7890 http://localhost:7891`
- MagicDNS format: `{hostname}.tail{zone}.ts.net`
- Cross-account TCP: raw `100.x.x.x` IPs are blocked. Must use MagicDNS.
- `tailscale serve` config is wiped on reinstall — must re-run after Tailscale reinstall

### Config Location
- macOS/Linux: `~/.config/clavus/config.json`
- Config class: `ClavusConfig.load()` / `.save()` / `.set(key, value)`

### Store Location
- `~/.clavus/` — projects, objects (content-addressed blobs), stems, snapshots

## Common Tasks

### Run the TUI
```bash
cd /Users/slowhands/Developer/clavus
pip install -e .    # or py -m pip install -e . on Windows
clavus tui
```

### Run a single test
```bash
python test_snapshot.py
python test_cues.py
```

### Start a relay
```bash
clavus share --port 7891
tailscale serve --bg --http 7890 http://localhost:7891
```

### Reset/store nuke
```bash
# Projects + store + config
rm -rf ~/.clavus ~/.config/clavus

# Or via CLI
clavus nuke --everything
```

## Architecture Notes

### Content Addressing
Snapshots and blobs use SHA256-like hashes (8-char prefix shown in TUI). Store key: `objects/{hash[:2]}/{hash}.json` (meta) and `objects/{hash[:2]}/{hash}` (data).

### Relay Sync Protocol
Push: `POST /api/push` with snapshot + blob refs → relay stores, advances HEAD.  
Pull: `GET /api/pull?project=<name>` → returns all refs since last sync.  
P2P direct: same protocol but on `:7892` directly between peers.

### Ableton .als Format
Gzipped XML. Clavus stores the raw compressed bytes and parses XML for cues/diffs. When restoring, writes the raw bytes back — Ableton handles decompression on open.

### TUI Patterns (Textual)
- `_run_setup()` — invokes `cmd_setup()` in subprocess, reads output line-by-line
- `_run_pull_all_async` — runs `clavus pull --all` in bg task, probes with `/api/ping`
- All subprocess decode uses `errors='replace'` on Windows
- `SelectorList` + `Static` for history/snapshot/cue items
- `ModalScreen` subclasses for conflicts, help, project picker
- Key bindings: lowercase for actions, uppercase for mode switches (S=snapshot, P=push)

## Design Language

- **Logo:** ⬡ (hexagon)
- **Colors:** Dark `#0b1418` background, teal `#1a9e9e` Clavus accent, amber `#e57200` CRUX (sister project)
- **Box-drawing** characters for borders and separators
- **Keyboard-first:** all primary actions accessible without mouse
- **Spacing:** generous whitespace, nothing cramped

## Dev Commands

```bash
# Lint (must pass before commit)
python -m py_compile clavus/*.py

# Generate test fixture
python fixtures/gen_fixture.py

# Full test suite
python test_parser.py && python test_snapshot.py && python test_cues.py && python test_watch.py && python test_cli.py && python test_cli_full.py
```