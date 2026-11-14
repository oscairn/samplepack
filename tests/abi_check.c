/* SPDX-License-Identifier: MIT */
/* Built and run by tests/c_abi.rs against the static library: checks that include/samplepack.h
 * agrees with the Rust definitions (EXPECT_* values come from the Rust types), then makes a few calls. */
#include <stdio.h>
#include <stdlib.h>
#include <stddef.h>
#include <string.h>
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

static uint8_t *slurp(const char *p, size_t *n) {
    FILE *f = fopen(p, "rb"); if (!f) return NULL;
    fseek(f, 0, SEEK_END); *n = (size_t)ftell(f); fseek(f, 0, SEEK_SET);
    uint8_t *d = malloc(*n ? *n : 1); if (fread(d, 1, *n, f) != *n) { free(d); d = NULL; }
    fclose(f); return d;
}

static int pack_calls(const char *pack_path, const char *pcm_path);
static int waveform_calls(void);

int main(int argc, char **argv) {
    if (argc != 4) return 2;
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
    int rc = pack_calls(argv[2], argv[3]);
    if (rc) return rc;
    rc = waveform_calls();
    if (rc) return rc;
    printf("abi ok\n");
    return 0;
}

/* The pack file holds a stereo record (the sweep corpus stream, 5000 frames, window 4096) and a mono one. */
static int pack_calls(const char *pack_path, const char *pcm_path) {
    size_t len = 0, pcm_len = 0;
    uint8_t *p = slurp(pack_path, &len), *pcm = slurp(pcm_path, &pcm_len);
    if (!p || !pcm) return 20;
    sp_pack_info pi; size_t n = 0, pos = 9; uint64_t committed = 0;
    static sp_verify_scratch vs; static sp_flac_scratch fs; static int16_t l[8192], r[8192];
    sp_index_entry idx[4]; sp_record_info ri; uint8_t id[32], id2[32];
    if (sp_pack_open(p, len, &pi) != SP_OK || pi.records != 2 || pi.length != len) return 21;
    if (sp_pack_open(p, len - 1, &pi) != SP_ERR_NO_FOOTER || pi.records != 0) return 22;
    if (sp_pack_recover(p, len - 1, &committed) != SP_OK || committed != 4096) return 23;
    if (sp_pack_index(p, len, idx, 1, &n) != SP_ERR_BUFFER_TOO_SMALL || n != 2) return 24;
    if (sp_pack_index(p, len, idx, 4, &n) != SP_OK || n != 2) return 25;
    if (sp_pack_rebuild_index(p, len, idx + 2, 2, &n) != SP_OK || n != 2 || memcmp(idx, idx + 2, 2 * sizeof idx[0])) return 26;
    if (sp_sample_id_pcm(44100, 2, (const int16_t *)pcm, pcm_len / 4, id) != SP_OK) return 27;
    if (sp_index_find(idx, 2, id, &pos) != SP_OK || pos > 1) return 28;
    if (sp_pack_record(p, len, &idx[pos], &ri) != SP_OK || ri.frames != 5000 || ri.channels != 2 || ri.window_frames != 4096 || ri.credit_len != 8) return 29;
    if (memcmp(p + ri.offset + ri.credit_offset, "abi test", 8)) return 30;
    const uint8_t *w = NULL, *fr = NULL, *tb = NULL; size_t wb = 0, fl = 0, te = 0;
    if (sp_record_window(p, len, &idx[pos], &w, &wb) != SP_OK || wb != 4096 * 4 || (size_t)(w - p) % SP_PACK_PAGE || memcmp(w, pcm, wb)) return 31;
    if (sp_record_frames(p, len, &idx[pos], &fr, &fl, &tb, &te) != SP_OK || te != ri.blocks + 1 || fl != ri.frame_bytes) return 32;
    uint64_t got = 0;
    if (sp_record_decode(p, len, &idx[pos], 0, ri.blocks, 0, &fs, l, r, 8192, &got) != SP_OK || got != 5000) return 33;
    for (int i = 0; i < 5000; i++) { int16_t a, b; memcpy(&a, pcm + 4 * i, 2); memcpy(&b, pcm + 4 * i + 2, 2); if (l[i] != a || r[i] != b) return 34; }
    if (sp_record_decode(p, len, &idx[pos], 4, 2, 0, &fs, l, r, 8192, &got) != SP_ERR_INVALID_ARGUMENT) return 35;
    if (sp_record_decode(p, len, &idx[pos], 0, 2, 0, &fs, l, r, 100, &got) != SP_ERR_BUFFER_TOO_SMALL) return 36;
    if (sp_record_sample_id(p, len, &idx[pos], &vs, id2) != SP_OK || memcmp(id, id2, 32)) return 37;
    if (sp_record_verify(p, len, &idx[pos], &vs) != SP_OK) return 38;
    if (sp_pack_verify(p, len, &vs, idx, 4) != SP_OK) return 39;
    id[0] ^= 1;
    if (sp_index_find(idx, 2, id, &pos) != SP_ERR_NOT_FOUND) return 40;
    if (sp_pack_open(NULL, 5, &pi) != SP_ERR_INVALID_ARGUMENT || sp_pack_open(p, len, NULL) != SP_ERR_INVALID_ARGUMENT) return 41;
    sp_index_entry bad = idx[pos]; bad.preload += SP_PACK_PAGE;
    if (sp_record_window(p, len, &bad, &w, &wb) != SP_ERR_MISMATCH || w || wb) return 42;
    p[4096 + 40] ^= 1;  /* inside the first record's sample id: its header checksum fails */
    if (sp_pack_record(p, len, &idx[0], &ri) != SP_ERR_BAD_CHECKSUM && sp_pack_record(p, len, &idx[1], &ri) != SP_ERR_BAD_CHECKSUM) return 43;
    free(p); free(pcm);
    return 0;
}

