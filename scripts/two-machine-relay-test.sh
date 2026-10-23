#!/usr/bin/env bash
#
# A real two-machine relay test.
#
# Three Linux machines in containers, one relay between them:
#
#   A  the studio       snapshots, adds a note, pushes
#   B  a collaborator   the same set at a different path and depth;
#                       pulls, restores, and ends up with A's bytes
#   C  a collaborator   the set and no audio at all; pulls, and the
#                       audio has to come over the wire
#
# The clients are the release binary built for Linux, so this also
# proves the musl build works — on the real target, over real TCP,
# with real bytes.
#
# Needs Docker. Nothing is written outside a temporary directory.

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${CLAVUS_RELAY_TEST_DIR:-$(mktemp -d -t clavus-relay-XXXXXX)}"
NET="clavus-relay-test-$$"
RELAY="clavus-relay-$$"
IMAGE="${CLAVUS_TEST_IMAGE:-alpine:3.20}"
PLATFORM="${CLAVUS_TEST_PLATFORM:-}"
TARGET_DEFAULT="x86_64-unknown-linux-musl"

say()  { printf '\n\033[1;32m▸\033[0m %s\n' "$1"; }
ok()   { printf '  \033[1;32m✓\033[0m %s\n' "$1"; }
bad()  { printf '  \033[1;31m✗\033[0m %s\n' "$1"; FAILURES=$((FAILURES + 1)); }
note() { printf '    %s\n' "$1"; }

FAILURES=0
cleanup() {
    docker rm -f "$RELAY" >/dev/null 2>&1
    docker network rm "$NET" >/dev/null 2>&1
}
trap cleanup EXIT

command -v docker >/dev/null || { echo "docker is needed for this test"; exit 1; }
docker info >/dev/null 2>&1 || { echo "docker is not running"; exit 1; }

# ── The binary ─────────────────────────────────────────────────────
say "Building the Linux binary"
BIN="$WORK/clavus"
if [ -n "${CLAVUS_TEST_BIN:-}" ]; then
    cp "$CLAVUS_TEST_BIN" "$BIN"
    note "using $CLAVUS_TEST_BIN"
else
    TARGET="${CLAVUS_TEST_TARGET:-$TARGET_DEFAULT}"
    # The binary has to match the machine it runs on. On an Apple
    # Silicon Mac the Linux target is still arm64, but the container
    # has to be told, or Docker picks the host's own architecture and
    # the binary will not run.
    if [ -z "$PLATFORM" ]; then
        case "$TARGET" in
            x86_64-*)  PLATFORM="linux/amd64" ;;
            aarch64-*) PLATFORM="linux/arm64" ;;
        esac
    fi
    ( cd "$ROOT/rust" && cargo build --release --target "$TARGET" ) || exit 1
    cp "$ROOT/rust/target/$TARGET/release/clavus" "$BIN"
    note "built $TARGET"
fi
chmod +x "$BIN"
note "$(file -b "$BIN" | cut -c1-70)"

# ── The fixture ────────────────────────────────────────────────────
say "Making the fixture"
rm -rf "$WORK/machines"
python3 "$ROOT/fixtures/gen_relay_fixture.py" "$WORK/machines" || exit 1
chmod -R a+rwX "$WORK/machines"

docker network create "$NET" >/dev/null 2>&1

# `clavus` inside a container.
run() {
    local machine=$1; shift
    local platform_arg=""
    [ -n "$PLATFORM" ] && platform_arg="--platform $PLATFORM"
    # shellcheck disable=SC2086
    docker run --rm $platform_arg --network "$NET" \
        -v "$WORK/machines:/w" -v "$WORK:/t:ro" -w "/w/$machine" \
        "$IMAGE" /t/clavus "$@" 2>&1
}

# ── The relay ──────────────────────────────────────────────────────
say "Starting the relay"
mkdir -p "$WORK/relaystate" && chmod -R a+rwX "$WORK/relaystate"
RELAY_PLATFORM=""
[ -n "$PLATFORM" ] && RELAY_PLATFORM="--platform $PLATFORM"
# shellcheck disable=SC2086
docker run -d --name "$RELAY" $RELAY_PLATFORM --network "$NET" \
    -v "$WORK:/t:ro" -v "$WORK/relaystate:/relay" \
    "$IMAGE" /t/clavus relay --bind 0.0.0.0:4477 --state-dir /relay --data /relay/store >/dev/null
sleep 2
RELAY_LOG=$(docker logs "$RELAY" 2>&1)
if ! echo "$RELAY_LOG" | grep -q "Listening"; then
    bad "the relay did not start"
    docker logs "$RELAY" 2>&1 | tail -5
    exit 1
fi
ok "the relay is listening"

A_SET="/w/machineA/Set/Set Project/Set.als"
B_SET="/w/machineB/Some/Other/Place/Set.als"
C_SET="/w/machineC/Work/Set/Set.als"

# ── A: snapshot, note, push ────────────────────────────────────────
say "Machine A — snapshot, note, push"
FIRST=$(run machineA --data /w/machineA/.clavus snapshot "$A_SET" -m "first take" \
        | grep -oE "snapshot [0-9a-f]{8}" | head -1 | cut -d" " -f2)
