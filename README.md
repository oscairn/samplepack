# samplepack

Reader for sample packs, in Rust, with a C interface. One implementation serves the sampler plugin's
loader thread and the host's install, verify and export tools.

Status: the FLAC subset decoder is implemented. The pack container (index, records, trailers) and
sample-id hashing are not yet; the crate documentation marks where they will go.

* `crates/samplepack`: the library. `no_std`, no allocation, no dependencies, no `unsafe`. Every
  length, order, shift and count taken from the input is validated before use; clippy denies
  indexing, unchecked arithmetic, `unwrap`, `expect` and `panic` in the library.
* `crates/samplepack-capi`: static library and C ABI (`include/samplepack.h`).
* `docs/decoder.md`: the exact subset and every rejection reason.
* `docs/build.md`: toolchain pin, CMake integration, reproducible builds, checks.
* `docs/gate.md`: the instruction-count gate against the C oracle, method and results.
* `tests/`: conformance corpus (made with the stock `flac` encoder from synthetic signals) and the C
  oracle used for differential testing. `fuzz/`: differential fuzz harness. `bench/`: cost gate.

```sh
cargo test --workspace
sh tools/run-fuzz.sh 200000
```

## Licence

AGPL-3.0-or-later, see `LICENSE`. Source files start with an SPDX line.
