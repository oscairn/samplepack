// SPDX-License-Identifier: AGPL-3.0-or-later
// Differential harness: the Rust decoder (through the C interface) against the C oracle.
//
//   diff files F...             decode every frame of every .flac with both, compare samples
//   diff fuzz N SEED F...       N mutated/truncated frames drawn from the files, fed to both
//
// Checks per case: neither side crashes (guard pages after the input and output buffers, canaries in
// the outputs, ASan/UBSan on the C side when built with them); outputs are untouched on failure and
// only written below the block size on success; both agree on success or failure and, on success, on
// samples, channels and consumed bytes; sp_flac_scan agrees with a loop over the oracle.
#define _DEFAULT_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include "samplepack.h"
#include "../tests/oracle/fsd.h"

static size_t pagesz;

// Memory region whose last usable byte is directly followed by a PROT_NONE page.
typedef struct { uint8_t *base; size_t map; } guarded;
static uint8_t *guarded_alloc(guarded *g, size_t n) {
    size_t body = (n + pagesz - 1) / pagesz * pagesz;
    g->map = body + pagesz;
    g->base = mmap(NULL, g->map, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (g->base == MAP_FAILED) { perror("mmap"); exit(1); }
    if (mprotect(g->base + body, pagesz, PROT_NONE)) { perror("mprotect"); exit(1); }
    return g->base + body - n;
}
static void guarded_free(guarded *g) { munmap(g->base, g->map); }

static uint8_t *slurp(const char *p, size_t *n) {
    FILE *f = fopen(p, "rb"); if (!f) { perror(p); exit(1); }
    fseek(f, 0, SEEK_END); *n = (size_t)ftell(f); fseek(f, 0, SEEK_SET);
    uint8_t *d = malloc(*n + 16); memset(d + *n, 0, 16);
    if (fread(d, 1, *n, f) != *n) exit(1);
    fclose(f); return d;
}

static uint64_t rng_s;
static uint32_t rnd(void) { rng_s = rng_s * 6364136223846793005ull + 1442695040888963407ull; return (uint32_t)(rng_s >> 33); }

typedef struct { uint8_t *d; size_t len; } blob;
static blob *frames; static size_t nframes, capframes;
static long histo[16];
static long diverge_pass, cases, ok_cases, fail_cases, scan_cases;
static unsigned long long stat_unsup_only;
static int verbose_diverge;

static const int16_t CANARY = 0x5A5A;

static sp_flac_scratch *scratch; static int16_t *rl, *rr;           // guarded
static int16_t ol[2 * FSD_MAXB];

static int fail(const char *what, size_t len, int flags) { printf("DIVERGENCE: %s (len %zu flags %d)\n", what, len, flags); return 1; }

// Runs one input through both decoders and compares. Returns nonzero on divergence.
static int one(const uint8_t *src, size_t len, int flags, int padfill) {
    guarded gi; uint8_t *in = guarded_alloc(&gi, len ? len : 1); memcpy(in, src, len);
    uint8_t *op = malloc(len + 16); memcpy(op, src, len); memset(op + len, padfill, 16);
    for (size_t i = 0; i < FSD_MAXB; i++) rl[i] = rr[i] = CANARY;
    sp_flac_frame_info info; memset(&info, 0xAB, sizeof info);
    sp_status st = sp_flac_decode_frame(in, len, flags ? SP_FLAC_CHECK_CRC : 0, scratch, rl, rr, FSD_MAXB, &info);
    int n = 0, ch = 0; memset(ol, 0, sizeof ol);
    int ol_len = fsd_decode_frame(op, op + len, flags, ol, &n, &ch);
    int rc = 0;
    cases++; if ((unsigned)st < 16) histo[st]++;
    if (st == SP_OK) {
        ok_cases++;
        if (ol_len < 0) { rc = fail("rust accepts, oracle rejects", len, flags); }
        else if ((int)info.samples != n || (int)info.channels != ch || (int)info.consumed != ol_len) rc = fail("info differs", len, flags);
        else {
            for (int i = 0; i < n && !rc; i++) if (rl[i] != ol[i] || (ch == 2 && rr[i] != ol[FSD_MAXB + i])) rc = fail("sample differs", len, flags);
            for (int i = n; i < FSD_MAXB && !rc; i++) if (rl[i] != CANARY || (ch == 2 ? 0 : rr[i] != CANARY)) rc = fail("wrote past block size", len, flags);
            if (ch == 1) for (int i = 0; i < FSD_MAXB && !rc; i++) if (rr[i] != CANARY) rc = fail("wrote channel 1 of mono frame", len, flags);
            if (ch == 2) for (int i = n; i < FSD_MAXB && !rc; i++) if (rr[i] != CANARY) rc = fail("wrote ch1 past block size", len, flags);
        }
    } else {
        fail_cases++;
        if (ol_len >= 0) rc = fail("oracle accepts, rust rejects", len, flags);
        for (int i = 0; i < FSD_MAXB && !rc; i++) if (rl[i] != CANARY || rr[i] != CANARY) rc = fail("outputs written on error", len, flags);
        if (!rc && (info.samples | info.channels | info.consumed | info.reserved)) rc = fail("info not zeroed on error", len, flags);
    }
    // scan against a loop over the oracle (decoding from the start of the data, then continuing)
    if (!rc && (cases & 3) == 0) {
        sp_flac_frame_entry tab[64]; sp_flac_scan_summary sum;
        sp_status ss = sp_flac_scan(in, len, flags ? SP_FLAC_CHECK_CRC : 0, scratch, tab, 64, &sum);
        size_t pos = 0, fr = 0; uint64_t tot = 0; int oracle_err = 0, full = 0;
        while (pos < len) {
            if (fr == 64) { full = 1; break; }
            int a, b, l = fsd_decode_frame(op + pos, op + len, flags, ol, &a, &b);
            if (l < 0) { oracle_err = 1; break; }
            if (tab[fr].offset != pos || tab[fr].length != (uint32_t)l || tab[fr].samples != (uint32_t)a) { rc = fail("scan entry differs", len, flags); break; }
            pos += (size_t)l; tot += (uint64_t)a; fr++;
        }
        if (!rc) {
            int expect_ok = !oracle_err && !full;
            if ((ss == SP_OK) != expect_ok || sum.frames != fr || sum.consumed != pos || sum.total_samples != tot) rc = fail("scan summary differs", len, flags);
            if (!rc && full && ss != SP_ERR_BUFFER_TOO_SMALL) rc = fail("scan full table status", len, flags);
        }
        scan_cases++;
    }
    free(op); guarded_free(&gi);
    return rc;
}

static void split_into_frames(const char *path) {
    size_t n; uint8_t *d = slurp(path, &n);
    long o = fsd_first_frame(d, n); if (o < 0) { printf("bad flac %s\n", path); exit(1); }
    int16_t out[2 * FSD_MAXB];
    for (const uint8_t *p = d + o; p < d + n;) {
        int a, b, l = fsd_decode_frame(p, d + n, 1, out, &a, &b);
        if (l < 0) break;                       // e.g. LPC order above 8: keep what decoded
        if (nframes == capframes) { capframes = capframes ? capframes * 2 : 256; frames = realloc(frames, capframes * sizeof *frames); }
        size_t extra = (size_t)(d + n - (p + l)); if (extra > 64) extra = 64;   // real bytes that follow, so len may exceed the frame
        frames[nframes].d = malloc((size_t)l + extra); memcpy(frames[nframes].d, p, (size_t)l + extra); frames[nframes].len = (size_t)l + extra;
        nframes++; p += l;
    }
    free(d);
}

int main(int argc, char **argv) {
    pagesz = (size_t)sysconf(_SC_PAGESIZE); fsd_init();
    guarded g1, g2, g3;
    scratch = (sp_flac_scratch *)guarded_alloc(&g1, sizeof *scratch); memset(scratch, 0, sizeof *scratch);
    rl = (int16_t *)guarded_alloc(&g2, FSD_MAXB * 2); rr = (int16_t *)guarded_alloc(&g3, FSD_MAXB * 2);
    if (argc == 4 && !strcmp(argv[1], "dump")) {   // decode a whole file with the Rust library to interleaved s16le PCM
        size_t n; uint8_t *d = slurp(argv[2], &n); uint64_t first; FILE *o = fopen(argv[3], "wb");
        if (sp_flac_first_frame(d, n, &first) != SP_OK || !o) return 1;
        for (size_t pos = (size_t)first; pos < n;) {
            sp_flac_frame_info inf;
            if (sp_flac_decode_frame(d + pos, n - pos, SP_FLAC_CHECK_CRC, scratch, rl, rr, FSD_MAXB, &inf) != SP_OK) { printf("decode failed at %zu of %zu\n", pos, n); return 1; }
            for (uint32_t i = 0; i < inf.samples; i++) { fwrite(&rl[i], 2, 1, o); if (inf.channels == 2) fwrite(&rr[i], 2, 1, o); }
            pos += inf.consumed;
        }
        fclose(o); return 0;
    }
    if (argc >= 3 && !strcmp(argv[1], "files")) {
        long files = 0, fr = 0;
        for (int a = 2; a < argc; a++) {
            size_t n; uint8_t *d = slurp(argv[a], &n); long o = fsd_first_frame(d, n); uint64_t first;
            if (sp_flac_first_frame(d, n, &first) != SP_OK || (long)first != o) { printf("first_frame differs %s\n", argv[a]); return 1; }
            for (const uint8_t *p = d + o; p < d + n;) {
                int a2, b2, l = fsd_decode_frame(p, d + n, 1, ol, &a2, &b2);
                size_t flen = (size_t)(d + n - p);
                if (l < 0) {   // oracle rejects: Rust must too, once (CRC on)
                    if (one(p, flen, 1, 0)) return 1;
                    break;
                }
                for (int flags = 0; flags < 2; flags++) if (one(p, flen, flags, 0xFF)) { printf("in %s at %ld\n", argv[a], (long)(p - d)); return 1; }
                p += l; fr++;
            }
            files++; free(d);
        }
        printf("files %ld frames %ld cases %ld: no divergence (rust ok %ld, rust rejected %ld, scan checks %ld)\n", files, fr, cases, ok_cases, fail_cases, scan_cases);
        return 0;
    }
    if (argc >= 5 && !strcmp(argv[1], "fuzz")) {
        long N = atol(argv[2]); rng_s = (uint64_t)atoll(argv[3]) * 2 + 1; (void)stat_unsup_only; (void)diverge_pass; (void)verbose_diverge;
        for (int a = 4; a < argc; a++) split_into_frames(argv[a]);
        if (!nframes) { puts("no frames"); return 1; }
        uint8_t *buf = malloc(1 << 16); long div = 0;
        for (long it = 0; it < N; it++) {
            blob *f = &frames[rnd() % nframes]; size_t len = f->len; memcpy(buf, f->d, len);
            int nm = (int)(rnd() % 10);   // 0 mutations: valid frames get fuzzed flags/lengths only
            for (int m = 0; m < nm; m++) {
                size_t at = (rnd() % 3 == 0 && len > 12) ? rnd() % 12 : rnd() % (len ? len : 1);   // a third of the hits go to the header
                switch (rnd() % 8) {
                case 0: case 1: if (len) buf[at] ^= (uint8_t)(1u << (rnd() % 8)); break;
                case 2: if (len) buf[at] = (uint8_t)rnd(); break;
                case 3: if (len) buf[at] = (rnd() & 1) ? 0x00 : 0xFF; break;
                case 4: if (len > 1) { memmove(buf + at, buf + at + 1, len - at - 1); len--; } break;
                case 5: if (len + 1 < (1 << 16)) { memmove(buf + at + 1, buf + at, len - at); buf[at] = (uint8_t)rnd(); len++; } break;
                case 6: { size_t k = 1 + rnd() % 4; for (size_t i = 0; i < k && at + i < len; i++) buf[at + i] = (uint8_t)rnd(); } break;
                case 7: if (len > 4) { size_t b = rnd() % len; size_t k = 1 + rnd() % 8; for (size_t i = 0; i < k && at + i < len && b + i < len; i++) buf[at + i] = buf[b + i]; } break;
                }
            }
            if (rnd() % 4 == 0 && len) len = rnd() % (len + 1);                    // truncate
            if (rnd() % 16 == 0) for (size_t i = 0; i < 24; i++) buf[len++] = (uint8_t)rnd();   // junk after the frame
            if (one(buf, len, (int)(rnd() & 1), (rnd() & 1) ? 0xFF : 0)) {
                div++;
                char name[64]; snprintf(name, sizeof name, "divergence-%ld.bin", div); FILE *o = fopen(name, "wb"); if (o) { fwrite(buf, 1, len, o); fclose(o); }
                if (div >= 10) break;
            }
        }
        printf("cases %ld (rust ok %ld, rust rejected %ld, scan checks %ld) divergences %ld\n", cases, ok_cases, fail_cases, scan_cases, div);
        static const char *nm[] = { "ok", "invalid_argument", "buffer_too_small", "truncated", "bad_sync", "reserved_value", "unsupported", "header_crc", "bad_subframe", "bad_residual", "overrun", "frame_crc", "bad_stream" };
        for (int i = 0; i < 13; i++) printf("  %-17s %ld\n", nm[i], histo[i]);
        free(buf);
        for (size_t i = 0; i < nframes; i++) free(frames[i].d);
        free(frames);
        return div != 0;
    }
    fprintf(stderr, "usage: diff files F... | diff fuzz N SEED F...\n");
    return 2;
}
