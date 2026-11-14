#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Instruction-count gate on aarch64: callgrind Ir per decoded sample, Rust library vs the C oracle,
# on the same streams. The Rust static library is cross-compiled on the host for
# aarch64-unknown-linux-gnu (no linker needed for a staticlib); the bench is linked and run in a
# throwaway aarch64 Linux container that has gcc and valgrind.
#   tools/gate.sh [files...]   (default: the committed corpus, order <= 8 streams)
# Environment: GATE_IMAGE (default analog-c-img); GATE_CPU (default cortex-a53).
set -e
cd "$(dirname "$0")/.."
CPU=${GATE_CPU:-cortex-a53}
OUT=$PWD/out/gate; mkdir -p "$OUT"
CARGO_ENCODED_RUSTFLAGS=$(printf "%s\037%s\037%s\037%s" -C "target-cpu=$CPU" --remap-path-prefix "$PWD=/src") cargo build --release --locked --target aarch64-unknown-linux-gnu -p samplepack-capi --target-dir "$OUT/target"
cp "$OUT/target/aarch64-unknown-linux-gnu/release/libsamplepack_capi.a" "$OUT/"
if [ $# -gt 0 ]; then FILES="$*"; else FILES=$(ls tests/data/*.flac | grep -v -- '-l12-'); fi
mkdir -p "$OUT/streams"; rm -f "$OUT/streams"/*
for f in $FILES; do cp "$f" "$OUT/streams/"; done
docker run --rm -v "$PWD:/w:ro" -v "$OUT:/o" "${GATE_IMAGE:-analog-c-img}" sh /w/tools/gate-inner.sh "$CPU"
