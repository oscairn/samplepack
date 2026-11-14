<!-- SPDX-License-Identifier: MIT -->
# Oscairn Sample Pack (OSP) format, version 1.0

An Oscairn Sample Pack (short name OSP, file extension `.osp`) is a file of audio samples, each
stored as one standard FLAC stream with a small header and an uncompressed preload window, plus
the entries that make the file appendable and recoverable. This document is normative: it is
meant to be enough to write a second, independent reader. "Must", "must not" and "may" bind
writers and readers. The reference implementation is `crates/samplepack` (module `pack`), its C
interface `include/samplepack.h`, and the `samplepack` command-line tool.

Contents: 1 Terms · 2 Conventions · 3 Sample id · 4 File layout · 5 Entries · 6 Records ·
7 Tombstones · 8 Trailer and footer · 9 Live set and index · 10 Writing · 11 Opening and torn
tails · 12 Security rules for readers · 13 Verification · 14 Forward compatibility · 15 Limits ·
16 Copying records · 17 Ordering and locality · 18 Export and import · 19 Error codes ·
20 Worked examples · Appendix A: what the host provides · Appendix B: departures from the draft.

## 1. Terms

| Term | Meaning |
| --- | --- |
| Sample | One piece of decoded audio: frames of 16-bit PCM at one rate with one or two channels. |
| Frame | One sample instant across all channels (FLAC calls a block of these a "block"; a coded block is a "FLAC frame"). |
| Sample id | 32 bytes, the SHA-256 in section 3. The one name of a sample anywhere. |
| Record | One sample stored self-contained: header, credit, frame table, preload window, FLAC frames. |
| Entry | A page-aligned unit of the file: the pack header, a record, a tombstone, a trailer, or an entry type a later minor version adds. |
| Trailer | An entry listing every live record; its last 32 bytes are the footer. The newest trailer marks the committed state. |
| Live | A record that the committed state includes. |
| Committed length | The length of the file up to the end of the newest valid trailer. Bytes after it are not part of the pack. |

## 2. Conventions

- All integers are unsigned and little-endian unless stated. Offsets and lengths are in bytes.
- The page size of the format is P = 4,096. Every entry starts at a multiple of P from the start
  of the file and its length is a multiple of P, so every committed pack is a multiple of P long.
- Reserved fields and padding are written as zero (but see section 14 for newer minor versions).
- **Checksum: CRC-32C** (Castagnoli; reflected polynomial 0x82F63B78, initial value 0xFFFFFFFF,
  final XOR 0xFFFFFFFF; the check value of the ASCII string `123456789` is 0xE3069283). Chosen
  over a truncated SHA-256 because the checksums detect torn writes and accidental damage, not
  adversaries: security never depends on them (section 12), identity uses SHA-256 (section 3).
  CRC-32C detects every burst of up to 32 bits and is the checksum of comparable storage formats;
  it costs a table lookup per byte where SHA-256 costs about twenty times more, and it is a few
  lines to implement without dependencies.
- MD5 (RFC 1321) appears only as the decoded-audio signature that a standard FLAC file carries
  in STREAMINFO. It is never used for identity or integrity decisions.

## 3. Sample id

The sample id is SHA-256 (FIPS 180-4) over this byte string:

| Bytes | Content |
| --- | --- |
| 17 | the domain tag `oscairn-sample-1` followed by one zero byte |
| 4 | sample rate in Hz, u32 |
| 1 | channels: 1 or 2 |
| 1 | bits per sample: 16 |
| 2 x channels x frames | every frame of the decoded audio in order, channels interleaved (left then right), each sample signed 16-bit little-endian |

- The id covers the decoded audio, the rate and the channel count, and nothing else: not names,
  loop points, the preload window, encoder settings, the FLAC bytes, the MD5, the credit, or where
  the record is stored. Two encodings of the same audio have the same id. The same PCM at another
  rate, or read as a different channel count, is a different sample.
- **Narrowest exact bit depth.** Audio must be stored, and hashed, at the narrowest bit depth the
  format version allows at which it is exact. Version 1 allows only 16 bits: 16-bit audio is
  stored as it is; audio of a greater depth whose extra low bits are all zero must be reduced to
  16 bits; other audio cannot be stored in version 1. A later version that adds wider depths must
  keep hashing audio that is exact at 16 bits as 16-bit, so one piece of audio never gets two ids.
- The domain tag makes a sample id differ from the SHA-256 of any ordinary file.
- A reader must never trust an id that a file states. A device accepts a record as the sample it
  names only after full verification (section 13), which recomputes the id from the decoded audio.

