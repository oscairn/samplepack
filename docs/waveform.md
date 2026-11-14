<!-- SPDX-License-Identifier: MIT -->
# Oscairn Waveform format, version 1

A waveform is a library item that says how a set of samples is played across the keyboard: which
sample sounds for which keys and velocities, at what root key, tuning, level and pan, and where it
starts, ends and loops. A sound names a waveform and adds its own limits, tuning, level and pan.
Samples stay in Oscairn Sample Packs (`docs/format.md`), unchanged: nothing about keys or loops
enters a pack.

A waveform's identity is its content. Editing a waveform makes a new waveform with a new id; sounds
that use the old one do not change. Applying an edit to every sound that uses a waveform is an
explicit action of the host that re-points the references.

This document is normative for sections 1 to 4. It is implemented by
`crates/samplepack` (module `waveform`), its C interface `include/samplepack.h` (`sp_waveform_*`),
and the `samplepack waveform` commands. The pack reader does not depend on it, and the pack format
does not change. Integers are little-endian; the checksum is the CRC-32C and the sample id is the
sample id of `docs/format.md` (sections 2 and 3).

Contents: 1 File · 2 Canonical form and the waveform id · 3 Refusals · 4 A waveform from a WAV file ·
5 How waveforms are used · 6 Worked example.

## 1. File

One waveform is one file, extension `.owf`, little-endian, at most 73,748 bytes.

