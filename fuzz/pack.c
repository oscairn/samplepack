// SPDX-License-Identifier: MIT
// Pack reader fuzz harness, through the C interface:
//
//   pack N SEED P...      N packs made by mutating, truncating and splicing the seed packs P
//
// Each case is mapped so that its last byte is directly followed by a PROT_NONE page (an
// out-of-bounds read of the mapped region faults); run under ASan/UBSan for the C side. Half the
// mutated cases get their entry and footer checksums recomputed, so the mutations reach the
// field checks behind them. Checks per case: no crash or abort; every returned pointer lies
// inside the mapping; anything sp_record_verify accepts decodes completely without CRC checks;
// anything sp_pack_verify accepts has every record verify; the constant-time record calls, with
// every listed entry and a damaged copy of it, return an error or stay inside the mapping.
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
static uint32_t rd32(const uint8_t *p) { uint32_t v; memcpy(&v, p, 4); return v; }
static uint64_t rd64(const uint8_t *p) { uint64_t v; memcpy(&v, p, 8); return v; }
static void wr32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }

// Recomputes the checksum of every page that starts with an entry prefix, and the footer's.
static void reseal(uint8_t *d, size_t len) {
    for (size_t at = 0; at + 24 <= len; at += SP_PACK_PAGE) {
        if (memcmp(d + at, "OSP", 3)) continue;
        uint32_t checked = rd32(d + at + 16);
        if (checked < 24 || checked > len - at) continue;
        wr32(d + at + 20, 0);
        wr32(d + at + 20, crc32c(d + at, checked));
    }
    if (len >= 32 && !memcmp(d + len - 32, "OSPF", 4)) wr32(d + len - 4, crc32c(d + len - 32, 28));
}

