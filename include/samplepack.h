/* SPDX-License-Identifier: MIT */
/*
 * samplepack: C interface to the Oscairn Sample Pack (OSP, .osp) reader and its FLAC subset
 * decoder, and to Oscairn Waveform (.owf) files. docs/format.md and docs/waveform.md are the
 * format specifications.
 *
 * Conventions
 *  - Every function returns a status: 0 (SP_OK) or a positive sp_status code. No function
 *    panics, allocates, blocks, or keeps state between calls; all are reentrant. Calls that
 *    share a sp_flac_scratch must not overlap in time.
 *  - All buffers are caller-provided. Nothing is written outside the stated capacities, and the
 *    decoded sample buffers are left untouched when a call fails.
 *  - Lengths and counts are in elements, not bytes, unless stated.
 *  - Memory: besides the caller's buffers (a pack mapping, 56 bytes per index entry, 32 KiB of
 *    sp_flac_scratch to decode, 64 KiB of sp_verify_scratch to verify) a call uses only stack,
 *    a few kilobytes, without recursion (README.md, "Memory"). Nothing grows between calls.
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
    SP_ERR_BAD_STREAM = 12,      /* not a FLAC stream header */
    /* packs (docs/format.md, "Errors") */
    SP_ERR_NOT_PACK = 32,            /* shorter than the pack header, or wrong magic */
    SP_ERR_UNSUPPORTED_VERSION = 33, /* major version other than 1 */
    SP_ERR_NO_FOOTER = 34,           /* no valid footer at the end: uncommitted tail, see sp_pack_recover */
    SP_ERR_TOO_LARGE = 35,           /* pack larger than SP_PACK_MAX_BYTES */
    SP_ERR_BAD_CHECKSUM = 36,        /* entry CRC-32C mismatch */
    SP_ERR_BAD_ENTRY = 37,           /* malformed entry prefix */
    SP_ERR_BAD_FOOTER = 38,          /* footer fields inconsistent */
    SP_ERR_BAD_TRAILER = 39,         /* trailer count, length or fields inconsistent */
    SP_ERR_BAD_OFFSET = 40,          /* offset, length or preload span unaligned or out of range */
    SP_ERR_OVERLAP = 41,             /* records overlap or are not listed in file order */
    SP_ERR_RESERVED_NONZERO = 42,    /* reserved field not zero */
    SP_ERR_BAD_FORMAT = 43,          /* rate, channels, bits, flags or block size outside version 1 */
    SP_ERR_BAD_LAYOUT = 44,          /* head, window or record length, window frames or block count wrong */
    SP_ERR_BAD_FRAME_TABLE = 45,     /* frame table not increasing, or smallest/largest frame wrong */
    SP_ERR_BAD_CREDIT = 46,          /* credit too long or not UTF-8 */
    SP_ERR_DUPLICATE_ID = 47,        /* two live records with one sample id */
    SP_ERR_NOT_LIVE = 48,            /* tombstone for a sample id that is not live */
    SP_ERR_MISMATCH = 49,            /* record disagrees with the index entry pointing at it */
    SP_ERR_NOT_FOUND = 50,           /* no record with that sample id */
    SP_ERR_FRAME_MISMATCH = 51,      /* frame block size, channels, number, rate or length disagree with the record */
    SP_ERR_PADDING = 52,             /* padding not zero (full verification) */
    SP_ERR_WINDOW_MISMATCH = 53,     /* preload window differs from the decoded frames */
    SP_ERR_MD5_MISMATCH = 54,        /* MD5 of the decoded audio differs */
    SP_ERR_ID_MISMATCH = 55,         /* sample id of the decoded audio differs */
    SP_ERR_TRAILER_MISMATCH = 56,    /* live set rebuilt from the records differs from the trailer */
    SP_ERR_NOT_RECORD = 57,          /* the entry is not a record */
    SP_ERR_RECOVER_LIMIT = 58,       /* sp_pack_recover reached its bound on work: damaged, not torn */
    /* waveforms (docs/waveform.md, "Refusals") */
    SP_ERR_NOT_WAVEFORM = 64,         /* no OPWF magic */
    SP_ERR_WF_VERSION = 65,           /* version other than 1 */
    SP_ERR_WF_FLAGS = 66,             /* flags not 0 */
    SP_ERR_WF_LENGTH = 67,            /* length not 20 + 32 S + 40 K, or over SP_WAVEFORM_MAX_BYTES */
    SP_ERR_WF_SIZE = 68,              /* size field differs from the length */
    SP_ERR_WF_CHECKSUM = 69,          /* CRC-32C of the body mismatch */
    SP_ERR_WF_SAMPLE_COUNT = 70,      /* S outside 1 to SP_WAVEFORM_MAX_SAMPLES */
    SP_ERR_WF_BANK_COUNT = 71,        /* K outside 1 to SP_WAVEFORM_MAX_BANKS */
    SP_ERR_WF_RESERVED = 72,          /* reserved byte of a key bank not 0 */
    SP_ERR_WF_RANGE = 73,             /* key bank field outside its range */
    SP_ERR_WF_LOW_ABOVE_HIGH = 74,    /* lowest key or velocity above the highest */
    SP_ERR_WF_SAMPLE_INDEX = 75,      /* sample index of S or more */
    SP_ERR_WF_LOOP = 76,              /* loop points without a loop, or out of order */
    SP_ERR_WF_NOT_SORTED = 77,        /* key banks not in canonical order */
    SP_ERR_WF_DUPLICATE_BANK = 78,    /* two equal key banks */
    SP_ERR_WF_SAMPLE_ORDER = 79,      /* sample ids not in order of first use */
    SP_ERR_WF_UNUSED_SAMPLE = 80,     /* a sample id no key bank uses */
    SP_ERR_WF_DUPLICATE_SAMPLE = 81,  /* one sample id listed twice */
    SP_ERR_WF_END = 82                /* end neither 0 nor greater than start */
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