| offset | size | field |
|---|---|---|
| 0 | 4 | magic `OPWF` |
| 4 | 2 | version: 1 |
| 6 | 2 | flags: 0 |
| 8 | 4 | size of the whole file in bytes |
| 12 | 4 | CRC-32C of bytes 16 to the end (the polynomial the pack uses) |
| 16 | 2 | S: number of samples, 1 to 1,024 |
| 18 | 2 | K: number of key banks, 1 to 1,024 |
| 20 | 32 x S | sample ids (the pack specification's sample id), in order of first use by the key banks |
| 20 + 32 S | 40 x K | key banks, in canonical order (section 2) |

Bytes 16 to the end are the **body**. The file has no name, credit, author or date: those belong to
the library's entry for the item, so renaming a waveform does not change its id.

### Key bank, 40 bytes

| offset | size | field |
|---|---|---|
| 0 | 1 | lowest key, 0 to 127 |
| 1 | 1 | highest key, lowest to 127 |
| 2 | 1 | lowest velocity, 1 to 127 |
| 3 | 1 | highest velocity, lowest to 127 |
| 4 | 1 | root key, 0 to 127: the key at which the sample plays at its own rate |
| 5 | 1 | transpose in semitones, signed, -127 to 127 |
| 6 | 1 | loop: 0 none (plays to its end once), 1 forward |
| 7 | 1 | 0 |
| 8 | 2 | sample: index into the file's sample ids |
| 10 | 2 | 0 |
| 12 | 4 | tune in 1/100 cent, signed, within ±1,200,000 |
| 16 | 4 | level in 1/100 dB, signed, -14,400 to +4,800 |
| 20 | 2 | pan in 1/100 of -100..100, signed, within ±10,000 |
| 22 | 2 | 0 |
| 24 | 4 | start frame: where playback begins |
| 28 | 4 | end frame, exclusive; 0 means the sample's last frame plus one |
| 32 | 4 | loop start frame |
| 36 | 4 | loop end frame, exclusive |

Frames count from 0 in the sample as its pack record stores it. With no loop, loop start and end are
0. With a loop: start <= loop start < loop end <= end (when end is not 0). Whether a frame number is
inside the sample can only be checked when the sample is at hand; a reader that has the sample
refuses a key bank that points outside it.

End is 0 or greater than start. Start and end let a sample be trimmed without re-encoding it.

## 2. Canonical form and the waveform id

A file is valid only in canonical form, so that two tools given the same key banks write the same
bytes:

1. Key banks are sorted by their 40 bytes compared as unsigned bytes from offset 0, **with the
   sample index (offsets 8 and 9) replaced by the 32-byte sample id for the comparison**. No two key
   banks may be equal under that comparison.
2. The sample ids are listed in the order the sorted key banks first use them; none unused, none
   twice.
3. Every reserved byte and the flags are 0; the size field equals the file's length; nothing follows
   the last key bank.

The **waveform id** is the SHA-256 of:

| size | bytes |
|---|---|
| 19 | the domain tag `oscairn-waveform-1` followed by one zero byte |
| size - 16 | the body: bytes 16 to the end of the canonical file |

written as 64 lower-case hexadecimal digits. The id covers the key banks and the ids of the samples
they use, and nothing else. The header's first 16 bytes are not hashed (they are derived). A reader
that is given a waveform under an id recomputes the id and refuses a mismatch.

A reader refuses: a wrong magic, version, flags, size or CRC; S or K outside its limits; a field
outside its range; lowest above highest; a sample index of S or more; an end that is neither 0 nor
greater than the start; loop points without a loop, or a loop that breaks start <= loop start < loop
end <= end (when end is not 0); any breach of the canonical form.

**Overlap is allowed.** Two key banks of one waveform may cover the same key and velocity, and a
note then starts each of them. Existing multisamples need this: they store left and right as two
key banks panned to the sides, whose root keys or key ranges can differ, so they cannot all be
stored as one stereo sample. One waveform can therefore start more than one voice for a note.

## 3. Refusals

Each refusal has its own code, a value of `sp_status` in `include/samplepack.h`. The library
checks in the order of this table, the key bank fields one key bank at a time, and reports
the first refusal that applies.

| Code | Name | Refused when |
| --- | --- | --- |
| 64 | `NOT_WAVEFORM` | the file does not start with `OPWF` |
| 67 | `WF_LENGTH` | the length is below 20 or above 73,748 bytes |
| 65 | `WF_VERSION` | the version is not 1 |
| 66 | `WF_FLAGS` | the flags are not 0 |
| 68 | `WF_SIZE` | the size field differs from the file's length |
| 70 | `WF_SAMPLE_COUNT` | S is 0 or above 1,024 |
| 71 | `WF_BANK_COUNT` | K is 0 or above 1,024 |
| 67 | `WF_LENGTH` | the length is not 20 + 32 S + 40 K (truncated, or bytes after the last key bank) |
| 69 | `WF_CHECKSUM` | the CRC-32C of the body does not match |
| 72 | `WF_RESERVED` | a reserved byte of a key bank (offsets 7, 10, 11, 22, 23) is not 0 |
| 73 | `WF_RANGE` | a key, velocity, root, transpose, loop, tune, level or pan outside its range |
| 74 | `WF_LOW_ABOVE_HIGH` | a lowest key or velocity above the highest |
| 75 | `WF_SAMPLE_INDEX` | a sample index of S or more |
| 82 | `WF_END` | end not 0 and not greater than start |
| 76 | `WF_LOOP` | loop start or end not 0 without a loop; with a loop, not start <= loop start < loop end <= end (end not 0) |
| 81 | `WF_DUPLICATE_SAMPLE` | one sample id listed twice |
| 77 | `WF_NOT_SORTED` | key banks not in canonical order |
| 78 | `WF_DUPLICATE_BANK` | two key banks equal under the canonical comparison |
| 79 | `WF_SAMPLE_ORDER` | sample ids not in order of first use |
| 80 | `WF_UNUSED_SAMPLE` | a sample id that no key bank uses |

A writer (`build`, `sp_waveform_build`)
refuses equal key banks and fields out of range with the same codes, and checks sample indices
against the ids it is given.

## 4. A waveform from a WAV file

`samplepack waveform from-wav IN.wav --pack PACK.osp --out DIR` adds the file's audio to the pack
exactly as `samplepack pack` does (same record, same sample id; nothing is added when the pack
already holds that id; the pack is created if it does not exist), and writes `DIR/<waveform id>.owf`
holding one key bank:

- keys 0 to 127, velocities 1 to 127, transpose 0, start 0, end 0, level 0, pan 0;
- **root**: the MIDI unity note of the file's `smpl` chunk (the u32 at offset 12 of the chunk's
  body), or 60 when the file has no `smpl` chunk; a note above 127 is refused;
- **tune**: the chunk's MIDI pitch fraction F (the u32 at offset 16) says that the sample sounds
  F / 2^32 of a semitone above the unity note, so tune = -round(F x 10,000 / 2^32) in 1/100 cent
  (halves round up in magnitude), from 0 to -10,000; 0 without a chunk;
- **loop**: when the chunk lists at least one loop (count at offset 28; loops of 24 bytes from
  offset 36) and the first has type 0 (forward), loop forward from its start to its end plus one
  (a WAV loop end is inclusive), which must lie inside the sample; otherwise no loop.

Tools differ in how reliably they fill the pitch fraction; a wrong tune is corrected by editing the
waveform.

