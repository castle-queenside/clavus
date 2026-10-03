#!/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# setup-stack.sh — bootstrap a professional agent stack in any ~/Projects repo
# Usage:  bash scripts/setup-stack.sh <project-name>
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

if [ -z "${1:-}" ]; then
  echo "Usage:  bash scripts/setup-stack.sh <project-name>"
  echo "  e.g.:  bash scripts/setup-stack.sh my-awesome-project"
  exit 1
fi

PROJECT="$1"
REPO_DIR="$(dirname "$0")/.."
TARGET_DIR="$HOME/Projects/$PROJECT"

echo "── Cloning or updating stack in $TARGET_DIR ──"

if [ ! -d "$TARGET_DIR" ]; then
  echo "  ! $TARGET_DIR does not exist yet"
  echo "  Run this script AFTER creating the project directory"
  exit 1
fi

# ── Copy stack files ───────────────────────────────────────────────────────────
cp "$REPO_DIR/scripts/verify.sh"      "$TARGET_DIR/scripts/verify.sh"
cp "$REPO_DIR/scripts/typecheck.sh"   "$TARGET_DIR/scripts/typecheck.sh"
cp "$REPO_DIR/.pre-commit-config.yaml" "$TARGET_DIR/.pre-commit-config.yaml"
cp "$REPO_DIR/pyrightconfig.json"     "$TARGET_DIR/pyrightconfig.json"
chmod +x "$TARGET_DIR/scripts/verify.sh"
chmod +x "$TARGET_DIR/scripts/typecheck.sh"

# ── Install pre-commit hooks ──────────────────────────────────────────────────
cd "$TARGET_DIR"
pre-commit install 2>/dev/null && echo "  ✓ pre-commit hooks installed" || echo "  ! pre-commit not available"

# ── Create CLAVUS.md placeholder ───────────────────────────────────────────────
if [ ! -f "$TARGET_DIR/CLAVUS.md" ]; then
  cat > "$TARGET_DIR/CLAVUS.md" << 'EOF'
# CLAVUS.md — Project Agent Notes

> For the agent. Read before touching any code.

## Quick Facts

| | |
|---|---|
| **Repo** | |
| **Local** | `~/Projects/` |
| **Tests** | `python3 -m pytest tests/` |
| **Verify** | `bash scripts/verify.sh` |

## Stack

- `scripts/verify.sh` — fast pre-commit check (syntax + tests)
- `scripts/typecheck.sh` — pyright type checking
- `.pre-commit-config.yaml` — pre-commit hooks
- `pyrightconfig.json` — pyright settings

## Status

### Working
-

### Blocked
-

### Pending Work
-

## Design Decisions
-
EOF
  echo "  ✓ CLAVUS.md created (edit it!)"
fi

# ── Run verify to confirm setup ───────────────────────────────────────────────
echo ""
echo "── Verifying setup ──"
cd "$TARGET_DIR"
bash scripts/verify.sh 2>/dev/null && echo "  ✓ verify.sh passed" || echo "  ! verify.sh has issues"

echo ""
echo "Done! Edit $TARGET_DIR/CLAVUS.md to add project context."
