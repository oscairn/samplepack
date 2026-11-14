#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Differential fuzz run against the C oracle, with ASan and UBSan on the C side:
#   tools/run-fuzz.sh [cases [seed]]      default 200000 cases, seed 1
# Exits non-zero on any divergence or crash. Divergent inputs are written to divergence-N.bin in the
# current directory.
set -e
cd "$(dirname "$0")/.."
sh tools/build-harness.sh -fsanitize=address,undefined
out/diff files tests/data/*.flac
out/diff fuzz "${1:-200000}" "${2:-1}" tests/data/*.flac