/* ---------------------------------------------------------------------------------------------
 * Packs (docs/format.md). Every function takes the committed pack as data[0..len): normally a
 * read-only mapping of the whole file. Returned pointers point into that memory. Nothing in a
 * pack is trusted: every offset, length and count is checked against len before use.
 */

#define SP_PACK_PAGE 4096            /* entry alignment */
#define SP_PACK_MAX_BYTES (1u << 30) /* largest pack */
#define SP_PACK_MAX_RECORDS 1048576  /* most records in one trailer */
#define SP_PACK_MAX_CREDIT 1024      /* longest credit string, bytes */
#define SP_SAMPLE_ID_BYTES 32

/* One live record: an element of the index the caller holds. */
typedef struct sp_index_entry {
    uint8_t id[SP_SAMPLE_ID_BYTES]; /* sample id */
    uint64_t offset;                /* record offset in the pack */
    uint64_t length;                /* record length, bytes */
    uint64_t preload;               /* bytes from the record start to the end of its window: read these first */
} sp_index_entry;

typedef struct sp_pack_info {
    uint64_t length;         /* committed length (len) */
    uint64_t trailer_offset; /* == length for an empty pack */
    uint64_t records;        /* live records */
    uint32_t minor;          /* minor version of the pack header */
    uint32_t reserved;       /* zero */
} sp_pack_info;

/* A record's header fields, as sp_pack_record found them. */
typedef struct sp_record_info {
    uint64_t offset, length;
    uint8_t id[SP_SAMPLE_ID_BYTES]; /* as stated; trusted only after sp_record_verify */
    uint8_t md5[16];                /* MD5 of the decoded audio, as stated */
    uint32_t rate, channels, bits, frames;
    uint32_t window_frames, block_size, blocks, frame_bytes;
    uint32_t head_len, window_bytes, credit_offset, credit_len; /* offsets within the record */
    uint32_t table_offset, min_frame, max_frame, minor;
} sp_record_info;

/* Working memory for verification (about 64 KiB, 4-byte aligned), reusable, one call at a time. */
typedef struct sp_verify_scratch {
    sp_flac_scratch flac;
    int16_t ch[2][SP_FLAC_MAX_BLOCK];
    uint8_t bytes[SP_FLAC_MAX_BLOCK * 4];
} sp_verify_scratch;

