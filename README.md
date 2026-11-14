# samplepack

Reader and writer for Oscairn Sample Packs (OSP, `.osp`), in Rust, with a C interface and a
command-line tool. One implementation serves the sampler plugin's loader thread and the host's
install, verify and export tools; it is the single parser of untrusted pack files for both.

`docs/format.md` is the format specification.

* `crates/samplepack`: the library. `no_std`, no allocation, no dependencies, no `unsafe`. Every
  length, order, shift and count taken from the input is validated before use; clippy denies
  indexing, unchecked arithmetic, `unwrap`, `expect` and `panic` in the library.
  Modules: `flac` (the subset decoder), `pack` (the container: open, recover, index, records,
  windows, frames, verification, and pure writer functions), `id` (sample ids), `hash` (CRC-32C,
  SHA-256, MD5).
* `crates/samplepack-capi`: static library and C ABI (`include/samplepack.h`).
* `crates/samplepack-cli`: the `samplepack` tool (std, no third-party crates): `pack` (from 16-bit
  WAV or FLAC, with its own small subset encoder), `list`, `verify [--full]`, `extract` (to a
  standard `.flac`), `copy`, `combine`, `remove`, `compact`, `locality`.
* `docs/format.md`: the OSP format, normative.
* `docs/decoder.md`: the exact FLAC subset and every rejection reason.
* `docs/build.md`: toolchain pin, CMake integration, reproducible builds, checks.
* `docs/gate.md`: the instruction-count gate against the C oracle, method and results.
* `tests/`: conformance corpus (made with the stock `flac` encoder from synthetic signals) and the C
  oracle used for differential testing. `fuzz/`: differential decoder fuzz harness, pack reader
  fuzz harness and its seed packs. `bench/`: cost gate.

```sh
cargo test --workspace
sh tools/run-fuzz.sh 200000
```

## Licence

MIT, see `LICENSE`. Source files start with an SPDX line. The repository contains no third-party
code; the test corpus and the fuzz seed packs are made from synthetic signals.
