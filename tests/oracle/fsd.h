// SPDX-License-Identifier: MIT
// fsd: decoder for a restricted FLAC subset (study code). Kept here as the test oracle for the Rust decoder;
// the only change is the FSD_STRICT_UTF8 block below, off unless defined. Standard FLAC frames, but only:
// 1-2 channels, 16-bit, block size <= FSD_MAXB, CONSTANT / VERBATIM / FIXED (order 0-4) /
// LPC (order <= FSD_MAXLPC) subframes, Rice or Rice2 residuals (escaped partitions accepted),
// wasted bits accepted. Anything else is rejected. Every read is bounded by the frame end:
// the caller guarantees 16 readable bytes after `end` (the pack pads); the reader looks ahead
// up to 8 bytes past the bits it has consumed, so it only fails once it is more than 8 bytes past `end`.
#pragma once
#include <stdint.h>
#include <string.h>

#ifndef FSD_MAXB
#define FSD_MAXB 4096
#endif
#ifndef FSD_MAXLPC
#define FSD_MAXLPC 8
#endif

#ifndef FSD_STAT
#define FSD_STAT(x)
#endif

typedef struct { const uint8_t *p, *end; uint64_t c; int n; int bad; } fsd_br;

static inline void fsd_fill(fsd_br *r) {
    if (__builtin_expect(r->p > r->end + 8, 0)) { r->bad = 1; r->c = ~0ull; r->n = 64; return; }   // ran past the frame
    uint64_t v; memcpy(&v, r->p, 8); v = __builtin_bswap64(v);
    r->c |= v >> r->n; int take = (63 - r->n) >> 3; r->p += take; r->n += take * 8;
}
static inline uint32_t fsd_get(fsd_br *r, int bits) {          // 1..32
    if (r->n < bits) fsd_fill(r);
    uint32_t v = (uint32_t)(r->c >> (64 - bits)); r->c <<= bits; r->n -= bits; return v;
}
static inline int32_t fsd_sget(fsd_br *r, int bits) { uint32_t v = fsd_get(r, bits); return (int32_t)(v << (32 - bits)) >> (32 - bits); }
static uint32_t fsd_rice_slow(fsd_br *r, int k) {
    uint32_t q = 0; int z;
    for (;;) {
        if (r->n < 32) fsd_fill(r);
        if (r->bad) return 0;
        z = r->c ? __builtin_clzll(r->c) : 64;
        if (z < r->n) break;
        q += (uint32_t)r->n; r->c = 0; r->n = 0;
    }
    r->c <<= z + 1; r->n -= z + 1; q += (uint32_t)z;
    uint32_t lo = k ? fsd_get(r, k) : 0;
    return (q << k) | lo;
}
static inline void fsd_rice_run(fsd_br *r, int k, int32_t *s, int cnt) {
    uint64_t c = r->c; int n = r->n; const uint8_t *p = r->p;
    for (int i = 0; i < cnt; i++) {
        if (n < 40) {
            if (__builtin_expect(p > r->end + 8, 0)) { r->bad = 1; return; }
            uint64_t v; memcpy(&v, p, 8); v = __builtin_bswap64(v); c |= v >> n; int take = (63 - n) >> 3; p += take; n += take * 8;
        }
        int z = __builtin_clzll(c | 1), len = z + 1 + k; uint32_t u;
        if (__builtin_expect(len <= n, 1)) { u = ((uint32_t)z << k) | (uint32_t)(((c << (z + 1)) >> 1) >> (63 - k)); c <<= len; n -= len; }
        else { r->c = c; r->n = n; r->p = p; u = fsd_rice_slow(r, k); if (r->bad) return; c = r->c; n = r->n; p = r->p; }
        s[i] = (int32_t)(u >> 1) ^ -(int32_t)(u & 1);
    }
    r->c = c; r->n = n; r->p = p;
}

static uint16_t fsd_crc16_tab[256]; static uint8_t fsd_crc8_tab[256];
static void fsd_init(void) {
    for (int i = 0; i < 256; i++) {
        uint16_t c = (uint16_t)(i << 8); for (int j = 0; j < 8; j++) c = (uint16_t)((c & 0x8000) ? (c << 1) ^ 0x8005 : c << 1); fsd_crc16_tab[i] = c;
        uint8_t d = (uint8_t)i; for (int j = 0; j < 8; j++) d = (uint8_t)((d & 0x80) ? (d << 1) ^ 0x07 : d << 1); fsd_crc8_tab[i] = d;
    }
}
static uint16_t fsd_crc16(const uint8_t *p, size_t n) { uint16_t c = 0; while (n--) c = (uint16_t)((c << 8) ^ fsd_crc16_tab[(c >> 8) ^ *p++]); return c; }

