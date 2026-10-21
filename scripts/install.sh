#!/usr/bin/env bash
# Build the Rust clavus binary and install it as `clavus` on PATH.
#
# Replaces the Python console script (kept as `clavus-py`) so `clavus tui`
# launches the Rust build.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"

# Where the Python pip console script lives (first match wins).
# A dir qualifies if it holds a python `clavus` shim, OR a `clavus-py` alias
# (meaning we already replaced the shim there on a previous run).
PYTHON_BIN_DIR=""
for d in "$HOME/Library/Python/3.10/bin" "$HOME/Library/Python/3.9/bin" \
         "$HOME/.local/bin" "/usr/local/bin"; do
    if [ -e "$d/clavus-py" ]; then
        PYTHON_BIN_DIR="$d"
        break
    fi
    if [ -f "$d/clavus" ] && head -1 "$d/clavus" 2>/dev/null | grep -q python; then
        PYTHON_BIN_DIR="$d"
        break
    fi
done

# If no python shim found, install to ~/.local/bin
if [ -z "$PYTHON_BIN_DIR" ]; then
    PYTHON_BIN_DIR="$HOME/.local/bin"
    mkdir -p "$PYTHON_BIN_DIR"
fi

echo "▸ Building release binary…"
cd "$REPO_ROOT/rust"
"$CARGO" build --release

BIN="$REPO_ROOT/rust/target/release/clavus"
if [ ! -f "$BIN" ]; then
    echo "✗ Build produced no binary at $BIN" >&2
    exit 1
fi

echo "▸ Installing to $PYTHON_BIN_DIR/clavus…"

# Preserve the old Python entry point as clavus-py (if it looks like a python shim)
if [ -f "$PYTHON_BIN_DIR/clavus" ] \
   && head -1 "$PYTHON_BIN_DIR/clavus" 2>/dev/null | grep -q python \
   && [ ! -e "$PYTHON_BIN_DIR/clavus-py" ]; then
    cp "$PYTHON_BIN_DIR/clavus" "$PYTHON_BIN_DIR/clavus-py"
    echo "  ✓ old Python build preserved as clavus-py"
fi

cp "$BIN" "$PYTHON_BIN_DIR/clavus"
chmod +x "$PYTHON_BIN_DIR/clavus"

echo
echo "✓ Installed:"
echo "    $PYTHON_BIN_DIR/clavus  →  $($PYTHON_BIN_DIR/clavus --version)"
echo "    $BIN  ($(du -h "$BIN" | cut -f1))"
echo
echo "Run:  clavus tui"
