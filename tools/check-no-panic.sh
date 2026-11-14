#!/bin/sh
# SPDX-License-Identifier: MIT
# Panic-freedom checks on the release static library:
#  1. no panic/bounds-check/unwrap machinery in the archive's symbol table;
#  2. a build whose panic handler calls a non-existent symbol still links into a program, because
#     dead-code removal leaves no path to the handler.
set -e
cd "$(dirname "$0")/.."
T=${TMPDIR:-/tmp}/samplepack-nopanic.$$; mkdir -p "$T"; trap 'rm -rf "$T"' EXIT
cargo build --release --locked -p samplepack-capi
if nm target/release/libsamplepack_capi.a 2>/dev/null | grep -iE 'panic|unwrap_failed|expect_failed|slice_index|bounds_check|slice_end_index|unreachable_unchecked'; then
    echo "FAIL: panic machinery in the archive"; exit 1
fi
echo "ok: no panic symbols in libsamplepack_capi.a"
cargo build --release --locked -p samplepack-capi --features link-fail-on-panic --target-dir "$T/t"
case $(uname) in Darwin) gc=-Wl,-dead_strip;; *) gc="-Wl,--gc-sections -ffunction-sections";; esac
cat > "$T/m.c" <<'EOC'
#include "samplepack.h"
#include <stdio.h>
static sp_flac_scratch s; static int16_t l[4096], r[4096]; static uint8_t d[64];
int main(void) {
    sp_flac_frame_info i; sp_flac_frame_entry t[4]; sp_flac_scan_summary u; uint64_t o;
    int st = sp_flac_decode_frame(d, sizeof d, 1, &s, l, r, 4096, &i);
    st += sp_flac_scan(d, sizeof d, 1, &s, t, 4, &u);
    st += sp_flac_first_frame(d, sizeof d, &o);
    printf("%d\n", st); return 0;
}
EOC
${CC:-cc} -O1 $gc -Iinclude "$T/m.c" "$T/t/release/libsamplepack_capi.a" -o "$T/m"
"$T/m"
echo "ok: program links against the link-fail-on-panic build, so no panic path is reachable"
