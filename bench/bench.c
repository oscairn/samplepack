// SPDX-License-Identifier: MIT
// Decode-cost bench for callgrind: decodes every frame of every file given, with the Rust library
// (rust) or the C oracle (oracle), in a function callgrind can toggle on.
//   bench rust|oracle crc|nocrc f1.flac ...        prints the number of samples decoded
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "samplepack.h"
#include "../tests/oracle/fsd.h"

static int16_t out[2 * FSD_MAXB];
static sp_flac_scratch scratch;

__attribute__((noinline)) unsigned long long decode_all_oracle(uint8_t **d, size_t *len, int nf, int crc) {
    unsigned long long s = 0; unsigned sink = 0;
    for (int f = 0; f < nf; f++) {
        long o = fsd_first_frame(d[f], len[f]);
        for (const uint8_t *p = d[f] + o; p < d[f] + len[f];) {
            int n, ch, l = fsd_decode_frame(p, d[f] + len[f], crc, out, &n, &ch);
            if (l < 0) { puts("decode error"); exit(1); }
            p += l; s += (unsigned long long)n * ch; sink += (uint16_t)out[n - 1];
        }
    }
    if (sink == 12345) puts("");
    return s;
}

__attribute__((noinline)) unsigned long long decode_all_rust(uint8_t **d, size_t *len, int nf, int crc) {
    unsigned long long s = 0; unsigned sink = 0;
    for (int f = 0; f < nf; f++) {
        uint64_t o;
        if (sp_flac_first_frame(d[f], len[f], &o)) { puts("bad stream"); exit(1); }
        for (size_t pos = (size_t)o; pos < len[f];) {
            sp_flac_frame_info info;
            if (sp_flac_decode_frame(d[f] + pos, len[f] - pos, crc ? SP_FLAC_CHECK_CRC : 0, &scratch, out, out + FSD_MAXB, FSD_MAXB, &info)) { puts("decode error"); exit(1); }
            pos += info.consumed; s += (unsigned long long)info.samples * info.channels; sink += (uint16_t)out[info.samples - 1];
        }
    }
    if (sink == 12345) puts("");
    return s;
}

int main(int argc, char **argv) {
    if (argc < 4) return 2;
    fsd_init(); int nf = argc - 3; uint8_t **d = malloc((size_t)nf * sizeof *d); size_t *len = malloc((size_t)nf * sizeof *len);
    for (int i = 0; i < nf; i++) {
        FILE *f = fopen(argv[i + 3], "rb"); if (!f) { perror(argv[i + 3]); return 1; }
        fseek(f, 0, SEEK_END); len[i] = (size_t)ftell(f); fseek(f, 0, SEEK_SET);
        d[i] = malloc(len[i] + 16); memset(d[i] + len[i], 0, 16); if (fread(d[i], 1, len[i], f) != len[i]) return 1; fclose(f);
        // the embedded oracle streams end in 16 bytes of padding: drop it
        while (len[i] > 0 && d[i][len[i] - 1] == 0 && len[i] >= 16 && !memcmp(d[i] + len[i] - 16, "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0", 16)) len[i] -= 16;
    }
    int crc = !strcmp(argv[2], "crc");
    printf("samples %llu\n", !strcmp(argv[1], "rust") ? decode_all_rust(d, len, nf, crc) : decode_all_oracle(d, len, nf, crc));
    return 0;
}
