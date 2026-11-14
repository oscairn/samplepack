#!/bin/sh
# SPDX-License-Identifier: MIT
# Runs inside the aarch64 container (see gate.sh): /w is the repository (read-only), /o the gate output directory.
set -e
cd /tmp
gcc -O3 -mcpu="$1" -I/w/include -o bench /w/bench/bench.c /o/libsamplepack_capi.a
run() {
    impl=$1; crc=$2; shift 2
    ir=$(valgrind --tool=callgrind --collect-atstart=no --toggle-collect=decode_all_$impl --callgrind-out-file=/tmp/cg /tmp/bench $impl $crc "$@" 2>&1 >/tmp/samples | awk '/Collected/ {print $4}')
    samples=$(awk '{print $2}' /tmp/samples)
    echo "$impl $crc $ir $samples" | awk '{printf "  %-6s %-5s Ir=%-10s samples=%-8s ips=%.3f\n", $1, $2, $3, $4, $3/$4}'
}
group() {
    name=$1; shift; [ $# -gt 0 ] || return 0; echo "== $name ($# files)"
    for crc in nocrc crc; do run oracle $crc "$@"; run rust $crc "$@"; done
}
cd /o/streams
group all *.flac
for l in 0 4 8; do group "-l $l" $(ls | grep -- "-l$l-"); done
group mono $(ls | grep -- "-mono-")
for g in sweep noise colored correlated clipped; do group "$g stereo" $(ls | grep "^$g-st-"); done