[ -n "$FIRST" ] && ok "snapshot taken ($FIRST)" || bad "the snapshot failed"
CUE=$(run machineA --data /w/machineA/.clavus cue "$A_SET" --text "sidechain the pad" \
      --position 1.1.1 --author chris)
if echo "$CUE" | grep -q "sidechain"; then ok "note added"; else bad "the note failed"; note "$CUE"; fi

# A second take, so the chain has a parent.
python3 - "$WORK/machines/machineA/Set/Set Project/Set.als" <<'PY'
import gzip, pathlib, sys
p = pathlib.Path(sys.argv[1])
t = gzip.decompress(p.read_bytes()).decode().replace("Drums v1", "Drums v2")
p.write_bytes(gzip.compress(t.encode(), compresslevel=9, mtime=0))
PY
SECOND=$(run machineA --data /w/machineA/.clavus snapshot "$A_SET" -m "second take" \
         | grep -oE "snapshot [0-9a-f]{8}" | head -1 | cut -d" " -f2)
[ -n "$SECOND" ] && ok "a second take ($SECOND)" || bad "the second snapshot failed"

PUSH=$(run machineA --data /w/machineA/.clavus push --relay "$RELAY:4477" "$A_SET")
echo "$PUSH" | grep -q "Pushed the local chain" && ok "pushed" || { bad "the push failed"; echo "$PUSH"; }
echo "$PUSH" | grep -q "sent the audio" && ok "the audio was sent" || bad "the audio was not sent"
echo "$PUSH" | grep -q "1 cue" && ok "the note was sent" || bad "the note was not sent"

# ── B: pull and restore ────────────────────────────────────────────
say "Machine B — a collaborator, same set, different path"
PULL=$(run machineB --data /w/machineB/.clavus pull --relay "$RELAY:4477" "$B_SET")
echo "$PULL" | grep -q "Pulled 2 new snapshot" && ok "pulled two snapshots" || { bad "the pull failed"; echo "$PULL"; }
echo "$PULL" | grep -q "Imported 1 new cue" && ok "got the note" || bad "the note did not come"
echo "$PULL" | grep -q "can be restored here" && ok "the project bytes came with it" || bad "the project bytes did not come"

CHAIN=$(run machineB --data /w/machineB/.clavus log "$B_SET")
if echo "$CHAIN" | grep -q "second take"; then
    ok "the chain is there, with its parent"
else
    bad "the chain is wrong"; note "$CHAIN"
fi

RESTORE=$(run machineB --data /w/machineB/.clavus restore "$B_SET" "$SECOND" 2>&1)
note "$(echo "$RESTORE" | head -2 | tr '\n' ' ')"

# The bytes must now be A's.
A_HASH=$(shasum -a 256 "$WORK/machines/machineA/Set/Set Project/Set.als" | cut -d' ' -f1)
B_HASH=$(shasum -a 256 "$WORK/machines/machineB/Some/Other/Place/Set.als" | cut -d' ' -f1)
[ "$A_HASH" = "$B_HASH" ] && ok "the restored set is byte-for-byte A's" \
    || bad "the restored set differs from A's ($A_HASH vs $B_HASH)"

# ── C: pull the audio ──────────────────────────────────────────────
say "Machine C — a collaborator with no audio at all"
AUDIT_C=$(run machineC --data /w/machineC/.clavus audit "$C_SET")
if echo "$AUDIT_C" | grep -q "missing"; then
    ok "the audio is missing, as it should be"
else
    bad "the audio was already there"; note "$AUDIT_C"
fi

PULL_C=$(run machineC --data /w/machineC/.clavus pull --relay "$RELAY:4477" "$C_SET")
echo "$PULL_C" | grep -q "Pulled" && ok "pulled" || { bad "the pull failed"; echo "$PULL_C"; }
RESTORE_C=$(run machineC --data /w/machineC/.clavus restore "$C_SET" "$FIRST" 2>&1)
if echo "$RESTORE_C" | grep -qi "sample"; then
    ok "the audio was restored"
else
    bad "the audio was not restored"; note "$RESTORE_C"
fi

A_AUDIO=$(shasum -a 256 "$WORK/machines/machineA/Set/Set Project/Samples/loop.wav" | cut -d' ' -f1)
C_AUDIO=$(shasum -a 256 "$WORK/machines/machineC/Work/Set/Samples/loop.wav" 2>/dev/null | cut -d' ' -f1)
[ -n "$C_AUDIO" ] && [ "$A_AUDIO" = "$C_AUDIO" ] && ok "the audio is byte-for-byte A's" \
    || bad "the audio differs or is absent"

# ── The result ─────────────────────────────────────────────────────
echo
if [ "$FAILURES" -eq 0 ]; then
    printf '\033[1;32m── the two-machine relay test passed ──\033[0m\n'
    exit 0
fi
printf '\033[1;31m── %s check(s) failed ──\033[0m\n' "$FAILURES"
exit 1
