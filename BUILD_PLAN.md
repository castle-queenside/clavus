# Clavus Build Plan — v0.6.0 Release Prep

## Current State (May 2026)

Everything core is built and working. What remains is polish, hardening, and docs for public release.

## ✅ Complete

| Area | What | Status |
|------|------|--------|
| Parser | .als XML (Ableton 10+ wrapper + Live 9 root) | ✅ |
| Snapshots | Content-addressed, diff engine, visual diff | ✅ |
| CLI | init, snapshot, log, diff, status, restore | ✅ |
| Cues | CRUD, threaded replies, assignees, marker injection | ✅ |
| Watch daemon | File watcher + auto-snapshot (polling, cross-platform) | ✅ |
| TUI | Full keyboard-driven cues + history management | ✅ |
| Sync | P2P push/pull of cues, snapshots, stems | ✅ |
| WebSocket | Real-time sync between TUI and server | ✅ |
| Stems | Import, list, push, pull, dedup via blob store | ✅ |
| Restore | Full .als restore from snapshot (CLI + TUI + API) | ✅ |
| Diff in TUI | Inline text diff via `d` key | ✅ |
| Share/Join | Tailscale-first peer discovery, human-friendly codes | ✅ |
| Relay | API-only server for collaboration | ✅ |
| Git integration | Branch, checkout, merge via CLI | ✅ |
| Keyboard | Hammerspoon (macOS) + AutoHotkey (Windows) | ✅ |
| mDNS discovery | LAN peer discovery via zeroconf | ✅ |
| Tailscale | Tailnet device scanning via local API | ✅ |
| Config | File/env/CLI inheritance, config wizard | ✅ |
| Repair | index.json auto-backup + recovery + `clavus repair` | ✅ |
| Store backup | Full tar.gz backup/restore (CLI + TUI) + daily auto-backup | ✅ |
| TUI auto-relay | TUI spawns relay server if none running | ✅ |
| Cross-platform | All tests isolated (never touch real ~/.clavus) | ✅ |
| Web companion | Removed (HTML/CSS/JS/M4L bloat) | ✅ |
| Test isolation | All tests use temp dirs, never destroy real store | ✅ |

## 🔲 Remaining for Public Release

### 1. Polish & UX
- [ ] Handle port-in-use gracefully in relay auto-start
- [ ] `:share` from TUI should try alternate port if 7890 busy
- [ ] Better connection-lost recovery in TUI
- [ ] `CLAVUS_NO_AUTO_RELAY` env var documented

### 2. Packaging
- [ ] Verify `pip install clavus` from clean venv
- [ ] Version bump to 0.7.0 or 1.0.0-beta
- [ ] Add PyPI long description from README
- [ ] Verify CI passes on all Python 3.10-3.13

### 3. Documentation
- [x] README — stripped web companion, share/join, platform compat
- [x] SETUP_STEVEN.md — updated for share/join, no web companion
- [x] BUILD_PLAN.md — this file, current
- [x] CLI help text — cleaned up serve → relay references
- [ ] README — add backup/restore commands to quick reference

### 4. Edge Cases
- [ ] Handle TUI starting with no projects in store
- [ ] Auto-reconnect relay after network interruption
- [ ] Windows: Tailscale local API via HTTP (not Unix sockets)
