// SPDX-License-Identifier: MIT
//! Decoder for a restricted subset of standard FLAC frames. See `docs/decoder.md` for the exact
//! subset and every rejection reason.

mod bits;
mod crc;
mod subframe;

use bits::BitReader;

/// Largest block size accepted, in samples per channel.
pub const MAX_BLOCK: usize = 4096;
/// Highest LPC order accepted.
pub const MAX_LPC_ORDER: usize = 8;
/// Flag: verify the CRC-8 of the frame header and the CRC-16 of the frame.
pub const FLAG_CHECK_CRC: u32 = 1;
/// Every flag bit this version understands.
pub const FLAGS_KNOWN: u32 = FLAG_CHECK_CRC;

/// Why a frame or stream was rejected. The numeric values are part of the C interface.
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A pointer was null or misaligned, a buffer ranges overlap, or an unknown flag was set.
    InvalidArgument = 1,
    /// An output buffer is smaller than the frame's block size, or the frame table is full.
    BufferTooSmall = 2,
    /// The data ends inside the frame header.
    Truncated = 3,
    /// The frame sync code is missing.
    BadSync = 4,
    /// A reserved or invalid value in the frame header.
    ReservedValue = 5,
    /// A valid FLAC feature outside the subset.
    Unsupported = 6,
    /// Frame header CRC-8 mismatch.
    HeaderCrc = 7,
    /// A malformed or reserved subframe.
    BadSubframe = 8,
    /// A malformed residual.
    BadResidual = 9,
    /// The frame needs more bits than the data holds.
    Overrun = 10,
    /// Frame CRC-16 mismatch.
    FrameCrc = 11,
    /// Not a FLAC stream header.
    BadStream = 12,
}

/// Decoder working memory, two channels of 32-bit samples (32 KiB). Contents between calls are
/// irrelevant; one scratch may be reused for any number of calls, by one call at a time.
#[repr(C)]
pub struct Scratch {
    /// Channel buffers.
    pub ch: [[i32; MAX_BLOCK]; 2],
}

impl Scratch {
    /// A zeroed scratch.
    pub const fn new() -> Self {
        Self {
            ch: [[0; MAX_BLOCK]; 2],
        }
    }
}

impl Default for Scratch {
    fn default() -> Self {
        Self::new()
    }
}

/// What a successful frame decode produced.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameInfo {
    /// Samples per channel.
    pub samples: u32,
    /// Channels in the frame (1 or 2).
    pub channels: u32,
    /// Bytes the frame occupies, including its CRC-16.
    pub consumed: u32,
    /// Always zero.
    pub reserved: u32,
}

/// One frame found by [`scan`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameEntry {
    /// Offset of the frame from the start of the scanned data.
    pub offset: u64,
    /// Length of the frame in bytes.
    pub length: u32,
    /// Samples per channel.
    pub samples: u32,
}

/// Result of [`scan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanResult {
    /// Entries written to the table.
    pub frames: usize,
    /// Bytes covered by those frames.
    pub consumed: usize,
    /// Sum of samples per channel over those frames.
    pub total_samples: u64,
    /// `Ok` when the data ended exactly after the last frame.
    pub status: Result<(), Error>,
}

struct Header {
    samples: usize,
    assign: u8,
    channels: usize,
    /// Offset of the first subframe byte.
    body: usize,
}

const BLOCK_SIZES: [u16; 16] = [
    0, 192, 576, 1152, 2304, 4608, 0, 0, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768,
];