// residual of one subframe into s[order..n)
static int fsd_residual(fsd_br *r, int32_t *s, int n, int order) {
    int method = (int)fsd_get(r, 2); if (method > 1) return -1;
    int pbits = method ? 5 : 4, esc = method ? 31 : 15, porder = (int)fsd_get(r, 4);
    int psize = n >> porder; if ((psize << porder) != n || psize < order) return -1;
    for (int pt = 0, i = order; pt < (1 << porder); pt++) {
        int pe = (pt + 1) * psize, k = (int)fsd_get(r, pbits);
        FSD_STAT(if (method) st_rice2++;)
        if (k == esc) { FSD_STAT(st_escape++;) int b = (int)fsd_get(r, 5); for (; i < pe; i++) s[i] = b ? fsd_sget(r, b) : 0; }
        else { fsd_rice_run(r, k, s + i, pe - i); i = pe; }
        if (r->bad) return -1;
    }
    return 0;
}

// unsigned accumulation: identical to the exact sum for valid frames, no undefined overflow for malformed ones
#define FSD_LPC32(N) case N: for (int i = N; i < n; i++) { uint32_t a = 0; for (int j = 0; j < N; j++) a += (uint32_t)q[j] * (uint32_t)s[i - 1 - j]; s[i] = (int32_t)((uint32_t)s[i] + (uint32_t)((int32_t)a >> sh)); } break;
#define FSD_LPC64(N) case N: for (int i = N; i < n; i++) { uint64_t a = 0; for (int j = 0; j < N; j++) a += (uint64_t)((int64_t)q[j] * s[i - 1 - j]); s[i] = (int32_t)((uint32_t)s[i] + (uint32_t)(int32_t)((int64_t)a >> sh)); } break;

static int fsd_subframe(fsd_br *r, int32_t *s, int n, int bps) {
    if (fsd_get(r, 1)) return -1;
    int type = (int)fsd_get(r, 6), wasted = 0;
    FSD_STAT(st_type[type]++;)
    if (fsd_get(r, 1)) { wasted = 1; FSD_STAT(st_wasted++;) while (!fsd_get(r, 1)) { if (++wasted >= bps || r->bad) return -1; } bps -= wasted; }
    if (type == 0) { int32_t v = fsd_sget(r, bps); for (int i = 0; i < n; i++) s[i] = v; }
    else if (type == 1) { for (int i = 0; i < n; i++) s[i] = fsd_sget(r, bps); }
    else if (type >= 8 && type <= 12) {
        int order = type - 8; if (order > n) return -1;
        for (int i = 0; i < order; i++) s[i] = fsd_sget(r, bps);
        if (fsd_residual(r, s, n, order)) return -1;
        // shortcut: unsigned adds so malformed input wraps instead of hitting signed overflow
        uint32_t *u = (uint32_t *)s;
        switch (order) {
        case 1: for (int i = 1; i < n; i++) u[i] += u[i-1]; break;
        case 2: for (int i = 2; i < n; i++) u[i] += 2*u[i-1] - u[i-2]; break;
        case 3: for (int i = 3; i < n; i++) u[i] += 3*(u[i-1] - u[i-2]) + u[i-3]; break;
        case 4: for (int i = 4; i < n; i++) u[i] += 4*(u[i-1] + u[i-3]) - 6*u[i-2] - u[i-4]; break;
        }
    } else if (type >= 32) {
        int order = type - 31; if (order > FSD_MAXLPC || order > n) return -1;
        for (int i = 0; i < order; i++) s[i] = fsd_sget(r, bps);
        int prec = (int)fsd_get(r, 4) + 1; if (prec == 16) return -1;
        int sh = fsd_sget(r, 5); if (sh < 0) return -1;
        int32_t q[FSD_MAXLPC]; for (int j = 0; j < order; j++) q[j] = fsd_sget(r, prec);
        if (fsd_residual(r, s, n, order)) return -1;
        // 32-bit sums are exact when bps + prec + ceil(log2(order)) <= 32 (as libFLAC decides)
        int lg = 0; while ((1 << lg) < order) lg++;
        if (bps + prec + lg <= 32) { switch (order) { FSD_LPC32(1) FSD_LPC32(2) FSD_LPC32(3) FSD_LPC32(4) FSD_LPC32(5) FSD_LPC32(6) FSD_LPC32(7) FSD_LPC32(8) } }
        else { FSD_STAT(st_lpc64++;) switch (order) { FSD_LPC64(1) FSD_LPC64(2) FSD_LPC64(3) FSD_LPC64(4) FSD_LPC64(5) FSD_LPC64(6) FSD_LPC64(7) FSD_LPC64(8) } }
    } else return -1;
    if (wasted) for (int i = 0; i < n; i++) s[i] = (int32_t)((uint32_t)s[i] << wasted);
    return r->bad ? -1 : 0;
}