/* The worked example of docs/waveform.md, built from its key banks in the order given there. */
static int waveform_calls(void) {
    static const int16_t pcm[4] = { 0, 1, -1, 32767 };
    uint8_t ids[2][32], id[32]; char hex[65];
    if (sp_sample_id_pcm(44100, 1, pcm, 4, ids[0]) != SP_OK || sp_sample_id_pcm(48000, 1, pcm, 4, ids[1]) != SP_OK) return 50;
    sp_key_bank b[3] = {
        { .key_lo = 60, .key_hi = 127, .vel_lo = 1, .vel_hi = 127, .root = 72, .loop_mode = 1, .sample = 1, .tune = -1250, .level = -600, .loop_start = 100, .loop_end = 2000 },
        { .key_hi = 59, .vel_lo = 1, .vel_hi = 127, .root = 48, .pan = -10000 },
        { .key_hi = 59, .vel_lo = 1, .vel_hi = 127, .root = 48, .sample = 1, .pan = 10000 },
    };
    uint8_t out[256]; size_t n = 0, s = 0, k = 0;
    if (sp_waveform_build(b, 3, ids, 2, out, 100, &n) != SP_ERR_BUFFER_TOO_SMALL || n != 204) return 51;
    if (sp_waveform_build(b, 3, ids, 2, out, sizeof out, &n) != SP_OK || n != 204) return 52;
    if (sp_waveform_check(out, n, &s, &k) != SP_OK || s != 2 || k != 3) return 53;
    if (sp_waveform_sample(out, n, 1, id) != SP_OK || memcmp(id, ids[1], 32)) return 54;
    if (sp_waveform_sample(out, n, 2, id) != SP_ERR_INVALID_ARGUMENT) return 55;
    sp_key_bank got;
    if (sp_waveform_bank(out, n, 2, &got) != SP_OK || got.key_lo != 60 || got.tune != -1250 || got.level != -600 || got.loop_end != 2000 || got.sample != 1) return 56;
    if (sp_waveform_bank(out, n, 3, &got) != SP_ERR_INVALID_ARGUMENT) return 57;
    if (sp_waveform_id(out, n, id) != SP_OK) return 58;
    for (int i = 0; i < 32; i++) snprintf(hex + 2 * i, 3, "%02x", id[i]);
    if (strcmp(hex, "0ab2c142fc10f5f50f76c044e10942059087d87d02ff9acb6fa2edb298529e3f")) return 59;
    out[100] ^= 1;
    if (sp_waveform_check(out, 204, &s, &k) != SP_ERR_WF_CHECKSUM || s || k) return 62;
    b[2] = b[0];
    if (sp_waveform_build(b, 3, ids, 2, out, sizeof out, &n) != SP_ERR_WF_DUPLICATE_BANK || n != 0) return 60;
    if (sp_waveform_build(b, 3, ids, 2, out, sizeof out, NULL) != SP_ERR_INVALID_ARGUMENT) return 61;
    return 0;
}
