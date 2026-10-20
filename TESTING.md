# Clavus Testing Plan

> Last updated: 2026-10-20
> Status: 🚧 In progress

## Coverage at a Glance

| Area | Unit tests | Integration tests | Manual | Status |
|------|-----------|-----------------|--------|--------|
| Parser ALS | 27 | 0 | Fixture only | 🟢 |
| Parser Bitwig | 2 | 0 | **Needs fixture** | 🔴 |
| BlobStore / Core | 25 | 0 | — | 🟡 |
| Sample audit + collect | 4 | 0 | ✅ Manual test | 🟡 |
| Relay sync | 0 | 0 | Manual test | 🔴 |
| Sync client | 0 | 0 | — | 🔴 |
| Conflict detection | 0 | 0 | Manual test | 🟡 |
| TUI | 0 | 0 | Manual only | 🟠 |
| CLI commands | 0 | 0 | ✅ Manual test | 🟡 |

Legend: 🟢 good  🟡 partial  🟠 manual only  🔴 missing

---

## Priority 1: Fix the Blanks

### 1.1 Relay + Sync Integration Tests

The relay has zero test coverage. This is the most critical gap — sync is a network
protocol with complex state (conflict detection, persistence, HEAD tracking).

**What to test:**

```rust
// crates/relay/tests/relay_tests.rs

#[test]
fn test_push_stores_snapshots() {
    // Start relay, connect client, push snapshots, verify relay stores them
}

#[test]
fn test_pull_returns_missing() {
    // Relay has snapshots A,B,C. Client knows A. Pull → gets B,C
}

#[test]
fn test_conflict_detected_on_divergent_push() {
    // Two clients, different HEAD chains → relay detects conflict
    // Relay sends SyncMsg::Conflict to both
}

#[test]
fn test_conflict_fast_forward_ok() {
    // Client A pushed HEAD=abc. Client B knows abc, pushes abc→def (ff) → no conflict
}

#[test]
fn test_persistence_survives_restart() {
    // Push snapshots, kill relay, restart relay, verify snapshots still there
}

#[test]
fn test_cues_synced_on_connect() {
    // Relay has cues. Client connects → receives cues in Push message
}

#[test]
fn test_dead_client_timeout() {
    // Slow/dying client: recv times out after 30s → connection closed
}
```

**Approach:** Start relay on a random port in each test, connect real `SyncClient`,
exchange messages. Use `std::thread::spawn` for the relay server.

**Owner:** automated

---

### 1.2 Sample Audit + Collect Integration Test

`collect_samples` and `audit_project` have zero integration tests. The manual test
proved the path rewriting works, but it should be automated.

**What to test:**

```rust
// crates/core/tests/collect_tests.rs

#[test]
fn test_collect_copies_and_rewrites() {
    // 1. Create temp dir with:
    //    - project.als (refs /external/sample.wav)
    //    - ../external/sample.wav
    // 2. Run audit → finds outside_project sample
    // 3. Run collect_samples → copies to project/Samples/
    // 4. Re-parse .als → verify path is now "Samples/sample.wav"
}

#[test]
fn test_collect_idempotent() {
    // Run collect twice → second run finds 0 outside-project samples
}

#[test]
fn test_collect_missing_file_propagates_error() {
    // Sample ref exists in XML but file gone on disk → collect fails gracefully
}

#[test]
fn test_audit_with_real_fixture() {
    // audit_project on real test_project.als → matches expected sample count
}
```

**Owner:** automated

---

### 1.3 Bitwig Parser — Needs Fixture

The Bitwig parser has zero real tests. `test_parse_bitwig_fixture` is `#[ignore]`.

**Action required:** User saves any `.bwproject` in Bitwig Studio to
`~/Projects/clavus-rust/fixtures/test_project.bwproject`, then:

```bash
# After fixture is added:
cd ~/Projects/clavus-rust/rust
# Remove #[ignore] from test_parse_bitwig_fixture
cargo test -p clavus-parser-bw
```

**Owner:** User (needs Bitwig Studio)

Until then, at minimum add these without a fixture:

```rust
// crates/parser-bw/tests/bw_parser.rs — add

#[test]
fn test_parse_returns_err_for_corrupt_zip() {
    let tmp = temp_dir.join("corrupt.bwproject");
    std::fs::write(&tmp, b"not a zip").ok();
    let r = BitwigParser::new(&tmp);
    assert!(r.is_err());
}

#[test]
fn test_parse_returns_err_for_truncated_zip() {
    let tmp = temp_dir.join("truncated.bwproject");
    let valid_zip = create_valid_minimal_bwproject_zip();
    std::fs::write(&tmp, &valid_zip[..valid_zip.len()/2]).ok();
    let r = BitwigParser::new(&tmp);
    assert!(r.is_err());
}
```

---

## Priority 2: Fill the Gaps

### 2.1 SyncClient Unit Tests

```rust
// crates/sync/tests/client_tests.rs

#[test]
fn test_hello_contains_known_heads() { ... }
#[test]
fn test_recv_timeout_returns_none_on_close() { ... }
#[test]
fn test_send_after_close_returns_error() { ... }
#[test]
fn test_conflict_msg_variant() {
    // Verify SyncMsg::Conflict deserializes correctly
    let json = r#"{"type":"conflict","project":"Test","relay_head":"abc","push_head":"def"}"#;
    let msg: SyncMsg = serde_json::from_str(json).unwrap();
    assert!(matches!(msg, SyncMsg::Conflict { .. }));
}
```

### 2.2 SnapshotStore Tests

```rust
// crates/core/tests/snapshot_tests.rs

#[test]
fn test_save_and_load_snapshot() { ... }
#[test]
fn test_head_tracking() {
    // save() updates HEAD ref after each snapshot
}
#[test]
fn test_walk_chain() {
    // Build chain: A→B→C. walk_chain(C) → [C,B,A]
}
#[test]
fn test_chain_length() { ... }
```

### 2.3 TUI Key Handler Tests

```rust
// crates/tui/tests/key_tests.rs

#[test]
fn test_snapshot_hint_on_c_press() {
    // Render with project selected, press 'c', check hint shown
}

#[test]
fn test_filter_cycles_through_states() {
    // Press 'f' → Pending → Resolved → Skipped → all → Pending
}
```

Note: TUI rendering tests need to mock terminal output. Consider snapshot testing
with `insta` crate for TUI state.

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