fn parse_header(d: &[u8], check_crc: bool) -> Result<Header, Error> {
    let [b0, b1, b2, b3, b4, _] = *d.first_chunk::<6>().ok_or(Error::Truncated)?;
    if b0 != 0xFF || (b1 & 0xFE) != 0xF8 {
        return Err(Error::BadSync);
    }
    let variable = b1 & 1 != 0;
    let bsc = usize::from(b2 >> 4);
    let src = b2 & 15;
    let assign = b3 >> 4;
    let size_code = (b3 >> 1) & 7;
    if b3 & 1 != 0 || src == 15 || assign > 10 {
        return Err(Error::ReservedValue);
    }
    match size_code {
        0 | 4 => {}
        1 | 2 | 5 | 6 => return Err(Error::Unsupported),
        _ => return Err(Error::ReservedValue),
    }
    // Frame or sample number: UTF-8 style, up to 6 bytes (fixed block size) or 7 (variable).
    let ext: usize = match b4 {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        0xF8..=0xFB => 5,
        0xFC..=0xFD => 6,
        0xFE if variable => 7,
        _ => return Err(Error::ReservedValue),
    };
    let num = d.get(4..4usize.wrapping_add(ext)).ok_or(Error::Truncated)?;
    if num.iter().skip(1).any(|b| b & 0xC0 != 0x80) {
        return Err(Error::ReservedValue);
    }
    let mut pos = 4usize.wrapping_add(ext);
    let mut samples = usize::from(BLOCK_SIZES.get(bsc).copied().unwrap_or(0));
    if bsc == 6 {
        samples = usize::from(*d.get(pos).ok_or(Error::Truncated)?).wrapping_add(1);
        pos = pos.wrapping_add(1);
    } else if bsc == 7 {
        let hi = *d.get(pos).ok_or(Error::Truncated)?;
        let lo = *d.get(pos.wrapping_add(1)).ok_or(Error::Truncated)?;
        samples = (usize::from(hi) << 8 | usize::from(lo)).wrapping_add(1);
        pos = pos.wrapping_add(2);
    } else if samples == 0 {
        return Err(Error::ReservedValue);
    }
    if samples > MAX_BLOCK {
        return Err(Error::Unsupported);
    }
    pos = pos.wrapping_add(match src {
        12 => 1,
        13 | 14 => 2,
        _ => 0,
    });
    let stored = *d.get(pos).ok_or(Error::Truncated)?;
    if check_crc && crc::crc8(d.get(..pos).unwrap_or(&[])) != stored {
        return Err(Error::HeaderCrc);
    }
    let channels = if assign < 8 {
        usize::from(assign).wrapping_add(1)
    } else {
        2
    };
    if channels > 2 {
        return Err(Error::Unsupported);
    }
    Ok(Header {
        samples,
        assign,
        channels,
        body: pos.wrapping_add(1),
    })
}

/// Decodes the header and subframes into `scratch`; returns the header and the frame length.
fn decode_body(d: &[u8], flags: u32, scratch: &mut Scratch, hdr: &Header) -> Result<usize, Error> {
    let mut r = BitReader::new(d, hdr.body);
    let [a, b] = &mut scratch.ch;
    for c in 0..hdr.channels {
        let side = (hdr.assign == 8 && c == 1) || (hdr.assign == 9 && c == 0) || (hdr.assign == 10 && c == 1);
        let bps = if side { 17 } else { 16 };
        let buf = if c == 0 { &mut *a } else { &mut *b };
        subframe::subframe(&mut r, buf.get_mut(..hdr.samples).ok_or(Error::Unsupported)?, bps)?;
    }
    let len = usize::try_from(r.consumed_bits().wrapping_add(7) / 8)
        .map_err(|_| Error::Overrun)?
        .wrapping_add(2);
    if len > d.len() {
        return Err(Error::Overrun);
    }
    if flags & FLAG_CHECK_CRC != 0 {
        let body = d.get(..len.wrapping_sub(2)).unwrap_or(&[]);
        let stored = d.get(len.wrapping_sub(2)..len).and_then(|s| s.first_chunk::<2>());
        if stored.map(|s| u16::from_be_bytes(*s)) != Some(crc::crc16(body)) {
            return Err(Error::FrameCrc);
        }
    }
    Ok(len)
}

