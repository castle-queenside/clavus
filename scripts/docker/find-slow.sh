#!/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# find-slow.sh — find out which test binary is slow, and whether it hangs.
#
# `cargo test --workspace` prints one line per binary and hides the
# slow one inside the total. When a suite takes 37 minutes and the
# summary says nothing about where the time went, this runs each test
# binary on its own, with a time limit, and says which one is the
# culprit.
#
# It also catches a HANG: a binary that never finishes shows up as
# "TIMEOUT" rather than a pass, which is exactly what a test that
# waits for a write to fail looks like when it is running as root and
# the write succeeds.
#
# Run it the way the checks are run — as a normal user, with a home of
# its own. Root ignores permission bits, and a home of /tmp breaks the
# rule that a project outside the home is identified by the last two
# folders of its path.
#
# Usage:  bash scripts/docker/find-slow.sh [seconds-per-binary]
#   default limit: 300
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
VOLUME="clavus-cargo"
IMAGE="clavus-rust-check:latest"
LIMIT="${1:-300}"

HOST_UID="$(id -u)"
HOST_GID="$(id -g)"
DOCKER_HOME="/tmp/clavus-docker-home"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  docker build -t "$IMAGE" -f "$REPO/scripts/docker/checker.Dockerfile" "$REPO/scripts/docker"
fi

docker run --rm -v "$VOLUME":/cargo "$IMAGE" \
  chown -R "$HOST_UID:$HOST_GID" /cargo >/dev/null 2>&1 || true
mkdir -p "$DOCKER_HOME"

docker run --rm \
  --name clavus-slowscan \
  --user "$HOST_UID:$HOST_GID" \
  -i \
  -e HOME=/home \
  -v "$REPO":/w \
  -v "$VOLUME":/cargo \
  -v "$DOCKER_HOME":/home \
  -w /w/rust \
  -e CARGO_HOME=/cargo/home \
  -e CARGO_TARGET_DIR=/cargo/target \
  "$IMAGE" \
  bash -s -- "$LIMIT" <<'INNER'
set -u
LIMIT="$1"
export PATH="/usr/local/cargo/bin:$PATH"
cd /w/rust

echo "── building the test binaries ──"
cargo test --workspace --no-run >/dev/null 2>&1

find /cargo/target/debug/deps -maxdepth 1 -type f -executable \
  ! -name '*.d' ! -name '*.so' ! -name '*.rlib' | sort > /tmp/bins.txt
echo "── $(wc -l < /tmp/bins.txt) test binaries, ${LIMIT}s limit each ──"

slowest=0
slowest_name=""
total=0
while read -r b; do
  name=$(basename "$b")
  started=$SECONDS
  output=$(timeout "$LIMIT" "$b" --test-threads 4 2>&1 | tail -3)
  rc=$?
  took=$((SECONDS - started))
  total=$((total + took))
  if [ "$rc" -eq 124 ]; then
    printf "%-52s %5ss  TIMEOUT — this one hangs\n" "$name" "$took"
    continue
  fi
  if [ "$rc" -ne 0 ]; then
    printf "%-52s %5ss  FAILED\n" "$name" "$took"
    printf '%s\n' "$output" | sed 's/^/      /'
    continue
  fi
  printf "%-52s %5ss  ok\n" "$name" "$took"
  if [ "$took" -gt "$slowest" ]; then slowest=$took; slowest_name="$name"; fi
done < /tmp/bins.txt

echo ""
echo "── total ${total}s across every binary ──"
echo "── slowest: $slowest_name (${slowest}s) ──"
INNER
