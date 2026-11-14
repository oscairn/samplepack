// SPDX-License-Identifier: MIT
// Waveform fuzz harness, through the C interface:
//
//   waveform N SEED      N cases: random waveforms built by sp_waveform_build, then mutated,
//                        truncated or extended, half of them with size and CRC made to agree
//
// Each case is mapped so that its last byte is directly followed by a PROT_NONE page; run under
// ASan/UBSan for the C side. Checks per case: no crash or abort; the constant-time accessors
// return an error or stay in bounds on any input; anything sp_waveform_check accepts is exactly
// what sp_waveform_build writes for its own key banks and sample ids (one canonical form), with
// the same id.
#define _DEFAULT_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include "samplepack.h"

static size_t pagesz;
static uint64_t rng_s;
static uint32_t rnd(void) { rng_s = rng_s * 6364136223846793005ull + 1442695040888963407ull; return (uint32_t)(rng_s >> 33); }

static uint32_t crc_tab[256];
static void crc_init(void) {
    for (uint32_t i = 0; i < 256; i++) { uint32_t c = i; for (int j = 0; j < 8; j++) c = c & 1 ? (c >> 1) ^ 0x82F63B78u : c >> 1; crc_tab[i] = c; }
}
static uint32_t crc32c(const uint8_t *d, size_t n) { uint32_t c = ~0u; while (n--) c = (c >> 8) ^ crc_tab[(c ^ *d++) & 255]; return ~c; }
static void wr32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }

static long cases, accepted, built, bad;
static long hist[128];
static void die(const char *what, size_t len) { printf("FAILURE: %s (len %zu)\n", what, len); bad++; }

static uint8_t ids[SP_WAVEFORM_MAX_SAMPLES][32];
static sp_key_bank banks[SP_WAVEFORM_MAX_BANKS];
static uint8_t again[SP_WAVEFORM_MAX_BYTES];

static void one(const uint8_t *src, size_t len) {
    size_t body = (len + pagesz - 1) / pagesz * pagesz + (len == 0 ? pagesz : 0), map = body + pagesz;
    uint8_t *base = mmap(NULL, map, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (base == MAP_FAILED || mprotect(base + body, pagesz, PROT_NONE)) { perror("mmap"); exit(1); }
    uint8_t *d = base + body - len;
    memcpy(d, src, len);
    mprotect(base, body, PROT_READ);
    cases++;
    size_t s = 0, k = 0, n = 0;
    uint8_t id[32], id2[32]; sp_key_bank b;
    sp_waveform_sample(d, len, rnd() % 8, id);   // constant-time accessors on unchecked bytes
    sp_waveform_bank(d, len, rnd() % 8, &b);
    sp_status st = sp_waveform_check(d, len, &s, &k);
    hist[st & 127]++;
    if (st == SP_OK) {
        accepted++;
        for (size_t i = 0; i < s; i++) if (sp_waveform_sample(d, len, i, ids[i]) != SP_OK) die("sample of a checked file", len);
        for (size_t i = 0; i < k; i++) if (sp_waveform_bank(d, len, i, &banks[i]) != SP_OK) die("bank of a checked file", len);
        if (sp_waveform_build(banks, k, ids, s, again, sizeof again, &n) != SP_OK || n != len || memcmp(again, d, len)) die("accepted file is not what build writes", len);
        if (sp_waveform_id(d, len, id) != SP_OK || sp_waveform_id(again, n, id2) != SP_OK || memcmp(id, id2, 32)) die("id", len);
    }
    munmap(base, map);
}

static int32_t pick(int32_t lo, int32_t hi) { return lo + (int32_t)(rnd() % (uint32_t)(hi - lo + 1)); }

// A random waveform written by sp_waveform_build into out; returns its length (0 if refused).
static size_t make(uint8_t *out) {
    size_t k = rnd() % 4 ? 1 + rnd() % 8 : 1 + rnd() % 200, np = 1 + rnd() % 6;
    for (size_t i = 0; i < np; i++) for (int j = 0; j < 32; j++) ids[i][j] = (uint8_t)(rnd() % 3);   // ids that share prefixes
    for (size_t i = 0; i < k; i++) {
        sp_key_bank *b = &banks[i]; memset(b, 0, sizeof *b);
        b->key_lo = (uint8_t)pick(0, 127); b->key_hi = (uint8_t)pick(b->key_lo, 127);
        b->vel_lo = (uint8_t)pick(1, 127); b->vel_hi = (uint8_t)pick(b->vel_lo, 127);
        b->root = (uint8_t)pick(0, 127); b->transpose = (int8_t)pick(-127, 127);
        b->sample = (uint16_t)(rnd() % np);
        b->tune = pick(-1200000, 1200000); b->level = pick(-14400, 4800); b->pan = (int16_t)pick(-10000, 10000);
        b->start = rnd() % 3 ? 0 : rnd() % 100; b->end = rnd() % 2 ? 0 : b->start + 2 + rnd() % 1000;
        if (rnd() % 2) { b->loop_mode = 1; b->loop_start = b->start; b->loop_end = b->end ? b->end : b->start + 1 + rnd() % 50; }
        if (rnd() % 8 == 0 && i) *b = banks[rnd() % i];   // an equal bank: build must refuse it
    }
    size_t len = 0;
    sp_status st = sp_waveform_build(banks, k, ids, np, out, SP_WAVEFORM_MAX_BYTES, &len);
    if (st != SP_OK && st != SP_ERR_WF_DUPLICATE_BANK) die("build refused valid banks", 0);
    return st == SP_OK ? len : 0;
}

int main(int argc, char **argv) {
    pagesz = (size_t)sysconf(_SC_PAGESIZE); crc_init();
    if (argc != 4 || strcmp(argv[1], "waveform")) { fprintf(stderr, "usage: waveform N SEED\n"); return 2; }
    long N = atol(argv[2]); rng_s = (uint64_t)atoll(argv[3]) * 2 + 1;
    static uint8_t buf[SP_WAVEFORM_MAX_BYTES + 64];
    for (long it = 0; it < N && bad < 5; it++) {
        size_t len = make(buf);
        if (!len) continue;
        built++;
        if (it % 4 == 0) { one(buf, len); continue; }   // unmodified
        for (int m = 1 + (int)(rnd() % 3); m > 0; m--) {
            size_t at = rnd() % len;
            switch (rnd() % 4) {
            case 0: buf[at] ^= (uint8_t)(1u << (rnd() % 8)); break;
            case 1: buf[at] = (uint8_t)rnd(); break;
            case 2: len = rnd() % (len + 1); if (!len) len = 1; break;
            case 3: { size_t add = rnd() % 64; memset(buf + len, (int)(rnd() & 255), add); len += add; } break;
            }
        }
        if (rnd() & 1 && len >= 16) { wr32(buf + 8, (uint32_t)len); wr32(buf + 12, crc32c(buf + 16, len - 16)); }
        one(buf, len);
    }
    printf("cases %ld: built %ld, accepted %ld, failures %ld\nsp_waveform_check status histogram:", cases, built, accepted, bad);
    for (int i = 0; i < 128; i++) if (hist[i]) printf(" %d:%ld", i, hist[i]);
    printf("\n");
    return bad != 0;
}
