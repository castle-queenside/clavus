# Clavus Testing Plan

> Last updated: 2026-10-20
> Status: 🚧 In progress

## Coverage at a Glance

| Area | Unit tests | Integration tests | Status |
|------|-----------|-----------------|--------|
| Parser ALS | 27 | 0 | 🟢 |
| Parser Bitwig | 2 | 0 | 🟡 (needs fixture) |
| BlobStore / Core | 25 | 0 | 🟡 |
| Sample audit + collect | 9 | 0 | 🟢 |
| Relay sync | 10 | 0 | 🟢 |
| Sync client | 0 | 0 | 🟠 |
| Conflict detection | 3 (relay) | 0 | 🟡 |
| TUI | 0 | 0 | 🟠 |
| CLI commands | 0 | 0 | 🟡 |

**78 tests passing, 1 ignored (needs Bitwig fixture).**

Legend: 🟢 good  🟡 partial  🟠 manual only  🔴 missing

---

## Coverage at a Glance

| Area | Unit tests | Integration tests | Status |
|------|-----------|-----------------|--------|
| Parser ALS | 27 | 0 | 🟢 |
| Parser Bitwig | 2 | 0 | 🟡 (needs fixture) |
| BlobStore / Core | 25 | 0 | 🟡 |
| Sample audit + collect | 9 | 0 | 🟢 ✅ DONE |
| Relay sync | 10 | 0 | 🟢 ✅ DONE |
| Sync client | 0 | 0 | 🟠 |
| Conflict detection | 3 (relay) | 0 | 🟡 |
| TUI | 0 | 0 | 🟠 |
| CLI commands | 0 | 0 | 🟡 |

**78 tests passing, 1 ignored (needs Bitwig fixture).**

---

## Priority 1: Done ✅

Relay + collect integration tests are written and passing. See:
- `crates/relay/tests/relay_tests.rs` — 10 tests
- `crates/core/tests/collect_tests.rs` — 9 tests

---

## Priority 2: Remaining Gaps

### 2.1 SyncClient Unit Tests

Add `crates/sync/tests/client_tests.rs`:
- `test_hello_contains_known_heads`
- `test_recv_timeout_returns_none_on_close`
- `test_send_after_close_returns_error`
- `test_conflict_msg_variant` (already covered in relay_tests.rs but add explicit unit test)

### 2.2 SnapshotStore Tests

Add `crates/core/tests/snapshot_tests.rs`:
- `test_save_and_load_snapshot`
- `test_head_tracking`
- `test_walk_chain`
- `test_chain_length`

### 2.3 TUI Key Handler Tests

Add `crates/tui/tests/key_tests.rs`. Consider `insta` crate for snapshot testing.

---

## Priority 3: Real-World Validation

### 3.1 Manual Test Checklist

Run these manually with a real Ableton project:

```
ENVIRONMENT CHECKS
□ Project with 0 samples (baseline — should pass cleanly)
□ Project with 10+ samples inside project folder (audit should pass)
□ Project with 3 samples outside project (collect needed)
□ Project with missing samples (audit should warn, snapshot should BLOCK)
□ Project with Unicode characters in path (/Users/chris/Projects/🎵/)
□ Project with very long path (>200 chars)
□ .als file is actually gzip (not plain XML)

COLLECT COMMAND
□ collect: copies files, rewrites .als, audit passes after
□ collect: idempotent (run twice — second run does nothing)
□ collect: fails gracefully if destination dir is read-only

SNAPSHOT LIFECYCLE
□ snapshot: saves blob + meta + HEAD ref
□ snapshot: blocks if samples missing (ensure_clean called)
□ log: shows chain with correct hashes and timestamps
□ log: shows correct parent-child relationships

RELAY SYNC
□ relay starts, persists state, restart recovers state
□ push/pull: snapshots sync between two clavus instances
□ conflict: divergent HEAD → both clients notified

TUI
□ Project list: correct track count, BPM, snapshot count
□ Snapshot list: navigation with ↑↓, selection resets on refresh
□ Cue list: filter cycling with 'f'
□ Sync connect: 's' key → connection state shown
□ Push/Pull: 'P'/'G' work when connected, hint when not
```

### 3.2 Property-Based Testing (Nice to Have)

Add `quickcheck` or `proptest` for:

```rust
// Parser: arbitrary XML that is valid LiveSet structure
// BlobStore: arbitrary bytes → deterministic SHA-256
// Sample audit: arbitrary path resolution decisions
// Conflict detection: arbitrary DAG of snapshots → ancestor/descendant queries
```

---

## CI Integration

```yaml
# .github/workflows/test.yml
name: Tests
on: [push, pull_request]
jobs:
  test:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Run all tests
        run: cargo test --all
      - name: Clippy
        run: cargo clippy --all -- -D warnings
      - name: Build release
        run: cargo build --release
```

---

## Test Fixtures

All test fixtures live in `~/Projects/clavus-rust/fixtures/`:

| File | Source | Purpose |
|------|--------|---------|
| `test_project.als` | `gen_fixture.py` | Minimal valid .als — 8 tracks, 5 markers, 128 BPM, no samples |
| `test_project.bwproject` | **User needed** | Any real Bitwig project |

**To generate a realistic .als fixture:**

```bash
cd ~/Projects/clavus-rust/fixtures
# Option A: use gen_fixture.py (already has 8 tracks)
python3 gen_fixture.py

# Option B: export a real Ableton project as .als and test with it
# (most valuable — tests real Ableton XML structure)
```

---

## Running Tests

```bash
cd ~/Projects/clavus-rust/rust

# All tests
~/.cargo/bin/cargo test --all

# One crate
~/.cargo/bin/cargo test -p clavus-core

# With output
~/.cargo/bin/cargo test --all -- --nocapture

# Specific test
~/.cargo/bin/cargo test -p clavus-parser-als test_sample_ref

# Fuzz once (not via cargo-fuzz)
~/.cargo/bin/cargo test -p clavus-parser-als test_fuzz

# Coverage
~/.cargo/bin/cargo install cargo-llvm-cov
~/.cargo/bin/cargo llvm-cov --all
```