// Decodes one frame starting at p (frame bytes end at `end`, 16 readable bytes beyond).
// out: planar int16, channel c at out + c*FSD_MAXB. Returns bytes consumed, or -1 if the frame is
// malformed, outside the subset, or (check_crc) fails its CRC-8 / CRC-16.
static int fsd_decode_frame(const uint8_t *p, const uint8_t *end, int check_crc, int16_t *out, int *frames, int *channels) {
    static const int bs_tab[16] = { 0, 192, 576, 1152, 2304, 4608, -1, -2, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768 };
    if (end - p < 6 || p[0] != 0xFF || (p[1] & 0xFE) != 0xF8) return -1;
    int bsc = p[2] >> 4, src = p[2] & 15, ca = p[3] >> 4, ssz = (p[3] >> 1) & 7;
    if ((p[3] & 1) || src == 15 || ca > 10 || (ssz != 0 && ssz != 4)) return -1;
    const uint8_t *h = p + 4;
    int ext = *h >= 0xFE ? 7 : *h >= 0xFC ? 6 : *h >= 0xF8 ? 5 : *h >= 0xF0 ? 4 : *h >= 0xE0 ? 3 : *h >= 0xC0 ? 2 : *h < 0x80 ? 1 : 0;
    if (!ext) return -1;
#ifdef FSD_STRICT_UTF8
    // test-suite patch (absent from the original): the frame/sample number must be well-formed UTF-8 style
    // (RFC 9639, "coded number"): no 0xFF lead byte, 7 bytes only for a variable block size, continuation bytes 10xxxxxx.
    if (*h == 0xFF || (*h == 0xFE && !(p[1] & 1))) return -1;
    for (int i = 1; i < ext; i++) if ((h[i] & 0xC0) != 0x80) return -1;
#endif
    h += ext;
    int n = bs_tab[bsc];
    if (n == -1) n = *h++ + 1; else if (n == -2) { n = (h[0] << 8 | h[1]) + 1; h += 2; }
    if (n <= 0 || n > FSD_MAXB) return -1;
    if (src == 12) h += 1; else if (src == 13 || src == 14) h += 2;
    if (h >= end) return -1;
    if (check_crc) { uint8_t c = 0; for (const uint8_t *q = p; q < h; q++) c = fsd_crc8_tab[c ^ *q]; if (c != *h) return -1; }
    h++;
    int ch = ca < 8 ? ca + 1 : 2; if (ch > 2) return -1;
    FSD_STAT(st_ca[ca]++;)
    fsd_br r = { h, end, 0, 0, 0 };
    int32_t buf[2][FSD_MAXB];
    for (int c = 0; c < ch; c++) {
        int bps = 16 + ((ca == 8 && c == 1) || (ca == 9 && c == 0) || (ca == 10 && c == 1));
        if (fsd_subframe(&r, buf[c], n, bps)) return -1;
    }
    uint64_t used = (uint64_t)(r.p - p) * 8 - (uint64_t)r.n; size_t len = (size_t)((used + 7) / 8) + 2;
    if (p + len > end) return -1;
    if (check_crc && fsd_crc16(p, len - 2) != (uint16_t)(p[len - 2] << 8 | p[len - 1])) return -1;
    int16_t *L = out, *R = out + FSD_MAXB; int32_t *a = buf[0], *b = buf[1];
    if (ch == 1) for (int i = 0; i < n; i++) L[i] = (int16_t)a[i];
    else switch (ca) {
        case 8: for (int i = 0; i < n; i++) { L[i] = (int16_t)a[i]; R[i] = (int16_t)((uint32_t)a[i] - (uint32_t)b[i]); } break;
        case 9: for (int i = 0; i < n; i++) { L[i] = (int16_t)((uint32_t)a[i] + (uint32_t)b[i]); R[i] = (int16_t)b[i]; } break;
        case 10: for (int i = 0; i < n; i++) { int32_t m = (int32_t)((uint32_t)a[i] << 1) | (b[i] & 1); L[i] = (int16_t)((int32_t)((uint32_t)m + (uint32_t)b[i]) >> 1); R[i] = (int16_t)((int32_t)((uint32_t)m - (uint32_t)b[i]) >> 1); } break;
        default: for (int i = 0; i < n; i++) { L[i] = (int16_t)a[i]; R[i] = (int16_t)b[i]; }
    }
    *frames = n; *channels = ch;
    return (int)len;
}

// Offset of the first frame in a .flac file (after the metadata blocks), or -1.
static long fsd_first_frame(const uint8_t *d, size_t len) {
    if (len < 8 || memcmp(d, "fLaC", 4)) return -1;
    size_t i = 4;
    for (;;) { if (i + 4 > len) return -1; int last = d[i] & 0x80; size_t l = (size_t)d[i+1] << 16 | d[i+2] << 8 | d[i+3]; i += 4 + l; if (last) break; }
    return i <= len ? (long)i : -1;
}