typedef struct { uint8_t *base; size_t map; } guarded;
static uint8_t *guarded_alloc(guarded *g, size_t n) {
    size_t body = (n + pagesz - 1) / pagesz * pagesz;
    g->map = body + pagesz;
    g->base = mmap(NULL, g->map, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    if (g->base == MAP_FAILED) { perror("mmap"); exit(1); }
    if (mprotect(g->base + body, pagesz, PROT_NONE)) { perror("mprotect"); exit(1); }
    return g->base + body - n;
}

static uint8_t *slurp(const char *p, size_t *n) {
    FILE *f = fopen(p, "rb"); if (!f) { perror(p); exit(1); }
    fseek(f, 0, SEEK_END); *n = (size_t)ftell(f); fseek(f, 0, SEEK_SET);
    uint8_t *d = malloc(*n); if (fread(d, 1, *n, f) != *n) exit(1);
    fclose(f); return d;
}

#define MAXIDX 4096
static sp_index_entry idx[MAXIDX], idx2[MAXIDX];
static sp_verify_scratch vs;
static sp_flac_scratch fs;
static int16_t c0[1 << 20], c1[1 << 20];
static long hist[64], cases, opened, recovered, records_seen, verified, packs_verified, bad;

static void die(const char *what, size_t len) { printf("FAILURE: %s (len %zu)\n", what, len); bad++; }

static int inside(const uint8_t *p, size_t n, const uint8_t *d, size_t len) {
    return n == 0 || (p >= d && p <= d + len && n <= (size_t)(d + len - p));
}

static void one(const uint8_t *src, size_t len) {
    guarded g; uint8_t *d = guarded_alloc(&g, len ? len : 1);
    memcpy(d, src, len);
    mprotect(g.base, g.map - pagesz, PROT_READ);   // the reader gets a read-only mapping
    cases++;
    sp_pack_info pi; uint64_t committed = len;
    sp_status st = sp_pack_open(d, len, &pi);
    hist[st & 63]++;
    if (st == SP_ERR_NO_FOOTER) {
        if (sp_pack_recover(d, len, &committed) == SP_OK) {
            if (committed > len || committed % SP_PACK_PAGE) die("recover length", len);
            recovered++;
            st = sp_pack_open(d, (size_t)committed, &pi);
            if (st != SP_OK) die("recovered length does not open", len);
        }
    }
    if (st == SP_OK) {
        opened++;
        size_t L = (size_t)committed, n = 0, m = 0;
        sp_status is = sp_pack_index(d, L, idx, MAXIDX, &n);
        if (is == SP_OK && n != pi.records) die("index count", len);
        sp_pack_rebuild_index(d, L, idx2, MAXIDX, &m);
        if (is != SP_OK) n = 0;
        for (size_t i = 0; i < n && i < 64; i++) {
            // the constant-time record calls on the entry as listed, and on a damaged copy of it:
            // errors, or pointers inside the mapping, whatever the record's header holds
            sp_index_entry e[2] = { idx[i], idx[i] };
            switch (rnd() % 4) {
            case 0: e[1].offset += (uint64_t)(rnd() % 5) * SP_PACK_PAGE - 2 * SP_PACK_PAGE; break;
            case 1: e[1].length = rnd() % 3 == 0 ? UINT64_MAX : (uint64_t)(rnd() % 64) * SP_PACK_PAGE; break;
            case 2: e[1].preload = (uint64_t)(rnd() % 8) * SP_PACK_PAGE; break;
            case 3: e[1].id[rnd() % 32] ^= 1; break;
            }
            for (int k = 0; k < 2; k++) {
                const uint8_t *w, *f, *t; size_t wb, fl, te; uint64_t got;
                if (sp_record_window(d, L, &e[k], &w, &wb) == SP_OK && !inside(w, wb, d, L)) die("window outside", len);
                if (sp_record_frames(d, L, &e[k], &f, &fl, &t, &te) == SP_OK && (!inside(f, fl, d, L) || !inside(t, te * 4, d, L))) die("frames outside", len);
                sp_record_decode(d, L, &e[k], rnd() % 8, rnd() % 8, rnd() & 1, &fs, c0, c1, 1 << 20, &got);
            }
            sp_record_info ri;
            if (sp_pack_record(d, L, &idx[i], &ri) != SP_OK) continue;
            records_seen++;
            if (sp_record_verify(d, L, &idx[i], &vs) == SP_OK) {
                verified++;
                uint64_t got = 0;
                if (ri.frames > (1u << 20) || sp_record_decode(d, L, &idx[i], 0, ri.blocks, 0, &fs, c0, c1, 1 << 20, &got) != SP_OK || got != ri.frames)
                    die("verified record does not decode", len);
                uint8_t id[32];
                if (sp_record_sample_id(d, L, &idx[i], &vs, id) != SP_OK || memcmp(id, ri.id, 32)) die("verified record id", len);
            }
        }
        if ((cases & 7) == 0 && sp_pack_verify(d, L, &vs, idx, MAXIDX) == SP_OK) {
            packs_verified++;
            if (sp_pack_index(d, L, idx, MAXIDX, &n) != SP_OK) die("verified pack has no index", len);
            for (size_t i = 0; i < n; i++) {
                if (sp_record_verify(d, L, &idx[i], &vs) != SP_OK) die("verified pack, record fails", len);
            }
        }
    }
    munmap(g.base, g.map);
}

int main(int argc, char **argv) {
    pagesz = (size_t)sysconf(_SC_PAGESIZE); crc_init();
    if (argc < 5 || strcmp(argv[1], "pack")) { fprintf(stderr, "usage: pack N SEED P...\n"); return 2; }
    long N = atol(argv[2]); rng_s = (uint64_t)atoll(argv[3]) * 2 + 1;
    int np = argc - 4; uint8_t **seed = malloc((size_t)np * sizeof *seed); size_t *slen = malloc((size_t)np * sizeof *slen);
    size_t maxlen = 0;
    for (int i = 0; i < np; i++) { seed[i] = slurp(argv[4 + i], &slen[i]); if (slen[i] > maxlen) maxlen = slen[i]; }
    for (int i = 0; i < np; i++) one(seed[i], slen[i]);   // unmodified seeds first
    if (hist[0] != np || bad) { printf("seed packs must open cleanly\n"); return 1; }
    size_t cap = maxlen * 2 + 4 * SP_PACK_PAGE; uint8_t *buf = malloc(cap);
    static const uint32_t interesting[] = { 0, 1, 2, 3, 16, 24, 128, 1023, 1024, 4095, 4096, 4097, 8192, 16384, 65535, 65536, 0x7FFFFFFF, 0x80000000u, 0xFFFFFFFFu };
    for (long it = 0; it < N; it++) {
        int k = (int)(rnd() % (unsigned)np); size_t len = slen[k]; memcpy(buf, seed[k], len);
        int nm = (int)(rnd() % 6);
        for (int m = 0; m < nm && len; m++) {
            size_t page = (rnd() % ((len + SP_PACK_PAGE - 1) / SP_PACK_PAGE)) * SP_PACK_PAGE;
            size_t at = rnd() % 4 ? page + rnd() % 160 : rnd() % len;     // mostly entry headers
            if (rnd() % 8 == 0) at = len - 1 - rnd() % (len < 64 ? len : 64);  // footer and trailer end
            if (at >= len) at = len - 1;
            switch (rnd() % 7) {
            case 0: case 1: buf[at] ^= (uint8_t)(1u << (rnd() % 8)); break;
            case 2: buf[at] = (uint8_t)rnd(); break;
            case 3: if (at + 4 <= len) wr32(buf + at, interesting[rnd() % (sizeof interesting / sizeof interesting[0])]); break;
            case 4: if (at + 4 <= len) wr32(buf + at, rd32(buf + at) + (rnd() % 3) * SP_PACK_PAGE - SP_PACK_PAGE); break;
            case 5: {   // splice pages from another seed pack at a page boundary
                int o = (int)(rnd() % (unsigned)np); size_t src = (rnd() % (slen[o] / SP_PACK_PAGE)) * SP_PACK_PAGE;
                size_t n = SP_PACK_PAGE * (1 + rnd() % 4); if (src + n > slen[o]) n = slen[o] - src;
                if (page + n > cap) break;
                memcpy(buf + page, seed[o] + src, n); if (page + n > len) len = page + n;
            } break;
            case 6: if (at + 8 <= len) { uint64_t v = rd64(buf + at) ^ ((uint64_t)1 << (rnd() % 40)); memcpy(buf + at, &v, 8); } break;
            }
        }
        if (rnd() % 4 == 0) len = rnd() % 3 ? (rnd() % (len / SP_PACK_PAGE + 1)) * SP_PACK_PAGE : rnd() % (len + 1);   // truncate
        if (rnd() % 16 == 0 && len + 3000 <= cap) for (int i = 0; i < 3000; i++) buf[len++] = (uint8_t)rnd();     // torn junk
        if (rnd() & 1) reseal(buf, len);
        one(buf, len);
        if (bad >= 5) break;
    }
    printf("cases %ld: opened %ld (after recovery %ld), records checked %ld, records verified in full %ld, packs verified in full %ld, failures %ld\n",
           cases, opened, recovered, records_seen, verified, packs_verified, bad);
    printf("sp_pack_open status histogram:");
    for (int i = 0; i < 64; i++) if (hist[i]) printf(" %d:%ld", i, hist[i]);
    printf("\n");
    for (int i = 0; i < np; i++) free(seed[i]);
    free(seed); free(slen); free(buf);
    return bad != 0;
}