## 4. File layout

```
offset 0       pack header            1 page
offset 4096    entry                  records, tombstones, trailers, in the order written
...            entry
...            trailer + footer       the newest trailer ends the committed pack
```

Entries tile the file without gaps: each entry starts where the previous one ends.

## 5. Entries

Every entry starts with the same 24-byte prefix. Its layout is frozen for all versions, so any
reader can find, check and skip any entry.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 4 | magic, four ASCII bytes (table below) |
| 4 | 2 | major version: 1 |
| 6 | 2 | minor version: 0 for entries written by this version |
| 8 | 8 | length of the entry, a multiple of P, at least P |
| 16 | 4 | checked length C: bytes from the start of the entry covered by the checksum; 24 <= C <= length |
| 20 | 4 | CRC-32C of bytes [0, C) of the entry, computed with bytes 20 to 23 taken as zero |

| Magic | Entry |
| --- | --- |
| `OSPH` | pack header (only at offset 0) |
| `OSPR` | record |
| `OSPT` | tombstone |
| `OSPI` | trailer |

The footer (magic `OSPF`) is not an entry; it is the last 32 bytes of a trailer.

**Pack header.** One page: the prefix with magic `OSPH`, length P, C = 24, and zeros to the end of
the page. Its minor version is the minor version of the pack.

## 6. Records

### 6.1 Layout

```
[0, H)          header (128 bytes), credit, frame table, zero padding     H = C rounded up to P
[H, H+W)        preload window, zero padding                              W = window bytes rounded up to P
[H+W, H+W+F)    FLAC frames: one stream                                   F = frame bytes
[H+W+F, length) zero padding                                              length = (H+W+F) rounded up to P
```

Because the record starts on a page and H is a multiple of P, the window starts on a page
boundary of the file. Everything a voice needs before its first streamed block, the header and
the window, is the contiguous span [record start, record start + H + W): the record's preload span.

