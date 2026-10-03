#!/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# verify.sh — fast sanity check before every commit
# Run manually:  bash scripts/verify.sh
# Or install as pre-commit:  pre-commit install
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

cd "$(dirname "$0")/.."

echo "── Python syntax ──"
python3 -c "import ast; ast.parse(open('clavus/tui.py').read())" && echo "  ✓ clavus/tui.py"
python3 -c "import ast; ast.parse(open('clavus/store.py').read())" && echo "  ✓ clavus/store.py"
python3 -c "import ast; ast.parse(open('clavus/cues.py').read())" && echo "  ✓ clavus/cues.py"
python3 -c "import ast; ast.parse(open('clavus/sync.py').read())" && echo "  ✓ clavus/sync.py"
python3 -c "import ast; ast.parse(open('clavus/discovery.py').read())" && echo "  ✓ clavus/discovery.py"
python3 -c "import ast; ast.parse(open('clavus/cli/__init__.py').read())" && echo "  ✓ clavus/cli/__init__.py"
echo "  ✓ all core modules"

echo "── pytest ──"
python3 -m pytest tests/ -q --tb=short

echo "── no debug code in staged files ──"
if command -v git &>/dev/null && git rev-parse --git-dir &>/dev/null 2>/dev/null; then
  # git diff --cached returns exit 0 even when empty; grep returns exit 1 on no match
  # → use || true so set -e doesn't exit on empty staging area
  staged=$(git diff --cached --name-only 2>/dev/null | grep '\.py$' | grep -v '^tests/fixtures/' | tr '\n' ' ' || true)
  if [ -n "$staged" ]; then
    set +e
    debug_found=$(echo "$staged" | xargs grep -En 'print\s*\(\s*#|import\s+pdb|import\s+ipdb|import\s+ptvsd|breakpoint\(\)' 2>/dev/null || true)
    set -e
    if [ -n "$debug_found" ]; then
      echo "  ✗ debug imports found:"
      echo "$debug_found" | head -5
      exit 1
    fi
    echo "  ✓ no debug imports"
  else
    echo "  (no staged .py files — skipping)"
  fi
fi

echo ""
echo "── all checks passed ──"
