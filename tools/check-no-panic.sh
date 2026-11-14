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
static sp_flac_scratch s; static sp_verify_scratch v; static int16_t l[4096], r[4096]; static uint8_t d[8192];
int main(void) {
    sp_flac_frame_info i; sp_flac_frame_entry t[4]; sp_flac_scan_summary u; uint64_t o;
    sp_pack_info pi; sp_index_entry ix[4] = {0}; sp_record_info ri; size_t n; uint8_t id[32] = {0};
    const uint8_t *p1, *p2; size_t z1, z2;
    int st = sp_flac_decode_frame(d, sizeof d, 1, &s, l, r, 4096, &i);
    st += sp_flac_scan(d, sizeof d, 1, &s, t, 4, &u);
    st += sp_flac_first_frame(d, sizeof d, &o);
    st += sp_pack_open(d, sizeof d, &pi);
    st += sp_pack_recover(d, sizeof d, &o);
    st += sp_pack_index(d, sizeof d, ix, 4, &n);
    st += sp_pack_rebuild_index(d, sizeof d, ix, 4, &n);
    st += sp_index_find(ix, 4, id, &n);
    st += sp_pack_record(d, sizeof d, ix, &ri);
    st += sp_record_window(d, sizeof d, ix, &p1, &z1);
    st += sp_record_frames(d, sizeof d, ix, &p1, &z1, &p2, &z2);
    st += sp_record_decode(d, sizeof d, ix, 0, 1, 0, &s, l, r, 4096, &o);
    st += sp_sample_id_pcm(44100, 2, l, 2048, id);
    st += sp_record_sample_id(d, sizeof d, ix, &v, id);
    st += sp_record_verify(d, sizeof d, ix, &v);
    st += sp_pack_verify(d, sizeof d, &v, ix, 4);
    sp_key_bank kb[2] = {{0}}; uint8_t ids[2][32] = {{0}};
    st += sp_waveform_build(kb, 2, ids, 2, d, sizeof d, &n);
    st += sp_waveform_check(d, sizeof d, &n, &n);
    st += sp_waveform_sample(d, sizeof d, 0, id);
    st += sp_waveform_bank(d, sizeof d, 0, kb);
    st += sp_waveform_id(d, sizeof d, id);
    printf("%d\n", st); return 0;
}
EOC
${CC:-cc} -O1 $gc -Iinclude "$T/m.c" "$T/t/release/libsamplepack_capi.a" -o "$T/m"
"$T/m"
echo "ok: program links against the link-fail-on-panic build, so no panic path is reachable"
