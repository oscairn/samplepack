/* SPDX-License-Identifier: AGPL-3.0-or-later */
/*
 * samplepack: C interface to the sample pack reader.
 *
 * Conventions
 *  - Every function returns a status: 0 (SP_OK) or a positive sp_status code. No function
 *    panics, allocates, blocks, or keeps state between calls; all are reentrant. Calls that
 *    share a sp_flac_scratch must not overlap in time.
 *  - All buffers are caller-provided. Nothing is written outside the stated capacities, and the
 *    decoded sample buffers are left untouched when a call fails.
 *  - Lengths and counts are in elements, not bytes, unless stated.
 */
#ifndef SAMPLEPACK_H
#define SAMPLEPACK_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Largest FLAC block size (samples per channel) the decoder accepts. */
#define SP_FLAC_MAX_BLOCK 4096
/* Highest LPC order accepted. */
#define SP_FLAC_MAX_LPC_ORDER 8

typedef enum sp_status {
    SP_OK = 0,
    SP_ERR_INVALID_ARGUMENT = 1, /* null/misaligned pointer, overlapping buffers, unknown flag */
    SP_ERR_BUFFER_TOO_SMALL = 2, /* output smaller than the block size, or frame table full */
    SP_ERR_TRUNCATED = 3,        /* data ends inside the frame header */
    SP_ERR_BAD_SYNC = 4,         /* no frame sync code */
    SP_ERR_RESERVED_VALUE = 5,   /* reserved or invalid value in the frame header */
    SP_ERR_UNSUPPORTED = 6,      /* valid FLAC, outside the accepted subset */
    SP_ERR_HEADER_CRC = 7,       /* frame header CRC-8 mismatch (SP_FLAC_CHECK_CRC) */
    SP_ERR_BAD_SUBFRAME = 8,     /* malformed or reserved subframe */
    SP_ERR_BAD_RESIDUAL = 9,     /* malformed residual */
    SP_ERR_OVERRUN = 10,         /* frame needs more bits than the data holds */
    SP_ERR_FRAME_CRC = 11,       /* frame CRC-16 mismatch (SP_FLAC_CHECK_CRC) */
    SP_ERR_BAD_STREAM = 12       /* not a FLAC stream header */
} sp_status;

/* Flag for sp_flac_decode_frame and sp_flac_scan: verify the header CRC-8 and the frame CRC-16. */
#define SP_FLAC_CHECK_CRC 1u

/*
 * Decoder working memory (32 KiB), reusable across calls. It must be 4-byte aligned. Its
 * contents are irrelevant, but it must be readable memory (zero it once when you create it).
 */
typedef struct sp_flac_scratch {
    int32_t ch[2][SP_FLAC_MAX_BLOCK];
} sp_flac_scratch;

typedef struct sp_flac_frame_info {
    uint32_t samples;  /* samples per channel */
    uint32_t channels; /* 1 or 2 */
    uint32_t consumed; /* bytes the frame occupies, including its CRC-16 */
    uint32_t reserved; /* zero */
} sp_flac_frame_info;

typedef struct sp_flac_frame_entry {
    uint64_t offset;  /* from the start of the scanned data */
    uint32_t length;  /* bytes */
    uint32_t samples; /* per channel */
} sp_flac_frame_entry;

typedef struct sp_flac_scan_summary {
    uint64_t frames;        /* entries written */
    uint64_t consumed;      /* bytes covered by those frames */
    uint64_t total_samples; /* samples per channel over those frames */
} sp_flac_scan_summary;

/*
 * Decode the frame at the start of data[0..len) (len may extend past the frame).
 *
 * Channel 0 is written to ch0[0..samples); channel 1 of a stereo frame to ch1[0..samples).
 * `capacity` is the number of int16 elements available in each of ch0 and ch1. If the frame's
 * block size exceeds the capacity (or ch0 is NULL, or a stereo frame has ch1 NULL),
 * SP_ERR_BUFFER_TOO_SMALL is returned. ch0 and ch1 must not overlap. ch1 may be NULL for
 * mono data. `info` receives the result and is zeroed on failure. A null `data` is allowed only
 * with len == 0.
 */
sp_status sp_flac_decode_frame(const uint8_t *data, size_t len, uint32_t flags,
                               sp_flac_scratch *scratch, int16_t *ch0, int16_t *ch1,
                               size_t capacity, sp_flac_frame_info *info);

/*
 * Walk the frames in data[0..len), which must start at the first frame, by decoding each one,
 * and record their boundaries in table[0..table_capacity). Stops at the first error, or with
 * SP_ERR_BUFFER_TOO_SMALL when the table is full and data remains. `summary` always describes the
 * frames recorded so far. SP_OK means the data ended exactly after the last frame.
 */
sp_status sp_flac_scan(const uint8_t *data, size_t len, uint32_t flags, sp_flac_scratch *scratch,
                       sp_flac_frame_entry *table, size_t table_capacity,
                       sp_flac_scan_summary *summary);

/* Offset of the first frame in a .flac file (after the metadata blocks). */
sp_status sp_flac_first_frame(const uint8_t *data, size_t len, uint64_t *offset);

#ifdef __cplusplus
}
#endif

#endif /* SAMPLEPACK_H */