A `smpl` chunk shorter than 36 bytes, or than 36 + 24 x its loop count, or running past the end
of the file, is refused, as is a forward loop whose start is after its end or whose end is not
inside the sample. Nothing is written when the input is refused.

## 5. How waveforms are used (non-normative)

- **Library item.** A host stores and exports a waveform as `Assets/waveforms/<id>.owf`. On import
  it checks the file, recomputes the id from the body and refuses a mismatch, and skips an id
  already installed, as for samples. Display name and credit live in the library's entry.
- **Dependencies.** A waveform's samples are the S ids at offset 20; a host reads them without a
  parser beyond the two counts. Export and garbage collection go sound -> waveform -> samples.
- **Playing.** A player reads and validates a waveform off the audio thread when it prepares a
  sound, resolves each sample id to its pack record, and turns each key bank into regions of the
  sound. A key bank whose start is not 0 cannot use the record's preload window,
  which holds the first frames; the player decodes its own window from the start frame when it
  prepares the sound. An importer may trim when it packs instead.
- **Editing.** A host or tool builds the edited waveform as a new file with its new id. A waveform
  no sound refers to any more is ordinary garbage.

## 6. Worked example

Two mono samples: the four frames 0, 1, -1, 32767 at 44,100 Hz (sample id A, worked example A of
`docs/format.md`) and the same frames at 48,000 Hz (sample id B):

```
A = 1e272d15841dbddadd5503dec6a3ac43020b40c9c0a730cc5cd2f66101301338
B = d9fca2910054e1d97a59a316ed2202be78ac726618907e72c581266ed13d4891
```

Three key banks, given in this order:

1. keys 60 to 127, velocities 1 to 127, root 72, sample B, loop forward from frame 100 to 2,000,
   tune -12.5 cents (-1,250), level -6 dB (-600);
2. keys 0 to 59, velocities 1 to 127, root 48, sample A, pan fully left (-10,000);
3. keys 0 to 59, velocities 1 to 127, root 48, sample B, pan fully right (10,000).

Banks 2 and 3 agree in their first 8 bytes and A sorts before B, so the canonical order is 2, 3, 1;
A is used first, so it is sample 0. The file is 204 bytes:

```
000: 4f 50 57 46  01 00  00 00  cc 00 00 00  af 01 0c 8b   OPWF, version 1, flags 0, size 204, CRC-32C
010: 02 00  03 00                                          S 2, K 3
014: 1e 27 2d 15 84 1d bd da dd 55 03 de c6 a3 ac 43       sample id 0: A
     02 0b 40 c9 c0 a7 30 cc 5c d2 f6 61 01 30 13 38
034: d9 fc a2 91 00 54 e1 d9 7a 59 a3 16 ed 22 02 be       sample id 1: B
     78 ac 72 66 18 90 7e 72 c5 81 26 6e d1 3d 48 91
054: 00 3b 01 7f 30 00 00 00  00 00 00 00                  key bank 0: keys 0-59, vel 1-127, root 48, no loop, sample 0
     00 00 00 00  00 00 00 00  f0 d8 00 00                 tune 0, level 0, pan -10000
     00 00 00 00  00 00 00 00  00 00 00 00  00 00 00 00    start 0, end 0, loop 0 0
07c: 00 3b 01 7f 30 00 00 00  01 00 00 00                  key bank 1: as bank 0 with sample 1
     00 00 00 00  00 00 00 00  10 27 00 00                 pan 10000
     00 00 00 00  00 00 00 00  00 00 00 00  00 00 00 00
0a4: 3c 7f 01 7f 48 00 01 00  01 00 00 00                  key bank 2: keys 60-127, vel 1-127, root 72, forward, sample 1
     1e fb ff ff  a8 fd ff ff  00 00 00 00                 tune -1250, level -600, pan 0
     00 00 00 00  00 00 00 00  64 00 00 00  d0 07 00 00    start 0, end 0, loop 100 to 2000
```

The hashed string is the 19-byte tag `6f73636169726e2d77617665666f726d2d3100` ("oscairn-waveform-1\0")
followed by the 188 bytes from offset 16, and the waveform id is

```
0ab2c142fc10f5f50f76c044e10942059087d87d02ff9acb6fa2edb298529e3f
```

`tools/waveform-example.py` computes these bytes and the id without the library (Python's
`struct` and `hashlib`, a bitwise CRC-32C); `tests/waveform.rs` checks that the library builds the
same bytes from the banks in every order and gives the same id, and `tests/abi_check.c` does the
same through the C interface. `samplepack waveform show` prints its id, sample ids and key banks.
