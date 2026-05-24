# Contributing to Clavus

Weclome! Clavus is a personal tool that's grown into something useful enough to share. This guide explains how to work on it.

---

## What to Work On

Check the [issue tracker](https://github.com/castle-queenside/clavus/issues) for open bugs and planned features. The roadmap in README.md lists what's coming.

**Good first contributions:**
- Test on Windows (the most likely place things break)
- Report bugs with steps to reproduce
- Improve documentation
- Fix anything marked `good first issue`

**Before starting a bigger change:** open an issue first to make sure it's aligned with the direction.

---

## Dev Setup

### Clone
```bash
git clone https://github.com/castle-queenside/clavus
cd clavus
```

### Install
```bash
pip install -e .          # macOS/Linux
py -m pip install -e .    # Windows
```

### Run tests
```bash
python test_parser.py
python test_snapshot.py
python test_cues.py
python test_watch.py
python test_cli.py
python test_cli_full.py
```

### Run TUI
```bash
python -m clavus tui   # recommended — avoids PATH issues on Windows
# or: clavus tui       # if pip install worked cleanly
```

---

## Making Changes

### 1. Branch off main
```bash
git checkout main
git pull
git checkout -b feature/my-feature
```

### 2. Write code
- Python ≥ 3.10, no external deps beyond what's in `pyproject.toml`
- Run `python -m py_compile clavus/*.py` before committing (catches syntax errors)
- All subprocess `.decode()` calls on Windows must use `errors='replace'`

### 3. Test

At minimum:
- `python test_parser.py` — ALS parsing
- `python test_snapshot.py` — snapshot create/restore
- `python test_cues.py` — cue CRUD
- `python test_cli.py` — CLI commands

For anything touching the relay or sync:
- `python test_cli_full.py` — full round-trip tests

### 4. Commit
```bash
git add <changed files>
git commit -m "description of change"
git push origin feature/my-feature
```

Use imperative mood: "fix", "add", "improve", "remove" — not "fixed" or "adding".

### 5. Pull request
Open a PR against `main`. Describe what changed and why. Link any related issues.

---

## Project Structure

```
clavus/              # main package
  cli.py             # all clavus <subcommand> entry points
  tui.py             # Textual TUI (~4500 lines)
  config.py          # ClavusConfig — user settings JSON
  store.py           # BlobStore — content-addressed SQLite store
  sync.py            # relay sync, push/pull, Remote management
  discovery.py       # mDNS + Tailscale peer discovery
  ableton.py         # .als parsing, marker injection
  backup.py          # store backup / restore
  cues.py            # Cue data model
  project.py         # Project data model
  snapshot.py        # Snapshot data model

docs/
  collaborator-quickstart.md  # how collaborators set up

fixtures/
  gen_fixture.py     # generates test .als files
  test_project.als   # synthetic Ableton project for tests
```

---

## Key Patterns

### Adding a CLI command
1. Add function `def cmd_<name>(args)` in `clavus/cli.py`
2. Register it in `main()`'s `subparsers.add_parser()`
3. Document in README.md command reference

### Content-addressed storage
- Hash algorithm: SHA256 of file content, 8-char prefix shown in TUI
- Store path: `objects/{hash[:2]}/{hash}` (no extension for data, `.json` for metadata)
- All access via `BlobStore.put(content)` / `BlobStore.get(hash)` / `BlobStore.has(hash)`

### Tailscale relay
- Relay process: `clavus share --port 7891` → HTTP daemon on port 7891
- Expose via Tailscale: `tailscale serve --bg --http 7890 http://localhost:7891`
- API endpoints: `GET /api/ping`, `POST /api/push`, `GET /api/pull`, `GET /api/projects`
- Clients join with: `clavus join http://host.tailXXXX.ts.net:7891`

### Windows encoding
All pipe reads in async contexts must decode with `errors='replace'`:
```python
stdout=await process.stdout.read()
text = stdout.decode("utf-8", errors="replace")
```
Bare `.decode()` crashes on cp1252 with emoji.

---

## When Things Break

Check the issue tracker for similar issues — many real failures are documented there with root causes and fixes.

The most common failure modes:
1. **Tailscale serve config lost** — rerun `tailscale serve --bg --http 7890 http://localhost:7891`
2. **Port 7891 in use** — pick a different port with `--port`
3. **Cross-account Tailscale** — machine must be shared via admin.tailscale.com
4. **OneDrive syncing .als files** — move projects to a local non-synced folder
5. **Frozen tracks in .als** — unfreeze before snapshotting (crashes Ableton cross-platform)

---

## Questions

Open an issue or reach out. Response time varies — this is a side project.