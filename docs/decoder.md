# FLAC subset decoder

`samplepack::flac` decodes standard FLAC frames (RFC 9639) restricted to the subset below. The
encoder is the stock `flac` tool (for example `flac -8 -b 1024 -l 4`); anything outside the subset is
rejected, never approximated. Entry points: `decode_frame`, `scan`, `first_frame` (C: `sp_flac_*` in
`include/samplepack.h`).

## Accepted

| Feature | Accepted |
| --- | --- |
| Channels | 1, or 2 with independent, left/side, side/right or mid/side assignment |
| Sample size | 16 bit (header code 4). Code 0 ("take it from STREAMINFO") is accepted and read as 16 bit, because the decoder never sees STREAMINFO |
| Block size | 1 to 4096 samples, any header coding: the 4-bit table values 192, 576, 1152, 2304, 256, 512, 1024, 2048, 4096, or an explicit 8- or 16-bit value |
| Sample rate | any header code except the reserved one; the value is not used |
| Blocking strategy | fixed or variable; the coded frame/sample number is checked for form but its value is not used |
| Subframes | constant; verbatim; fixed predictor of order 0 to 4; LPC of order 1 to 8 |
| Residual | Rice and Rice2 coding, any partition order that divides the block size, escaped partitions |
| Wasted bits | accepted (fewer than the sample width) |
| Checksums | CRC-8 of the header and CRC-16 of the frame, verified only when `SP_FLAC_CHECK_CRC` is passed |

A frame is decoded as a unit: either all samples are written and `consumed` is returned, or nothing
is written.

Arithmetic follows the C oracle exactly, including on malformed input: predictor sums wrap in 32
bits (64 bits when the 32-bit sum could overflow: sample width + coefficient precision +
ceil(log2(order)) > 32), the side channel is 17 bits wide, and results are truncated to 16 bits.
For frames made by a conforming encoder none of this wraps.

## Rejected, by error code

Codes are stable (`sp_status`). Where more than one reason applies, the first one met in decoding
order is reported.

| Code | Name | Reasons |
| --- | --- | --- |
| 1 | `INVALID_ARGUMENT` | (C interface and flags only) null or misaligned pointer, overlapping channel buffers, scratch or `info` null, unknown flag bit; also a null `data` with a non-zero length |
| 2 | `BUFFER_TOO_SMALL` | output buffer holds fewer samples than the frame's block size (checked as soon as the header is parsed; ch1 only for a stereo frame); `scan` table full while data remains |
| 3 | `TRUNCATED` | fewer than 6 bytes; the data ends inside the coded number, the explicit block-size or sample-rate bytes, or before the header CRC-8 byte |
| 4 | `BAD_SYNC` | first byte is not 0xFF, or the second is not 0xF8 / 0xF9 |
| 5 | `RESERVED_VALUE` | reserved bit after the sample size set; sample rate code 15; channel assignment 11 to 15; sample size code 3 or 7; block size code 0; coded number whose first byte is 10xxxxxx or 0xFF, or 0xFE in a fixed-block-size frame, or whose continuation bytes are not 10xxxxxx |
| 6 | `UNSUPPORTED` | sample size code 1, 2, 5 or 6 (8, 12, 20, 24 bit); block size above 4096; channel assignment 2 to 7 (three to eight independent channels); LPC order 9 to 32 |
| 7 | `HEADER_CRC` | CRC-8 mismatch (only with the CRC flag) |
| 8 | `BAD_SUBFRAME` | subframe padding bit set; subframe type 2 to 7 or 13 to 31 (reserved); fixed or LPC order larger than the block size; wasted-bits count not below the sample width; LPC precision field 15; negative LPC shift |
| 9 | `BAD_RESIDUAL` | residual coding method 2 or 3; block size not divisible by 2^partition order; partition smaller than the predictor order |
| 10 | `OVERRUN` | the subframes need more bits than the data holds, or the frame (bits used rounded up to bytes, plus the 2-byte CRC-16) is longer than the data |
| 11 | `FRAME_CRC` | CRC-16 mismatch (only with the CRC flag) |
| 12 | `BAD_STREAM` | (`first_frame` only) missing `fLaC` marker, fewer than 8 bytes, or metadata blocks running past the data |

## Not checked

* The value of the frame or sample number, the sample rate value, and consistency with STREAMINFO.
* Zero padding in the last byte of a frame (only the CRC-16, when requested, covers it).
* Overlong forms of the coded number (a continuation byte after a lead byte must have the form
  10xxxxxx; the number's value is not decoded).
* The bits after the end of the frame: `decode_frame` ignores any data after the frame, which is how
  callers pass "the rest of the file".

## Reads past the end

All reads are bounded by the data length. The bit reader treats bytes beyond the end as zero and
gives up once it has loaded more than 8 bytes beyond the end; a frame that consumed bits beyond the
end is rejected (`OVERRUN`) after decoding, so zero padding can never turn a truncated frame into
a successful one. No input byte is read outside `data`.

## Relation to the C oracle

The C decoder (`tests/oracle/fsd.h`) is the differential-test oracle. The only difference in what it
accepts is the coded number: the original accepts any continuation bytes and any lead byte,
including 0xFF, where RFC 9639 requires 10xxxxxx continuation bytes, 0xFF is invalid, and the
seven-byte form exists only for sample numbers of variable-block-size streams. The Rust decoder
follows the specification, so the oracle in this repository carries a small `FSD_STRICT_UTF8` block
(enabled by the test harness) that applies the same rule. Built without it, the harness reports
divergences only on malformed coded numbers.
