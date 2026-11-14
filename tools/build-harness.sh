#!/bin/sh
# SPDX-License-Identifier: MIT
# Build the fuzz harnesses (differential decoder harness, pack reader harness, waveform harness) into out/: sh tools/build-harness.sh [extra cc flags, e.g. -fsanitize=address,undefined]
# Environment: CC (default cc), HARNESS_STRICT=0 to build against the unmodified oracle,
# SAMPLEPACK_LIB to use a library built elsewhere (no cargo run), LDLIBS for extra link libraries.
set -e
cd "$(dirname "$0")/.."
mkdir -p out
[ -n "$SAMPLEPACK_LIB" ] || cargo build --release --locked -p samplepack-capi
strict=-DFSD_STRICT_UTF8; [ "${HARNESS_STRICT:-1}" = 0 ] && strict=
LIB=${SAMPLEPACK_LIB:-target/release/libsamplepack_capi.a}
${CC:-cc} -O1 -g -std=gnu11 -Wall -Wextra $strict "$@" -Iinclude fuzz/diff.c "$LIB" -o out/diff $LDLIBS
${CC:-cc} -O1 -g -std=gnu11 -Wall -Wextra "$@" -Iinclude fuzz/pack.c "$LIB" -o out/packfuzz $LDLIBS
${CC:-cc} -O1 -g -std=gnu11 -Wall -Wextra "$@" -Iinclude fuzz/waveform.c "$LIB" -o out/wavefuzz $LDLIBS
echo built out/diff out/packfuzz out/wavefuzz