/* Check the header, footer and trailer. SP_ERR_NO_FOOTER: the tail is uncommitted (torn append);
 * call sp_pack_recover and use its length. `info` is zeroed on failure. */
sp_status sp_pack_open(const uint8_t *data, size_t len, sp_pack_info *info);

/* The committed length of a pack whose tail may be torn: the largest page-aligned prefix that
 * sp_pack_open accepts (SP_PACK_PAGE, an empty pack, if none). A writer truncates the file to it;
 * a reader uses data[0..committed). On an error nothing may be truncated.
 * Only for a pack this device itself was writing when it stopped: a pack from anywhere else must
 * pass sp_pack_open and sp_pack_verify, and is refused, never recovered, if open fails. Whatever
 * the input, at most 2 x len bytes are checksummed; past that SP_ERR_RECOVER_LIMIT is returned. */
sp_status sp_pack_recover(const uint8_t *data, size_t len, uint64_t *committed);

/* Fill table[0..capacity) with the trailer's entries sorted by id, for sp_index_find. *count is the
 * number written, or the number needed with SP_ERR_BUFFER_TOO_SMALL. */
sp_status sp_pack_index(const uint8_t *data, size_t len, sp_index_entry *table, size_t capacity,
                        size_t *count);

/* As sp_pack_index, but rebuilt from the records and tombstones alone (every record header is
 * checked). Needs one slot per record and tombstone in the pack. */
sp_status sp_pack_rebuild_index(const uint8_t *data, size_t len, sp_index_entry *table,
                                size_t capacity, size_t *count);

/* Binary search of a sorted index: *position is the entry's index, or SP_ERR_NOT_FOUND. */
sp_status sp_index_find(const sp_index_entry *table, size_t count, const uint8_t id[SP_SAMPLE_ID_BYTES],
                        size_t *position);

/* Check the header of the record an entry points at (CRC-32C, every field, the frame table) and
 * describe it. No audio is decoded.
 *
 * The sp_record_* calls below take the same index entry and re-read the record's header from the
 * pack on every call. sp_record_window, sp_record_frames and sp_record_decode do it in constant
 * time: every header field and its agreement with the entry is checked again, the CRC-32C, credit
 * and frame table only by sp_pack_record. A damaged entry gives an error, never a read outside
 * data[0..len). */
sp_status sp_pack_record(const uint8_t *data, size_t len, const sp_index_entry *entry,
                         sp_record_info *info);

/* The preload window: frames [0, window_frames), int16 little-endian interleaved, 4096-aligned
 * within the pack. *bytes may be 0 (no window). */
sp_status sp_record_window(const uint8_t *data, size_t len, const sp_index_entry *entry,
                           const uint8_t **window, size_t *bytes);

/* The FLAC frames (one stream) and the frame table: table_entries (= blocks + 1) little-endian
 * uint32 offsets into the frames. */
sp_status sp_record_frames(const uint8_t *data, size_t len, const sp_index_entry *entry,
                           const uint8_t **frames, size_t *frames_len, const uint8_t **table,
                           size_t *table_entries);

/* Decode blocks [first_block, first_block + blocks) into ch0 (and ch1 for stereo), contiguously;
 * each needs capacity for every frame of the range. flags: 0 at play time, or SP_FLAC_CHECK_CRC.
 * On an error the outputs may hold the blocks decoded before it. */
sp_status sp_record_decode(const uint8_t *data, size_t len, const sp_index_entry *entry,
                           uint32_t first_block, uint32_t blocks, uint32_t flags,
                           sp_flac_scratch *scratch, int16_t *ch0, int16_t *ch1, size_t capacity,
                           uint64_t *frames_written);

/* Sample id of 16-bit interleaved PCM (frames x channels samples). */
sp_status sp_sample_id_pcm(uint32_t rate, uint32_t channels, const int16_t *pcm, size_t frames,
                           uint8_t id[SP_SAMPLE_ID_BYTES]);

/* Sample id computed from a record's decoded audio (header checked as by sp_pack_record, then
 * every frame decoded, CRCs checked). */
sp_status sp_record_sample_id(const uint8_t *data, size_t len, const sp_index_entry *entry,
                              sp_verify_scratch *scratch, uint8_t id[SP_SAMPLE_ID_BYTES]);