### 6.2 Header

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | 24 | prefix | magic `OSPR`; length = record length; C = end of the frame table |
| 24 | 4 | H: head length | = C rounded up to P |
| 28 | 4 | W: window region bytes | = window_frames x channels x 2 rounded up to P (0 when window_frames is 0) |
| 32 | 32 | sample id | section 3 |
| 64 | 16 | MD5 of the decoded audio | MD5 of the same PCM bytes the id hashes after its 23-byte prefix (FLAC's STREAMINFO MD5) |
| 80 | 4 | sample rate, Hz | 1 to 1,048,575 |
| 84 | 1 | channels | 1 or 2 |
| 85 | 1 | bits per sample | 16 |
| 86 | 2 | flags | 0 (reserved) |
| 88 | 4 | frames | at least 1 |
| 92 | 4 | window_frames | see 6.4 |
| 96 | 4 | block size B | 256, 512, 1024, 2048 or 4096 |
| 100 | 4 | blocks N | = ceil(frames / B) |
| 104 | 4 | F: frame bytes | total bytes of FLAC frames |
| 108 | 4 | smallest FLAC frame, bytes | = the smallest difference in the frame table |
| 112 | 4 | largest FLAC frame, bytes | = the largest difference in the frame table |
| 116 | 2 | credit length L | at most 1,024 |
| 118 | 2 | reserved | 0 |
| 120 | 8 | reserved | 0 |
| 128 | L | credit | UTF-8 text, not interpreted (6.3) |
| 128 + L rounded up to 4 | 4 x (N + 1) | frame table | u32 offsets into the frames: entry i is where FLAC frame i starts; entry N is F. Entry 0 is 0 and the entries strictly increase. Bytes between the credit and the table are zero. |

The checked length C of a minor-0 record is exactly the end of the frame table: the checksum
covers the header, the credit and the table; padding, window and frames are outside it (the FLAC
frames carry their own CRCs, and the window is checked against the frames at full verification).

### 6.3 Credit

Up to 1,024 bytes of UTF-8, for example `Piano samples: <author>, CC BY 3.0, <url>`. The credit is
carried with the record through every copy, so attribution survives. It is not part of the
identity, and it never limits what may be done with the record: the format has no redistribution
marks, flags or restrictions of any kind.

### 6.4 Preload window

The window holds frames [0, window_frames) of the decoded audio, interleaved 16-bit signed
little-endian, uncompressed. A writer chooses a window setting S: 0 or a multiple of 1,024 frames
up to 16,384; the default is 4,096. The record stores window_frames = min(frames, S). So
window_frames is one of:

- 0 (no window; W = 0);
- frames, when the whole sample fits (frames <= 16,384);
- a multiple of 1,024, at most 16,384, less than frames, and a multiple of B, so that streaming
  continues at block window_frames / B, which starts a whole FLAC frame.

The window is derived data: it equals what decoding the first frames gives. Changing the window
means rewriting the record (new H, W and length); a byte copy never changes it.

### 6.5 FLAC frames

The frames are one standard FLAC stream (RFC 9639) without the `fLaC` marker and metadata,
restricted to the subset in `docs/decoder.md`: 16-bit, the record's channel count, constant,
verbatim, fixed (order 0 to 4) or LPC (order 1 to 8) subframes, Rice or Rice2 residuals. In
addition:

- fixed blocking strategy (frame header bit 0 of byte 1 clear);
- frame i holds exactly B frames, except the last, which holds frames - (N - 1) x B;
- the coded frame number of frame i is i, in its shortest form;
- the frame header's sample-rate code is 0 ("from STREAMINFO") or codes the record's rate;
- each frame exactly fills its table slot [table[i], table[i+1]);
- frame CRC-8 and CRC-16 are correct.

A record's header plus its frames therefore make a standard `.flac` file without any re-encoding:
`fLaC`, one STREAMINFO block (last-block flag set; min and max block size B; min and max frame
size from the header; rate, channels, bits and frames; the MD5) and then the frames byte for byte.
The `samplepack extract` command writes exactly this, and the stock `flac -t` accepts it.

Loop points are not in the record. They belong to the sounds that use the sample; a record is
always one stream starting at frame 0, whatever loops its users choose.

## 7. Tombstones

One page: the prefix with magic `OSPT`, length P, C = 56, the sample id at offset 24, zeros after.
It removes the record with that id from the live set. The record's bytes stay in place until the
pack is compacted.

## 8. Trailer and footer

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 24 | prefix: magic `OSPI`; C = 32 + 56 x count; length = (C + 32) rounded up to P |
| 24 | 4 | count: live records listed, at most 1,048,576 |
| 28 | 4 | reserved, 0 |
| 32 | 56 x count | one entry per live record, in increasing offset (file order) |
| C | ... | zero padding |
| length - 32 | 32 | footer |

Trailer entry (also the layout of an index entry in the C interface, `sp_index_entry`):

| Offset | Size | Field | Rule |
| --- | --- | --- | --- |
| 0 | 32 | sample id | |
| 32 | 8 | record offset | multiple of P, at least P, at or after the end of the previous entry's record |
| 40 | 8 | record length | multiple of P, at least P; offset + length <= trailer offset |
| 48 | 8 | preload | H + W of the record: the span a loader reads first; multiple of P, P <= preload <= length |

Footer:

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 4 | magic `OSPF` |
| 4 | 4 | reserved, 0 |
| 8 | 8 | trailer offset |
| 16 | 8 | trailer length (= the trailer entry's length) |
| 24 | 4 | reserved, 0 |
| 28 | 4 | CRC-32C of footer bytes [0, 28) |

The footer of the newest trailer is the last 32 bytes of the committed pack, so a reader finds the
trailer from the end of the file.

## 9. Live set and index

- **Committed state.** The pack is what its newest trailer says. A trailer lists every live record,
  and lists each sample id at most once (rule S-1: one live record per sample id per pack).
- **Rebuilding from the entries alone.** The live set is also defined by the entries: walk them in
  file order from offset P; a record adds its id, a tombstone removes its id; trailers and unknown
  entry types change nothing. A record whose id is already live, or a tombstone whose id is not
  live at that point, makes the pack invalid (`DUPLICATE_ID`, `NOT_LIVE`). The live set after the
  last entry must equal the newest trailer's list (`TRAILER_MISMATCH`). This is how the index is
  rebuilt when a reader does not want to rely on the trailer, and what full verification checks.
- **The index** maps sample id to (offset, length, preload). The library builds it in caller
  memory: `index` copies the trailer's entries and sorts them by id (lexicographic byte order);
  `rebuild_index` replays the entries as above. Lookup is a binary search. The index is a cache;
  it is never the only copy of anything.
- **Across packs.** A library may span several packs. On one device each sample id should have one
  live record in all packs together: a writer looks the id up in the device's index first and
  skips a record already present (section 18).

## 10. Writing

### 10.1 Creating a pack

Write the pack header, the records (and tombstones, if any) in the chosen order, sync, write the
trailer, sync. A new pack is written under a temporary name and renamed into place after the
second sync; the directory is then synced.

### 10.2 Appending (install, import, combining, removing)

1. If the file has an uncommitted tail (section 11), truncate it to the committed length and sync.
2. Write the new entries at the committed length: tombstones first, then records. Their offsets
   are known before writing: each entry starts where the previous one ends.
3. Sync the data (`fdatasync`; on Windows `FlushFileBuffers`).
4. Write one new trailer listing the new live set (old live records minus the tombstoned ids,
   plus the new records), in file order, after the new entries.
5. Sync again. Only now is the append committed; only now may the host update its index and tell
   the engines.

A crash before step 5 completes leaves an uncommitted tail; a crash can never lose a committed
record. An append is refused before anything is written if a new record's id is already live,
if a tombstone's id is not live, or if the pack would exceed the size limit. Engines that have
mapped the pack keep reading committed records while it grows, and remap to see new ones.

### 10.3 Removing and compacting

Removing appends tombstones and a trailer. Compaction copies every live record, byte for byte, in
a chosen order, into a new pack (10.1) and replaces the old one once no reader maps it. Old
trailers, removed records and tombstones are the dead bytes compaction reclaims. When to compact
is host policy (Appendix A).

### 10.4 Pack size

A pack must not exceed 1 GiB (2^30 bytes). A writer starts a new pack when an append would take
the current one past that; a record that cannot fit in an empty pack (with its header and a
trailer) is refused. A library therefore occupies one or more packs. Splitting a pack is copying
records into another; combining is copying records into one pack; an export of a subset is a pack
holding the subset. All use the same format.

### 10.5 Library writer interface

The library does no I/O and no allocation: `write_pack_header`, `write_record`,
`write_tombstone` and `write_trailer` serialise into caller buffers (`record_len` and
`trailer_len` give the sizes). `write_record` decodes the given frames (CRCs checked) to fill the
window and compute the MD5 and the sample id, then verifies the finished record in full. The
caller writes the buffers at the offsets above and performs the syncs.

## 11. Opening and torn tails

**Open** (`Pack::open`, `sp_pack_open`) on the file's bytes:

1. The file is at least P long and starts with `OSPH`; else `NOT_PACK`. Major version 1; else
   `UNSUPPORTED_VERSION`. The header's prefix and checksum are valid.
2. Length at most 1 GiB; else `TOO_LARGE`. A length of exactly P is an empty pack.
3. The length is a multiple of P and the last 32 bytes are a footer with a valid checksum; else
   `NO_FOOTER`. The footer's fields agree (trailer offset a multiple of P, at least P, trailer
   offset + trailer length = file length; else `BAD_FOOTER`).
4. The trailer has magic `OSPI` (else `BAD_FOOTER`), major 1, a valid prefix and checksum, and a
   count consistent with its lengths (`BAD_TRAILER`).
5. Every trailer entry is aligned and inside [P, trailer offset), and the entries are in file order
   without overlap (`BAD_OFFSET`, `OVERLAP`).

Record headers are checked when a record is used (section 13).

**Torn tail.** `NO_FOOTER` means the file has an uncommitted or torn tail. The committed length is
the largest multiple of P, L <= file length (and <= 1 GiB), such that the first L bytes open as
above; if there is none, L = P (an empty pack). Searching backwards from the end of the file,
page by page, finds it after at most the pages of the torn append. `recover` /
`sp_pack_recover` returns L.

- A writer truncates the file to L before appending (10.2).
- A read-only reader (an engine) opens the first L bytes and ignores the rest.
- Nothing may be truncated if `recover` returns an error: then the header is invalid, or a prefix
  ending in a well-formed footer is invalid in a way a torn write cannot cause (for example
  overlapping records): the pack is damaged, not torn, and must be re-verified or reinstalled.
- Bytes after L, even well-formed records, are not part of the pack: an append is all or nothing.

## 12. Security rules for readers

The reader is the single parser of untrusted pack files for its host and plugin. It must:

- check every offset, length and count taken from the file against the file length and against
  each other before using it; compute every sum and product with overflow checks (or in a width
  where overflow is impossible given the limits already checked);
- never read outside the mapped length (the reference reader is fuzzed with a guard page directly
  after the mapping);
- reject overlapping records and trailers that do not list records in file order;
- treat every stated value (sample id, MD5, window, frame table, lengths) as a claim until checked;
- never let a checksum stand in for a bounds check: checksums detect damage, bounds checks give
  safety;
- decode FLAC frames only with a decoder whose reads are bounded by the frame slice (the subset
  decoder in this library is);
- treat a structure passed back by its caller (`sp_record_info`) as untrusted too: the library
  re-checks it against the pack before every use.

The reference library is `no_std`, allocation-free, `unsafe`-free (the C interface excepted), and
panic-free: a build whose panic handler cannot link still links (`tools/check-no-panic.sh`).

## 13. Verification

| Level | What is checked | When |
| --- | --- | --- |
| Open | section 11 | every time a pack is opened |
| Record access | prefix and checksum, every header field and rule of 6.2 and 6.4, UTF-8 credit, frame table increasing with the stated smallest and largest frame, and agreement with the index entry (id, length, preload; `MISMATCH`) | every time a record is first used (`Pack::record`, `sp_pack_record`) |
| Full, per record | record access, plus: all padding zero; every frame decoded with CRC-8 and CRC-16 checked and the rules of 6.5; window equal to the decoded frames; MD5 and sample id recomputed and equal | before a record is installed or imported, before an install may skip a sample by id against it, and on explicit maintenance (`verify_record`, `sp_record_verify`) |
| Full, per pack | open, every entry well formed with zero padding, the rebuilt live set equal to the trailer, and full verification of every live record | after writing a pack, when a pack's identity guard fails (Appendix A), on explicit maintenance (`verify_pack`, `sp_pack_verify`, `samplepack verify --full`) |

At play time a loader decodes without CRC checks: on the target, the subset decoder costs about
27 to 30 instructions per sample, and checking frame CRCs while decoding costs about 23% more.
That cost is paid once, at install or verification. A frame that fails at play time (a decoding
error) gives silence for that block, never an out-of-bounds read.

## 14. Forward compatibility

- **Major version.** A reader supports major version 1 only. A pack header or a known entry type
  with another major version is refused (`UNSUPPORTED_VERSION`). Incompatible changes bump the
  major version of the pack header and of every entry.
- **Minor versions** only add things a version-1.0 reader may ignore. An entry of minor version
  greater than the reader's may use its reserved fields and padding, and may have a checked length
  larger than this document gives (new fields after the ones defined here, before H); the reader
  ignores them. An entry of minor version 0 must have exactly the checked length defined here
  and zero reserved fields (`RESERVED_NONZERO`, `BAD_LAYOUT`, `BAD_ENTRY`, `BAD_TRAILER`).
- **New entry types** may be added by a minor version. They have the frozen prefix, so a reader
  checks their checksum and skips them by length. A new entry type must not change the meaning of
  the entry types defined here; such a change needs a new major version.
- **Copies** keep the record's own minor version: a record from a newer-minor pack copied into an
  older pack is still read as its own version.
- A reader must not truncate a pack because of entries it does not understand: only the torn-tail
  rule of section 11 allows truncation.

## 15. Limits

| Item | Limit |
| --- | --- |
| Pack size | 1 GiB (readers reject larger packs) |
| Live records per trailer | 1,048,576 |
| Offsets and lengths in trailers and footers | u64 (bounded in practice by the pack size) |
| Frames per sample | u32, at least 1 |
| Frame bytes, frame table offsets | u32 |
| Block size | 256, 512, 1,024, 2,048 or 4,096 frames (the reference writer uses 1,024) |
| Preload window | 0, or multiples of 1,024 frames up to 16,384, or the whole sample if shorter |
| Credit | 1,024 bytes of UTF-8 |
| Channels, bits | 1 or 2 channels, 16 bits |
| Sample rate | 1 to 1,048,575 Hz |

## 16. Copying records

A record refers to nothing outside itself: no pack offset, no neighbouring entry. Copying a record
to another pack is a byte-for-byte copy with no fix-ups; only the destination's trailer gets a new
entry (with the new offset). Every operation that moves samples (export, import, combine, copy,
compact) copies records this way. Re-encoding or changing the window makes a new record of the
same sample id.

## 17. Ordering and locality

Measured on the target device: sequential reads run at about 167 MB/s, and a random 16 KiB read
costs about 0.59 ms (p95 about 1.0 ms). An 854-sample piano whose preload spans are read one by
one, cold, takes about 0.5 s. The format makes two things cheap; writers and loaders should use
both:

1. **Adjacency.** A writer appends records in the order it is given, and compaction takes an
   order. Hosts should give the order in which sounds use their samples (for example a library's
   manifest order, or one sound's samples together, by key and velocity), so one sound's records
   are adjacent.
2. **Batched, file-ordered preload.** Before the first note, a loader should:
   - take the index entries of every record the sound uses and sort them by offset;
   - merge spans whose gap is smaller than what one random read costs at sequential speed
     (0.59 ms x 167 MB/s, about 100 KB on the measured device);
   - issue read-ahead for all resulting spans at once, in file order (`posix_fadvise` or
     `madvise` with `WILLNEED`; on Windows `PrefetchVirtualMemory`), on a mapping advised
     `MADV_RANDOM` so that page faults elsewhere do not read ahead whole megabytes;
   - only then touch the windows.

   The trailer carries each record's preload span, so all hints can be issued from the index alone,
   without reading any record header first.

`samplepack locality PACK [ID...]` reports, for a set of records, the preload bytes, the file span,
the gaps between preload spans, and the cold-read cost under this device model (one read per
record; reads merged as above; one sequential read of the span). It is a model, not a measurement.

## 18. Export and import

These rules concern the format; the archive and package mechanics are the host's (Appendix A).

- **E-1. What an export carries.** By default an export embeds every sample that the exported
  sounds use, and only those: never the rest of a source pack or package. A mode that records a
  package dependency instead of embedding may exist only as a non-default option of the exporter;
  it is outside this library.
- **E-2. Form.** Each embedded sample is a one-record pack (pack header, the record copied byte for
  byte, a one-entry trailer) at `Assets/samples/<64 lower-case hex digits of the sample id>.osp`.
  No re-encoding and no decoding. One file per record keeps every archive entry small, lets an
  importer skip a file by its name before reading it, and makes each sample one content-addressed
  piece in a package repository. One-record packs are ordinary packs: `samplepack combine` joins
  them.
- **E-3. Credit.** An exporter may collect the distinct credit strings of the records it carries
  into a credits file. The credit is also inside every record.
- **E-4. No restrictions.** Nothing in the format marks a sample as restricted, non-redistributable
  or otherwise limited, and an exporter must not refuse a sample because of where it came from.
- **I-1. Skip by id.** An importer may skip a sample entry, without reading or fetching it, when a
  record with that sample id is already installed on the device (from any archive or package)
  and verified (section 13); it must check that the installed record's rate, channel count and
  frame count are those the importing content expects.
- **I-2. Otherwise verify.** The file opens as a pack, holds exactly one live record, the record
  passes full verification, and the recomputed sample id equals both the header's and the file
  name's. Any failure refuses the import.
- **I-3. Append** the verified records byte for byte to a device pack (10.2), one trailer for the
  whole import.

## 19. Error codes

The codes are stable and are the values of `sp_status` (`include/samplepack.h`). Codes 1 to 12
are shared with the FLAC subset decoder (`docs/decoder.md`); when a frame fails to decode, its
decoder code is returned.

| Code | Name | Meaning |
| --- | --- | --- |
| 1 | `INVALID_ARGUMENT` | bad pointer or argument; inconsistent writer input; a damaged `sp_record_info` |
| 2 | `BUFFER_TOO_SMALL` | an output or index buffer is too small (the needed count is reported) |
| 32 | `NOT_PACK` | shorter than one page, or no `OSPH` magic |
| 33 | `UNSUPPORTED_VERSION` | major version other than 1 |
| 34 | `NO_FOOTER` | no valid footer at the end: uncommitted or torn tail (section 11) |
| 35 | `TOO_LARGE` | larger than 1 GiB |
| 36 | `BAD_CHECKSUM` | an entry's CRC-32C does not match |
| 37 | `BAD_ENTRY` | malformed prefix: length or checked length out of range, wrong magic for the place, malformed tombstone |
| 38 | `BAD_FOOTER` | footer checksum good but fields inconsistent, or it does not point at a trailer |
| 39 | `BAD_TRAILER` | trailer count and lengths inconsistent |
| 40 | `BAD_OFFSET` | offset, length or preload unaligned or out of range |
| 41 | `OVERLAP` | records overlap or are not in file order |
| 42 | `RESERVED_NONZERO` | a reserved field is not zero (minor version 0) |
| 43 | `BAD_FORMAT` | rate, channels, bits or block size outside version 1 |
| 44 | `BAD_LAYOUT` | H, W, record length, window frames, block count or frames not as required |
| 45 | `BAD_FRAME_TABLE` | frame table not increasing from 0 to F, or smallest/largest frame wrong |
| 46 | `BAD_CREDIT` | credit longer than 1,024 bytes or not UTF-8 |
| 47 | `DUPLICATE_ID` | two live records with one sample id |
| 48 | `NOT_LIVE` | a tombstone for an id that is not live |
| 49 | `MISMATCH` | a record disagrees with the index entry that points at it |
| 50 | `NOT_FOUND` | no record with that id in the index |
| 51 | `FRAME_MISMATCH` | a frame's block size, channels, number, rate or length disagrees with the record |
| 52 | `PADDING` | padding not zero |
| 53 | `WINDOW_MISMATCH` | the window differs from the decoded frames |
| 54 | `MD5_MISMATCH` | MD5 of the decoded audio differs |
| 55 | `ID_MISMATCH` | sample id of the decoded audio differs |
| 56 | `TRAILER_MISMATCH` | the live set rebuilt from the entries differs from the trailer |
| 57 | `NOT_RECORD` | the entry at that offset is not a record |

## 20. Worked examples

**A. A sample id.** Four mono frames at 44,100 Hz with the values 0, 1, -1, 32767. The hashed bytes
(31 bytes) are

```
6f73636169726e2d73616d706c652d3100   "oscairn-sample-1\0"
44ac0000                             44100
01 10                                1 channel, 16 bits
0000 0100 ffff ff7f                  0, 1, -1, 32767
```

and the sample id is `1e272d15841dbddadd5503dec6a3ac43020b40c9c0a730cc5cd2f66101301338`.

**B. A one-record pack.** A 1,500-frame mono sine tone at 44,100 Hz, packed with
`samplepack pack --window 1024 --credit "Example tone, CC0"`. The file is 20,480 bytes: the pack
header (page 0), a 12,288-byte record (pages 1 to 3) and a trailer (page 4).

```
000000: 4f 53 50 48 01 00 00 00 00 10 00 00 00 00 00 00   OSPH, major 1, minor 0, length 4096
000010: 18 00 00 00 c2 80 f6 9d 00 00 00 00 00 00 00 00   checked 24, CRC-32C; zeros to 0x1000
001000: 4f 53 50 52 01 00 00 00 00 30 00 00 00 00 00 00   OSPR, length 12288
001010: a0 00 00 00 77 e5 26 c9 00 10 00 00 00 10 00 00   checked 160, CRC, H 4096, W 4096
001020: f2 94 0a 6c 3b 66 9b dc 16 4e e8 05 71 1a ca 0e   sample id ...
001030: 0a a8 9d 2f 35 e3 be 48 bf c4 6a bf a8 39 51 5c
001040: 9a cb 5f 46 13 d6 3a ca 69 6e a6 09 33 06 79 08   MD5 of the decoded audio
001050: 44 ac 00 00 01 10 00 00 dc 05 00 00 00 04 00 00   44100 Hz, 1 ch, 16 bit, flags 0, 1500 frames, window 1024
001060: 00 04 00 00 02 00 00 00 6e 02 00 00 ce 00 00 00   block 1024, 2 blocks, 622 frame bytes, smallest 206
001070: a0 01 00 00 11 00 00 00 00 00 00 00 00 00 00 00   largest 416, credit 17 bytes, reserved
001080: 45 78 61 6d 70 6c 65 20 74 6f 6e 65 2c 20 43 43   "Example tone, CC0", padded to 20 bytes
001090: 30 00 00 00 00 00 00 00 a0 01 00 00 6e 02 00 00   frame table at 148: 0, 416, 622; C = 160
002000: (window: frames 0 to 1023, 2048 bytes, then zeros to 0x3000)
003000: (FLAC frames, 622 bytes, then zeros to 0x4000)
004000: 4f 53 50 49 01 00 00 00 00 10 00 00 00 00 00 00   OSPI, length 4096
004010: 58 00 00 00 c7 bc 22 54 01 00 00 00 00 00 00 00   checked 88, CRC, count 1, reserved
004020: f2 94 0a 6c 3b 66 9b dc 16 4e e8 05 71 1a ca 0e   entry: sample id ...
004030: 0a a8 9d 2f 35 e3 be 48 bf c4 6a bf a8 39 51 5c
004040: 00 10 00 00 00 00 00 00 00 30 00 00 00 00 00 00   offset 4096, length 12288
004050: 00 20 00 00 00 00 00 00 00 00 00 00 00 00 00 00   preload 8192 (H + W)
004fe0: 4f 53 50 46 00 00 00 00 00 40 00 00 00 00 00 00   OSPF, trailer at 16384
004ff0: 00 10 00 00 00 00 00 00 00 00 00 00 22 21 24 17   trailer length 4096, CRC-32C of the footer
```

A loader reads the preload span [4096, 12288): header and window in one 8 KiB read. Block 1
(frames 1024 to 1499) is FLAC frame 1 at 0x3000 + 416.

**C. An append and a torn tail.** The pack above gets a second record (another 12,288 bytes) and,
in the same append, a tombstone for the first. The writer writes the tombstone at 20,480, the
record at 24,576, syncs, writes a trailer listing only the new record at 36,864 and syncs: the
pack is 40,960 bytes. If power fails before the second sync completes, the file may end anywhere
after 20,480, or contain the trailer without a valid footer. Opening finds no valid footer at
the end; the backward search finds the footer at 20,480 - 32 valid, so the committed length is
20,480 and the pack is exactly the one-record pack of example B. The test suite truncates such an
append at every byte position and checks this (`tests/pack.rs`, `torn_tail_recovers_at_every_byte`).

**D. One sample, several sounds; an export.** Two sounds and a Performance use the same piano note.
The device holds one record; all three resolve its id through the index to the same offset and
share one mapped window. Exporting the Performance embeds that record (and every other sample the
Performance's sounds use) as `Assets/samples/<id>.osp`. A recipient that already has the id
installed and verified skips the file; one that does not verifies and appends it.

## Appendix A (non-normative): what the host provides

The format and library leave these to the host:

- **Asset store and device index.** Which packs exist, which pack holds which id, and the choice of
  pack to append to; a persistent index is a cache rebuilt from trailers.
- **Identity guard.** The host keeps, per pack, what it verified (size, modification time, file id).
  While they match, installed records count as verified for skip-by-id; otherwise the pack is fully
  verified again first.
- **Liveness.** Which samples are referenced (library items, open and saved Performances, installed
  asset libraries), worked out from explicit roots, not stored reference counts; removal appends
  tombstones only for unreferenced samples; deleting a pack with no live records.
- **Compaction policy.** For example when dead bytes are at least 25% of a pack and at least
  64 MiB, the device is not playing, and there is free space for the live bytes; replacing a pack
  only when no engine maps it (on Windows, at the next start).
- **Package pieces.** In a package repository each `Assets/samples/*.osp` should be its own
  content-addressed piece whatever its size (a new rule for the package index: today small files
  share one archive piece), so updates download only new samples and packages share pieces.
- **Dependencies and credits in manifests.** Package dependencies for the non-default
  dependency-instead-of-embedding export mode, and structured attribution, are manifest features.
- **Loop windows** for looped regions are built by the engine in memory; loop points never enter
  the format.

## Appendix B (non-normative): departures from the design draft

| Draft | This specification | Why |
| --- | --- | --- |
| `.opack`; magics `OSPK`, `OSR1`, `OST1`, `OSI1`, `OSF1` | `.osp`; magics `OSPH`, `OSPR`, `OSPT`, `OSPI`, `OSPF` | the owner named the format OSP |
| Per-type CRC-32 at type-specific offsets | one frozen 24-byte prefix with CRC-32C for every entry | one scan and checksum rule for all entry types; unknown types can be skipped safely |
| `split` field for two-stream loops | none | the owner ruled out loop splitting: one stream per record |
| 32-byte footer after the trailer | footer is the last 32 bytes of the trailer entry | every entry and every committed pack stays a multiple of 4,096 |
| Trailer sorted by id: {id, offset} | trailer in file order: {id, offset, length, preload}; the caller's index is sorted by id | overlap and order checked in one pass without memory; preload hints issued from the trailer alone |
| Recovery by scanning forward to the last good trailer | backward search for the largest prefix that opens; forward replay for rebuilding and verifying | damage in the middle of a pack cannot make recovery truncate committed records |
| 1 GiB cap as a writer rule | readers also reject packs over 1 GiB | one limit, enforced on both sides |
| Records on a 1,024-frame grid | block size field: 256 to 4,096 (powers of two), window a multiple of it | the task required a block size field; the reference writer uses 1,024 |
| Export: dependency by default, "Embed all" option | embed every used sample by default; dependency only as a non-default option | the owner's ruling |
| "LPC > 4 is a newer format" (failure table) | LPC up to order 8 is version 1 | the subset decoder accepts order 8 |
| Credit: bytes | credit must be UTF-8 | it is displayed text; a reader can rely on it |
