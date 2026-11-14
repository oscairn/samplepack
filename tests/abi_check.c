/* SPDX-License-Identifier: AGPL-3.0-or-later */
/* Built and run by tests/c_abi.rs against the static library: checks that include/samplepack.h
 * agrees with the Rust definitions (EXPECT_* values come from the Rust types), then makes a few calls. */
#include <stdio.h>
#include <stdlib.h>
#include <stddef.h>
#include "samplepack.h"
#include "expect.h"

_Static_assert(sizeof(sp_flac_scratch) == EXPECT_SCRATCH_SIZE, "scratch size");
_Static_assert(_Alignof(sp_flac_scratch) == EXPECT_SCRATCH_ALIGN, "scratch align");
_Static_assert(sizeof(sp_flac_frame_info) == EXPECT_INFO_SIZE, "info size");
_Static_assert(_Alignof(sp_flac_frame_info) == EXPECT_INFO_ALIGN, "info align");
_Static_assert(offsetof(sp_flac_frame_info, samples) == EXPECT_INFO_SAMPLES, "info.samples");
_Static_assert(offsetof(sp_flac_frame_info, channels) == EXPECT_INFO_CHANNELS, "info.channels");
_Static_assert(offsetof(sp_flac_frame_info, consumed) == EXPECT_INFO_CONSUMED, "info.consumed");
_Static_assert(offsetof(sp_flac_frame_info, reserved) == EXPECT_INFO_RESERVED, "info.reserved");
_Static_assert(sizeof(sp_flac_frame_entry) == EXPECT_ENTRY_SIZE, "entry size");
_Static_assert(_Alignof(sp_flac_frame_entry) == EXPECT_ENTRY_ALIGN, "entry align");
_Static_assert(offsetof(sp_flac_frame_entry, offset) == EXPECT_ENTRY_OFFSET, "entry.offset");
_Static_assert(offsetof(sp_flac_frame_entry, length) == EXPECT_ENTRY_LENGTH, "entry.length");
_Static_assert(offsetof(sp_flac_frame_entry, samples) == EXPECT_ENTRY_SAMPLES, "entry.samples");
_Static_assert(sizeof(sp_flac_scan_summary) == 24, "summary size");
_Static_assert(offsetof(sp_flac_scan_summary, consumed) == 8, "summary.consumed");
_Static_assert(offsetof(sp_flac_scan_summary, total_samples) == 16, "summary.total_samples");
_Static_assert(SP_FLAC_MAX_BLOCK == EXPECT_MAX_BLOCK, "max block");
_Static_assert(SP_FLAC_MAX_LPC_ORDER == EXPECT_MAX_LPC, "max lpc");
_Static_assert(SP_FLAC_CHECK_CRC == EXPECT_FLAG_CRC, "crc flag");
EXPECT_ENUM_ASSERTS

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *f = fopen(argv[1], "rb");
    if (!f) return 2;
    static uint8_t buf[1 << 20];
    size_t len = fread(buf, 1, sizeof buf, f);
    fclose(f);
    static sp_flac_scratch scratch;
    static int16_t l[SP_FLAC_MAX_BLOCK], r[SP_FLAC_MAX_BLOCK];
    sp_flac_frame_info info;
    uint64_t first = 0;
    if (sp_flac_first_frame(buf, len, &first) != SP_OK) return 3;
    if (sp_flac_first_frame(buf, len, NULL) != SP_ERR_INVALID_ARGUMENT) return 4;
    if (sp_flac_decode_frame(buf + first, len - first, SP_FLAC_CHECK_CRC, &scratch, l, r, SP_FLAC_MAX_BLOCK, &info) != SP_OK) return 5;
    if (info.samples != 1024 || info.channels != 2 || info.reserved != 0 || info.consumed == 0) return 6;
    if (sp_flac_decode_frame(buf + first, len - first, 0, &scratch, l, r, 1023, &info) != SP_ERR_BUFFER_TOO_SMALL || info.samples != 0) return 7;
    if (sp_flac_decode_frame(buf + first, len - first, 0, &scratch, l, l, SP_FLAC_MAX_BLOCK, &info) != SP_ERR_INVALID_ARGUMENT) return 8;
    if (sp_flac_decode_frame(buf + first, len - first, 0, NULL, l, r, SP_FLAC_MAX_BLOCK, &info) != SP_ERR_INVALID_ARGUMENT) return 9;
    if (sp_flac_decode_frame(buf + first, len - first, 0, &scratch, l, r, SP_FLAC_MAX_BLOCK, NULL) != SP_ERR_INVALID_ARGUMENT) return 10;
    if (sp_flac_decode_frame(NULL, 0, 0, &scratch, l, r, SP_FLAC_MAX_BLOCK, &info) != SP_ERR_TRUNCATED) return 11;
    if (sp_flac_decode_frame(buf + first, len - first, 0x80, &scratch, l, r, SP_FLAC_MAX_BLOCK, &info) != SP_ERR_INVALID_ARGUMENT) return 12;
    sp_flac_frame_entry table[64];
    sp_flac_scan_summary sum;
    if (sp_flac_scan(buf + first, len - first, SP_FLAC_CHECK_CRC, &scratch, table, 64, &sum) != SP_OK) return 13;
    if (sum.frames != 5 || sum.consumed != len - first || sum.total_samples != 5000 || table[0].offset != 0 || table[0].samples != 1024) return 14;
    if (sp_flac_scan(buf + first, len - first, 0, &scratch, table, 2, &sum) != SP_ERR_BUFFER_TOO_SMALL || sum.frames != 2) return 15;
    printf("abi ok\n");
    return 0;
}