/* Full verification of one record: header, padding, every frame (CRCs, numbering, block sizes),
 * window, MD5 and recomputed sample id. Required before a record is installed or trusted. */
sp_status sp_record_verify(const uint8_t *data, size_t len, const sp_index_entry *entry,
                           sp_verify_scratch *scratch);

/* Full verification of a whole pack: sp_pack_open, every entry, the rebuilt live set equal to the
 * trailer, and sp_record_verify on every live record. table: one slot per record and tombstone. */
sp_status sp_pack_verify(const uint8_t *data, size_t len, sp_verify_scratch *scratch,
                         sp_index_entry *table, size_t capacity);

/* ---------------------------------------------------------------------------------------------
 * Waveforms (docs/waveform.md): which sample sounds for which keys and velocities, and how. A
 * waveform file is valid only in canonical form; its id is a SHA-256 of its body. These calls are
 * independent of the pack calls above.
 */

#define SP_WAVEFORM_MAX_BYTES 73748 /* largest file */
#define SP_WAVEFORM_MAX_SAMPLES 1024
#define SP_WAVEFORM_MAX_BANKS 1024

/* One key bank. Same field offsets as in the file (40 bytes; the gaps are its reserved bytes). */
typedef struct sp_key_bank {
    uint8_t key_lo, key_hi;          /* 0..127, key_lo <= key_hi */
    uint8_t vel_lo, vel_hi;          /* 1..127, vel_lo <= vel_hi */
    uint8_t root;                    /* 0..127: the key at which the sample plays at its own rate */
    int8_t transpose;                /* semitones, -127..127 */
    uint8_t loop_mode;               /* 0 none, 1 forward */
    uint16_t sample;                 /* index into the sample ids */
    int32_t tune;                    /* 1/100 cent, within +-1,200,000 */
    int32_t level;                   /* 1/100 dB, -14,400..4,800 */
    int16_t pan;                     /* 1/100 of -100..100, within +-10,000 */
    uint32_t start, end;             /* frames; end exclusive and greater than start, or 0 for the end of the sample */
    uint32_t loop_start, loop_end;   /* frames, loop_end exclusive; both 0 without a loop */
} sp_key_bank;

/* Check a waveform file against every rule of the specification; *samples and *banks receive
 * S and K (0 on failure). */
sp_status sp_waveform_check(const uint8_t *data, size_t len, size_t *samples, size_t *banks);

/* Sample id i (< S) and key bank i (< K, in canonical order) of a file that has passed
 * sp_waveform_check. Only the header and the length are checked again (constant time); an index
 * out of range is SP_ERR_INVALID_ARGUMENT. */
sp_status sp_waveform_sample(const uint8_t *data, size_t len, size_t i, uint8_t id[SP_SAMPLE_ID_BYTES]);
sp_status sp_waveform_bank(const uint8_t *data, size_t len, size_t i, sp_key_bank *bank);

/* The waveform id (after the checks of sp_waveform_check). A reader given a waveform under an id
 * compares it with this and refuses a mismatch. */
sp_status sp_waveform_id(const uint8_t *data, size_t len, uint8_t id[SP_SAMPLE_ID_BYTES]);

/* Write the canonical file for banks[0..count), whose `sample` fields index ids[0..id_count)
 * (any order; duplicates and unused ids are allowed), into out[0..capacity). The banks are
 * sorted and the ids put in order of first use in out; the inputs are not changed. *written is
 * the file's length, or the length needed with SP_ERR_BUFFER_TOO_SMALL; 20 + 72 x count bytes
 * are always enough. Equal banks give SP_ERR_WF_DUPLICATE_BANK, a field out of range its
 * SP_ERR_WF_* code; out[0..capacity) is unspecified after an error. out must not overlap the
 * inputs. */
sp_status sp_waveform_build(const sp_key_bank *banks, size_t count, const uint8_t (*ids)[SP_SAMPLE_ID_BYTES],
                            size_t id_count, uint8_t *out, size_t capacity, size_t *written);

#ifdef __cplusplus
}
#endif

#endif /* SAMPLEPACK_H */
