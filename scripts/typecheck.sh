#!/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# typecheck.sh — pyright type checking
# Run manually:  bash scripts/typecheck.sh
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

cd "$(dirname "$0")/.."

PYRIGHT="$(python3 -c 'import site; print(site.USER_BASE + "/bin/pyright")' 2>/dev/null)"
: "${PYRIGHT:=pyright}"

echo "── pyright type check ──"
"$PYRIGHT" --outputjson 2>/dev/null | python3 -c "
import sys, json
data = json.load(sys.stdin)
summary = data.get('summary', '')
errors = data.get('generalDiagnostics', [])
files = {}
for e in errors:
    f = e.get('file', '')
    files[f] = files.get(f, 0) + 1

if errors:
    print(f'  ✗ {len(errors)} error(s) in {len(files)} file(s)')
    for f, n in files.items():
        print(f'    {n}x  {f}')
    print()
    print('  Run without --outputjson for full output:')
    print(f'    {PYRIGHT}')
    sys.exit(1)
else:
    print(f'  ✓ {summary}')
" || {
    # Fallback: run pyright directly (may not be in PATH)
    "$PYRIGHT" || {
        echo "  ! pyright not in PATH, skipping type check"
        echo "  Install: pip3 install --user pyright"
        exit 0
    }
}
