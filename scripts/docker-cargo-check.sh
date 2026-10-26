#!/bin/bash
# ─────────────────────────────────────────────────────────────────────────────
# docker-cargo-check.sh — run the Rust checks on real Linux, warm.
#
# Why a script and not a one-liner: every previous Docker run used a
# FRESH cargo target volume, so every run compiled the whole workspace
# from cold AND re-downloaded the crates.io index. That, plus a
# memory-starved host, is what turned a 48-second verification into a
# 20-minute one.
#
# What this fixes:
#   * one persistent volume for CARGO_HOME (registry cache) and the
#     target directory, so both survive between runs;
#   * CARGO_TARGET_DIR points INSIDE the volume, so the container never
#     touches the host's target/ and never contends for the host's
#     build lock;
#   * an image with clippy baked in (scripts/docker/checker.Dockerfile),
#     so there is no per-run rustup download.
#
# Usage:  bash scripts/docker-cargo-check.sh [cargo args...]
#   default: clippy --workspace --all-targets -- -D warnings
#   e.g.    bash scripts/docker-cargo-check.sh test --workspace
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
VOLUME="clavus-cargo"
IMAGE="clavus-rust-check:latest"

if [ "$#" -gt 0 ]; then
  CARGO_ARGS=("$@")
else
  CARGO_ARGS=(clippy --workspace --all-targets -- -D warnings)
fi

# The container must NOT run as root. Three tests depend on permission
# bits — they make a directory read-only and expect the write to fail.
# Root ignores the permission bits, so the relay starts happily and one
# test waits forever (the suite HANGS instead of failing) while the
# other two fail. CI runs as a normal user, so it keeps all three; the
# container has to as well, or it is not testing the same thing.
HOST_UID="$(id -u)"
HOST_GID="$(id -g)"

# Build the checker image if it is not there. Docker caches the layer,
# so this is a no-op after the first run.
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "── building the checker image ──"
  docker build -t "$IMAGE" -f "$REPO/scripts/docker/checker.Dockerfile" "$REPO/scripts/docker"
fi

echo "── Linux check in Docker ───"
echo "   repo:   $REPO"
echo "   volume: $VOLUME (persistent — registry cache and build artifacts survive)"
echo "   args:   ${CARGO_ARGS[*]}"
echo "   user:   $HOST_UID:$HOST_GID (not root — three tests need the permission bits to bite)"
echo ""

# A container from an interrupted run still holds the name, and Docker
# refuses to reuse it. Clear it out first.
docker rm -f clavus-cargo-check >/dev/null 2>&1 || true

# The volume was created by an earlier root run, so it belongs to root.
# Hand it to the user the container will run as.
docker run --rm -v "$VOLUME":/cargo "$IMAGE" \
  chown -R "$HOST_UID:$HOST_GID" /cargo >/dev/null 2>&1 || true

# The container needs a home directory that is NOT the temporary
# directory. A project outside the home keeps the last two folders of
# its path as its identity, so with HOME=/tmp every temporary project
# would look like it lived inside the home — each one with a different
# path — and two machines would no longer agree on what a project is.
# A home of its own keeps that rule working.
DOCKER_HOME="/tmp/clavus-docker-home"
mkdir -p "$DOCKER_HOME"

docker run --rm \
  --name clavus-cargo-check \
  --user "$HOST_UID:$HOST_GID" \
  -e HOME=/home \
  -v "$REPO":/w \
  -v "$VOLUME":/cargo \
  -v "$DOCKER_HOME":/home \
  -w /w/rust \
  -e CARGO_HOME=/cargo/home \
  -e CARGO_TARGET_DIR=/cargo/target \
  "$IMAGE" \
  bash -c '
    set -e
    export PATH="/usr/local/cargo/bin:$PATH"
    echo "── cargo --version ──"
    cargo --version
    rustc --version
    cargo clippy --version
    echo ""
    echo "── running: cargo $* ──"
    started=$SECONDS
    cargo "$@"
    echo ""
    echo "── finished in $((SECONDS - started))s ──"
  ' _ "${CARGO_ARGS[@]}"
