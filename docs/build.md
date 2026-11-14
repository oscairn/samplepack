# Building and integrating

## Toolchain

`rust-toolchain.toml` pins an exact stable release (1.95.0) with clippy and rustfmt, plus the
`aarch64-unknown-linux-gnu` standard library used by the cost gate. rustup installs it on first use;
nothing in the build uses nightly. `Cargo.lock` is committed and every command uses `--locked`. The
workspace has no dependencies, so builds need no network once the toolchain is installed.

Other targets need their standard library added to the pinned toolchain (`rustup target add
x86_64-pc-windows-msvc`, and so on) before cross builds.

## Layout of the build

* `crates/samplepack` is the library (`no_std`, no allocation, no dependencies, no `unsafe`).
* `crates/samplepack-capi` is the C interface: a `staticlib` named `libsamplepack_capi.a`
  (`samplepack_capi.lib` with MSVC) holding the only `unsafe` code, the conversion of pointers and
  lengths into slices, and the panic handler (abort).
* `include/samplepack.h` is the header, written by hand. `tests/c_abi.rs` builds the library, compiles
  `tests/abi_check.c` against it and compares struct sizes, offsets and enum values with the Rust
  definitions.

The split exists because a `no_std` static library must define a panic handler, and a crate that
does cannot also be linked, as an rlib, into the std test programs.

## CMake (plugin and host)

```cmake
list(APPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/third_party/samplepack/cmake")
include(SamplePack)
target_link_libraries(my_target PRIVATE samplepack::samplepack)   # adds the include directory too
```

`cmake/SamplePack.cmake` runs `cargo build --release --locked --offline -p samplepack-capi --target
<triple>` from the source tree (so rustup picks the pinned compiler) and exposes the result as the
imported target `samplepack::samplepack`. The triple comes from the active system and architecture,
or `-DSAMPLEPACK_RUST_TARGET=...`. With several `CMAKE_OSX_ARCHITECTURES` each is built and joined
with `lipo`. Extra rustc flags go in `SAMPLEPACK_RUSTFLAGS`, for example
`-DSAMPLEPACK_RUSTFLAGS="-C;target-cpu=x86-64-v2"`.

C++ includes the header as `<samplepack.h>`; it has `extern "C"` guards.

## Reproducible builds

Build with `--locked`, from a toolchain pinned as above, with `--remap-path-prefix` for the source
directory (the CMake module does this for the source and build directories):

```sh
CARGO_ENCODED_RUSTFLAGS=$(printf '%s\037%s' --remap-path-prefix "$PWD=/src") \
    cargo build --release --locked --offline -p samplepack-capi
```

`CARGO_ENCODED_RUSTFLAGS` (flags separated by 0x1f) is used instead of `RUSTFLAGS` so that paths
containing spaces survive. Two builds of the same source from differently named directories, with
different target directories, produced byte-identical `libsamplepack_capi.a` on macOS arm64
(checked with SHA-256). Reproducibility on other hosts has not been checked.

## Checks

| Command | What it checks |
| --- | --- |
| `cargo test --workspace` | corpus against `flac -d` output, rejections, buffers, scan, header against library |
| `sh tools/check-no-panic.sh` | no panic machinery in the archive; a build whose panic handler needs a non-existent symbol still links |
| `sh tools/run-fuzz.sh [cases [seed]]` | differential fuzz of the C interface against the C oracle, ASan and UBSan |
| `sh tools/gate.sh` | callgrind instructions per sample, Rust against the oracle, aarch64 (needs docker and an aarch64 image with gcc and valgrind) |
| `python3 tools/make-corpus.py` | regenerates `tests/data` with the stock `flac` encoder (needs `flac`) |
