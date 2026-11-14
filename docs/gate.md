# Cost gate

The decoder must stay within 5% of the C oracle's instruction count on aarch64, on the same frames.

## Method

`tools/gate.sh` cross-compiles `libsamplepack_capi.a` on the host for `aarch64-unknown-linux-gnu`
(release profile, `-C target-cpu=cortex-a53`; a static library needs no linker), then runs
`tools/gate-inner.sh` in a throwaway `docker run --rm` aarch64 Linux container. There `bench/bench.c`
is compiled with `gcc -O3 -mcpu=cortex-a53` and linked with the Rust library and the oracle header,
and valgrind's callgrind counts instructions (Ir) inside one function per decoder
(`decode_all_rust`, `decode_all_oracle`) that decodes every frame of every stream. Instructions per
sample are Ir divided by samples times channels. Frame decoding is called the way a loader would:
one frame at a time, output to planar `int16_t` buffers. With `nocrc` neither decoder checks CRCs.
Callgrind counts instructions, not cycles: it says nothing about memory stalls or the in-order
Cortex-A53 pipeline.

## Result

Compiler: rustc 1.95.0 (pinned), gcc 13.3 for the oracle. Instructions per sample, oracle / Rust:

```
all (82 files) | nocrc: oracle 28.43 rust 27.25 ips (-4.1%) | crc: oracle 36.66 rust 33.45 ips (-8.8%)
-l 0 (12 files) | nocrc: oracle 27.93 rust 27.62 ips (-1.1%) | crc: oracle 36.79 rust 34.28 ips (-6.8%)
-l 4 (38 files) | nocrc: oracle 26.01 rust 25.29 ips (-2.8%) | crc: oracle 33.75 rust 31.11 ips (-7.8%)
-l 8 (25 files) | nocrc: oracle 31.51 rust 29.43 ips (-6.6%) | crc: oracle 40.05 rust 35.85 ips (-10.5%)
mono (10 files) | nocrc: oracle 24.79 rust 24.58 ips (-0.8%) | crc: oracle 31.14 rust 29.38 ips (-5.6%)
sweep stereo (11 files) | nocrc: oracle 35.08 rust 34.30 ips (-2.2%) | crc: oracle 41.72 rust 39.30 ips (-5.8%)
noise stereo (12 files) | nocrc: oracle 23.99 rust 17.57 ips (-26.7%) | crc: oracle 40.09 rust 29.67 ips (-26.0%)
colored stereo (12 files) | nocrc: oracle 33.17 rust 32.18 ips (-3.0%) | crc: oracle 45.25 rust 41.26 ips (-8.8%)
correlated stereo (11 files) | nocrc: oracle 36.28 rust 35.32 ips (-2.6%) | crc: oracle 43.28 rust 40.60 ips (-6.2%)
clipped stereo (11 files) | nocrc: oracle 31.60 rust 32.79 ips (+3.8%) | crc: oracle 39.63 rust 38.83 ips (-2.0%)
```

The same on the three subset streams embedded in the oracle's own unit bench (a 6 s stereo synthetic
signal; 529,200 samples each; not re-run with the current code):

```
all (3 files) | nocrc: oracle 33.94 rust 32.61 ips (-3.9%) | crc: oracle 38.67 rust 36.16 ips (-6.5%)
```

Without CRC every group is within +5%: the worst is the clipped-signal group (+3.8%), the aggregate is
-4.1%. The Rust decoder is cheaper than the oracle on noise (verbatim and escaped residuals) and
with CRC checking; the reasons were not investigated.

## History

The first measurement was 11.8% over the oracle on the corpus (24.48 M against 21.89 M instructions,
no CRC). What closed the gap, from reading the generated aarch64 code and re-measuring:

* Rice decoding: the zero count and the remainder are taken from the top `len` bits of the cache in
  one shift (`(1 << k | remainder) + ((zeros - 1) << k)`), and the shifts use `wrapping_*` forms with
  amounts known to be below 64, which removes the compare-and-select code that `checked_shl` generates.
* The test for reading too far past the end is made only when the 8-byte load from the slice fails,
  not on every refill. (Applying the same change to the general bit reader made things worse and was
  dropped.)

Design choices made from the start rather than measured: the 32 KiB working buffers are supplied by
the caller (`sp_flac_scratch`) instead of being zeroed on the stack for each frame, and the
predictors walk the samples with `windows()` over `Cell<i32>`, so no index needs a bounds check (the
LPC loops come out with the coefficients in registers).
