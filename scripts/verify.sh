#!/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# verify.sh — fast sanity check before every commit (Rust branch)
# Run manually:  bash scripts/verify.sh
# Or install as pre-commit:  pre-commit install
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

cd "$(dirname "$0")/.."
RUST="${HOME}/.cargo/bin"

# rust/ is the Cargo workspace root
cd rust

echo "── cargo check ──"
"${RUST}/cargo" check --all-targets
echo "  ✓ cargo check"

echo "── cargo test ──"
"${RUST}/cargo" test --all
echo "  ✓ cargo test"

echo "── no debug code in staged files ──"
if command -v git &>/dev/null && git rev-parse --git-dir &>/dev/null 2>/dev/null; then
  staged=$(git diff --cached --name-only 2>/dev/null | tr '\n' ' ' || true)
  if [ -n "$staged" ]; then
    debug_found=$(echo "$staged" | xargs grep -En 'eprintln!\s*\(\s*"\[DEBUG' 2>/dev/null | grep -v '^[^:]*://' | head -5 || true)
    if [ -n "$debug_found" ]; then
      echo "  ✗ debug code found:"
      echo "$debug_found" | head -5
      exit 1
    fi
    echo "  ✓ no debug code"
  fi
fi

echo ""
echo "── all checks passed ──"