/// Decodes the frame at the start of `data` (which may extend past the frame).
///
/// Channel 0 goes to `out0`; channel 1 of a stereo frame goes to `out1`. Both must hold at least
/// the frame's block size (`out1` may be empty for a mono frame), otherwise
/// [`Error::BufferTooSmall`] is returned. On any error the outputs are left untouched.
pub fn decode_frame(
    data: &[u8],
    flags: u32,
    scratch: &mut Scratch,
    out0: &mut [i16],
    out1: &mut [i16],
) -> Result<FrameInfo, Error> {
    if flags & !FLAGS_KNOWN != 0 {
        return Err(Error::InvalidArgument);
    }
    let hdr = parse_header(data, flags & FLAG_CHECK_CRC != 0)?;
    let n = hdr.samples;
    if out0.len() < n || (hdr.channels == 2 && out1.len() < n) {
        return Err(Error::BufferTooSmall);
    }
    let len = decode_body(data, flags, scratch, &hdr)?;
    let [a, b] = &scratch.ch;
    let a = a.get(..n).unwrap_or(&[]);
    let b = b.get(..n).unwrap_or(&[]);
    let l = out0.get_mut(..n).unwrap_or(&mut []);
    if hdr.channels == 1 {
        for (o, x) in l.iter_mut().zip(a) {
            *o = *x as i16;
        }
    } else {
        let r = out1.get_mut(..n).unwrap_or(&mut []);
        let pairs = a.iter().zip(b).zip(l.iter_mut().zip(r.iter_mut()));
        match hdr.assign {
            8 => {
                for ((&x, &y), (lo, ro)) in pairs {
                    *lo = x as i16;
                    *ro = (x as u32).wrapping_sub(y as u32) as i16;
                }
            }
            9 => {
                for ((&x, &y), (lo, ro)) in pairs {
                    *lo = (x as u32).wrapping_add(y as u32) as i16;
                    *ro = y as i16;
                }
            }
            10 => {
                for ((&x, &y), (lo, ro)) in pairs {
                    let m = (((x as u32) << 1) as i32) | (y & 1);
                    *lo = ((m as u32).wrapping_add(y as u32) as i32 >> 1) as i16;
                    *ro = ((m as u32).wrapping_sub(y as u32) as i32 >> 1) as i16;
                }
            }
            _ => {
                for ((&x, &y), (lo, ro)) in pairs {
                    *lo = x as i16;
                    *ro = y as i16;
                }
            }
        }
    }
    Ok(FrameInfo {
        samples: n as u32,
        channels: hdr.channels as u32,
        consumed: len as u32,
        reserved: 0,
    })
}

/// Walks the frames of `data` (starting at the first frame) by decoding each one, and records
/// their boundaries in `table`. Stops at the first error, or when the table is full while more
/// data remains ([`Error::BufferTooSmall`]).
pub fn scan(data: &[u8], flags: u32, scratch: &mut Scratch, table: &mut [FrameEntry]) -> ScanResult {
    let mut res = ScanResult {
        frames: 0,
        consumed: 0,
        total_samples: 0,
        status: Ok(()),
    };
    if flags & !FLAGS_KNOWN != 0 {
        res.status = Err(Error::InvalidArgument);
        return res;
    }
    for slot in table.iter_mut() {
        let rest = data.get(res.consumed..).unwrap_or(&[]);
        if rest.is_empty() {
            return res;
        }
        let step = parse_header(rest, flags & FLAG_CHECK_CRC != 0)
            .and_then(|h| decode_body(rest, flags, scratch, &h).map(|len| (h.samples, len)));
        match step {
            Ok((samples, len)) => {
                *slot = FrameEntry {
                    offset: res.consumed as u64,
                    length: len as u32,
                    samples: samples as u32,
                };
                res.frames = res.frames.wrapping_add(1);
                res.consumed = res.consumed.wrapping_add(len);
                res.total_samples = res.total_samples.wrapping_add(samples as u64);
            }
            Err(e) => {
                res.status = Err(e);
                return res;
            }
        }
    }
    if res.consumed < data.len() {
        res.status = Err(Error::BufferTooSmall);
    }
    res
}

/// Offset of the first frame in a `.flac` file (after the metadata blocks).
pub fn first_frame(data: &[u8]) -> Result<usize, Error> {
    if data.len() < 8 || data.get(..4) != Some(b"fLaC") {
        return Err(Error::BadStream);
    }
    let mut i = 4usize;
    loop {
        let h = data
            .get(i..)
            .and_then(|s| s.first_chunk::<4>())
            .ok_or(Error::BadStream)?;
        let len = usize::from(h[1]) << 16 | usize::from(h[2]) << 8 | usize::from(h[3]);
        i = i
            .checked_add(4)
            .and_then(|v| v.checked_add(len))
            .ok_or(Error::BadStream)?;
        if h[0] & 0x80 != 0 {
            break;
        }
    }
    if i <= data.len() {
        Ok(i)
    } else {
        Err(Error::BadStream)
    }
}
