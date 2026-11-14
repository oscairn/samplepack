#!/bin/sh
# SPDX-License-Identifier: MIT
# Fuzz runs with ASan and UBSan on the C side: the decoder against the C oracle, then the pack
# reader on mutated, truncated and spliced packs (seed packs in fuzz/corpus, made with
# `samplepack pack` from the synthetic corpus), then the waveform reader on mutated waveforms that
# the harness builds itself:
#   tools/run-fuzz.sh [cases [seed [pack-cases]]]   default 200000 cases, seed 1, pack cases = cases
# Exits non-zero on any divergence or crash. Divergent inputs are written to divergence-N.bin in the
# current directory.
set -e
cd "$(dirname "$0")/.."
sh tools/build-harness.sh -fsanitize=address,undefined
out/diff files tests/data/*.flac
out/diff fuzz "${1:-200000}" "${2:-1}" tests/data/*.flac
out/packfuzz pack "${3:-${1:-200000}}" "${2:-1}" fuzz/corpus/*.osp
out/wavefuzz waveform "${3:-${1:-200000}}" "${2:-1}"
