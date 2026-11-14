#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build the differential harness into out/: sh tools/build-harness.sh [extra cc flags, e.g. -fsanitize=address,undefined]
# Environment: CC (default cc), HARNESS_STRICT=0 to build against the unmodified oracle.
set -e
cd "$(dirname "$0")/.."
mkdir -p out
cargo build --release --locked -p samplepack-capi
strict=-DFSD_STRICT_UTF8; [ "${HARNESS_STRICT:-1}" = 0 ] && strict=
${CC:-cc} -O1 -g -std=gnu11 -Wall -Wextra $strict "$@" -Iinclude fuzz/diff.c target/release/libsamplepack_capi.a -o out/diff
echo built out/diff
