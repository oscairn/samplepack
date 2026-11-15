# samplepack

Reader and writer for Oscairn Sample Packs (OSP, `.osp`), in Rust, with a C interface and a
command-line tool. One implementation serves the sampler plugin's loader thread and the host's
install, verify and export tools; it is the single parser of untrusted pack files for both.

`docs/format.md` is the format specification. The library also reads, checks and writes Oscairn
Waveform files (`.owf`, `docs/waveform.md`), which say how a set of samples is played across the
keyboard.

* `crates/samplepack`: the library. `no_std`, no allocation, no dependencies, no `unsafe`. Every
  length, order, shift and count taken from the input is validated before use; clippy denies
  indexing, unchecked arithmetic, `unwrap`, `expect` and `panic` in the library.
  Modules: `flac` (the subset decoder), `pack` (the container: open, recover, index, records,
  windows, frames, verification, and pure writer functions), `id` (sample ids), `hash` (CRC-32C,
  SHA-256, MD5), `waveform` (waveform files: parse, check, build, id; independent of `pack`).
* `crates/samplepack-capi`: static library and C ABI (`include/samplepack.h`).
* `crates/samplepack-cli`: the `samplepack` tool (std, no third-party crates): `pack` (from 16-bit
  WAV or FLAC, with its own deterministic subset encoder: fixed and LPC order 1 to 4 predictors,
  every choice made by exact coded size), `list`, `verify [--full]`, `extract` (to a
  standard `.flac`), `copy`, `combine`, `remove`, `compact`, `locality`, and `waveform show`,
  `waveform check` and `waveform from-wav` (a one-key-bank waveform and its sample from a WAV file).
* `docs/format.md`: the OSP format, normative.
* `docs/waveform.md`: the waveform format, normative.
* `docs/decoder.md`: the exact FLAC subset and every rejection reason.
* `docs/build.md`: toolchain pin, CMake integration, reproducible builds, checks.
* `docs/gate.md`: the instruction-count gate against the C oracle, method and results.
* `tests/`: conformance corpus (made with the stock `flac` encoder from synthetic signals) and the C
  oracle used for differential testing. `fuzz/`: differential decoder fuzz harness, pack reader
  fuzz harness and its seed packs, waveform fuzz harness. `bench/`: cost gate.

```sh
cargo test --workspace
sh tools/run-fuzz.sh 200000
```

## Memory

The library allocates nothing and keeps no state between calls. Everything it uses is either the
caller's memory, sized as below, or stack; nothing grows with use or over time.

| What | Caller provides |
| --- | --- |
| An open pack | the mapping of the file (read-only) and nothing else: no handle is kept; the C `sp_pack_info` is 32 bytes |
| An index of N records | 56 bytes per entry (`sp_index_entry`). `sp_pack_index` needs N = live records; `sp_pack_rebuild_index` and `sp_pack_verify` need one entry per record and tombstone in the pack (a 1 GiB pack has at most 262,143 entries, 14 MiB) |
| Decoding | 32,768 bytes of scratch (`sp_flac_scratch`), reusable, plus the output: 2 bytes per frame per channel (at most 8 KiB per channel for a 4,096-frame block) |
| Verifying, computing a record's id | 65,536 bytes of scratch (`sp_verify_scratch`, which contains a decode scratch) |
| A record description | 128 bytes (`sp_record_info`), optional |
| A waveform | the file's bytes (at most 73,748); building one, an output buffer of at most 20 + 72 x K bytes for K key banks |

Stack: a few kilobytes for any C function (plus the C library's `memcpy`, `memcmp` and `bzero`);
there is no recursion and no frame of variable size.

## Licence

MIT, see `LICENSE`. Source files start with an SPDX line. The repository contains no third-party
code; the test corpus and the fuzz seed packs are made from synthetic signals.

This project is independent and is not affiliated with or endorsed by any instrument manufacturer.
It contains and distributes no manufacturer samples, presets, firmware or code. Product names, where
they appear at all, belong to their owners.
