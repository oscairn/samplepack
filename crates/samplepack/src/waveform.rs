// SPDX-License-Identifier: MIT
//! Oscairn Waveform files (`.owf`): which sample sounds for which keys and velocities, and how.
//! `docs/waveform.md` is the specification. A file is valid only in canonical form; its id is the
//! SHA-256 of a domain tag and its body. Parsing borrows the input; building writes into a caller
//! buffer. Nothing here depends on the pack reader.

use crate::hash::{crc32c, Sha256};
use crate::id::SampleId;
use core::cmp::Ordering;

/// Largest file, in bytes.
pub const MAX_BYTES: usize = HEADER + MAX_SAMPLES * 32 + MAX_BANKS * BANK;
/// Most sample ids in one file.
pub const MAX_SAMPLES: usize = 1024;
/// Most key banks in one file.
pub const MAX_BANKS: usize = 1024;
/// Bytes per key bank.
pub const BANK: usize = 40;
/// The domain tag that starts the hashed string of a waveform id (19 bytes, the last one zero).
pub const DOMAIN_TAG: &[u8; 19] = b"oscairn-waveform-1\0";
const MAGIC: &[u8; 4] = b"OPWF";
const HEADER: usize = 20;
const BODY: usize = 16;

/// Why a waveform was refused, or could not be built. [`Error::code`] gives the stable number used
/// by the C interface (`sp_status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A pointer or argument is invalid (C interface).
    InvalidArgument,
    /// The output buffer is too small.
    BufferTooSmall,
    /// No `OPWF` magic.
    NotWaveform,
    /// A version other than 1.
    UnsupportedVersion,
    /// Flags not 0.
    BadFlags,
    /// The length is not 20 + 32 S + 40 K (truncated, trailing bytes, or longer than [`MAX_BYTES`]).
    BadLength,
    /// The size field differs from the length.
    BadSize,
    /// The CRC-32C of the body does not match.
    BadChecksum,
    /// S outside 1 to [`MAX_SAMPLES`].
    BadSampleCount,
    /// K outside 1 to [`MAX_BANKS`].
    BadBankCount,
    /// A reserved byte of a key bank is not 0.
    ReservedNonZero,
    /// A field outside its range.
    BadRange,
    /// A lowest key or velocity above the highest.
    LowAboveHigh,
    /// A sample index of S or more.
    BadSampleIndex,
    /// Loop points without a loop, or loop points out of order.
    BadLoop,
    /// Key banks not in canonical order.
    NotSorted,
    /// Two equal key banks.
    DuplicateBank,
    /// Sample ids not in order of first use.
    SampleOrder,
    /// A sample id no key bank uses.
    UnusedSample,
    /// One sample id listed twice.
    DuplicateSample,
}

impl Error {
    /// The stable numeric code (`sp_status` in `include/samplepack.h`).
    pub fn code(self) -> i32 {
        match self {
            Error::InvalidArgument => 1,
            Error::BufferTooSmall => 2,
            e => 62i32.wrapping_add(e as i32),
        }
    }
}

/// One key bank (`sp_key_bank` in the C interface); the field order and offsets are those of the
/// file, with the reserved bytes as padding.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyBank {
    /// Lowest key, 0 to 127.
    pub key_lo: u8,
    /// Highest key, `key_lo` to 127.
    pub key_hi: u8,
    /// Lowest velocity, 1 to 127.
    pub vel_lo: u8,
    /// Highest velocity, `vel_lo` to 127.
    pub vel_hi: u8,
    /// Key at which the sample plays at its own rate, 0 to 127.
    pub root: u8,
    /// Semitones, -127 to 127.
    pub transpose: i8,
    /// 0 none, 1 forward.
    pub loop_mode: u8,
    /// Index into the sample ids.
    pub sample: u16,
    /// 1/100 cent, within ±1,200,000.
    pub tune: i32,
    /// 1/100 dB, -14,400 to 4,800.
    pub level: i32,
    /// 1/100 of -100..100, within ±10,000.
    pub pan: i16,
    /// First frame played.
    pub start: u32,
    /// End frame, exclusive; 0 for the end of the sample.
    pub end: u32,
    /// Loop start frame.
    pub loop_start: u32,
    /// Loop end frame, exclusive.
    pub loop_end: u32,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    b.get(at..)
        .and_then(|s| s.first_chunk())
        .map_or(0, |c| u16::from_le_bytes(*c))
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..)
        .and_then(|s| s.first_chunk())
        .map_or(0, |c| u32::from_le_bytes(*c))
}

impl KeyBank {
    /// Decodes 40 file bytes (reserved bytes are not looked at).
    pub fn from_bytes(b: &[u8; BANK]) -> Self {
        let [key_lo, key_hi, vel_lo, vel_hi, root, transpose, loop_mode, ..] = *b;
        Self {
            key_lo,
            key_hi,
            vel_lo,
            vel_hi,
            root,
            transpose: transpose as i8,
            loop_mode,
            sample: u16_at(b, 8),
            tune: u32_at(b, 12) as i32,
            level: u32_at(b, 16) as i32,
            pan: u16_at(b, 20) as i16,
            start: u32_at(b, 24),
            end: u32_at(b, 28),
            loop_start: u32_at(b, 32),
            loop_end: u32_at(b, 36),
        }
    }

    /// The 40 file bytes, reserved bytes 0.
    pub fn to_bytes(&self) -> [u8; BANK] {
        let mut b = [0u8; BANK];
        let fields: [(usize, &[u8]); 9] = [
            (
                0,
                &[
                    self.key_lo,
                    self.key_hi,
                    self.vel_lo,
                    self.vel_hi,
                    self.root,
                    self.transpose as u8,
                    self.loop_mode,
                ],
            ),
            (8, &self.sample.to_le_bytes()),
            (12, &self.tune.to_le_bytes()),
            (16, &self.level.to_le_bytes()),
            (20, &self.pan.to_le_bytes()),
            (24, &self.start.to_le_bytes()),
            (28, &self.end.to_le_bytes()),
            (32, &self.loop_start.to_le_bytes()),
            (36, &self.loop_end.to_le_bytes()),
        ];
        for (at, v) in fields {
            crate::hash::copy_into(b.get_mut(at..).unwrap_or(&mut []), v);
        }
        b
    }

    /// Every field rule of a key bank in a file with `samples` sample ids.
    pub fn check(&self, samples: usize) -> Result<(), Error> {
        let in_range = self.key_hi <= 127
            && (1..=127).contains(&self.vel_lo)
            && self.vel_hi <= 127
            && self.root <= 127
            && self.transpose != i8::MIN
            && self.loop_mode <= 1
            && (-1_200_000..=1_200_000).contains(&self.tune)
            && (-14_400..=4_800).contains(&self.level)
            && (-10_000..=10_000).contains(&self.pan);
        if !in_range {
            return Err(Error::BadRange);
        }
        if self.key_lo > self.key_hi || self.vel_lo > self.vel_hi {
            return Err(Error::LowAboveHigh);
        }
        if usize::from(self.sample) >= samples {
            return Err(Error::BadSampleIndex);
        }
        let loop_ok = if self.loop_mode == 0 {
            self.loop_start == 0 && self.loop_end == 0
        } else {
            self.start <= self.loop_start
                && self.loop_start < self.loop_end
                && (self.end == 0 || self.loop_end <= self.end)
        };
        if loop_ok {
            Ok(())
        } else {
            Err(Error::BadLoop)
        }
    }
}

/// Canonical order of two key banks: their bytes, with the sample index replaced by its id.
fn order(a: &[u8; BANK], b: &[u8; BANK], ids: &[SampleId]) -> Ordering {
    type Key<'x> = (Option<&'x [u8]>, Option<&'x SampleId>, Option<&'x [u8]>);
    fn key<'x>(x: &'x [u8; BANK], ids: &'x [SampleId]) -> Key<'x> {
        (x.get(..8), ids.get(usize::from(u16_at(x, 8))), x.get(10..))
    }
    key(a, ids).cmp(&key(b, ids))
}

/// A valid waveform file, borrowed.
#[derive(Clone, Copy, Debug)]
pub struct Waveform<'a> {
    samples: &'a [SampleId],
    banks: &'a [[u8; BANK]],
    body: &'a [u8],
}

impl<'a> Waveform<'a> {
    /// Checks every rule of the specification.
    pub fn parse(d: &'a [u8]) -> Result<Self, Error> {
        let w = Self::reopen(d)?;
        if u32_at(d, 12) != crc32c(w.body) {
            return Err(Error::BadChecksum);
        }
        for b in w.banks {
            let k = KeyBank::from_bytes(b);
            if k.to_bytes() != *b {
                return Err(Error::ReservedNonZero);
            }
            k.check(w.samples.len())?;
        }
        for (i, id) in w.samples.iter().enumerate() {
            if w.samples.get(..i).is_some_and(|s| s.contains(id)) {
                return Err(Error::DuplicateSample);
            }
        }
        for p in w.banks.windows(2) {
            if let [a, b] = p {
                match order(a, b, w.samples) {
                    Ordering::Less => {}
                    Ordering::Equal => return Err(Error::DuplicateBank),
                    Ordering::Greater => return Err(Error::NotSorted),
                }
            }
        }
        let mut used = 0usize;
        for b in w.banks {
            let s = usize::from(u16_at(b, 8));
            if s == used {
                used = used.wrapping_add(1);
            } else if s > used {
                return Err(Error::SampleOrder);
            }
        }
        if used != w.samples.len() {
            return Err(Error::UnusedSample);
        }
        Ok(w)
    }

    /// Checks only the header fields and the length, in constant time: for bytes that have
    /// passed [`Waveform::parse`] before (the C interface's accessors).
    pub fn reopen(d: &'a [u8]) -> Result<Self, Error> {
        if d.get(..4) != Some(MAGIC) {
            return Err(Error::NotWaveform);
        }
        if d.len() < HEADER || d.len() > MAX_BYTES {
            return Err(Error::BadLength);
        }
        if u16_at(d, 4) != 1 {
            return Err(Error::UnsupportedVersion);
        }
        if u16_at(d, 6) != 0 {
            return Err(Error::BadFlags);
        }
        if u32_at(d, 8) as usize != d.len() {
            return Err(Error::BadSize);
        }
        let (s, k) = (usize::from(u16_at(d, 16)), usize::from(u16_at(d, 18)));
        if !(1..=MAX_SAMPLES).contains(&s) {
            return Err(Error::BadSampleCount);
        }
        if !(1..=MAX_BANKS).contains(&k) {
            return Err(Error::BadBankCount);
        }
        // s and k are at most 1,024: no overflow
        let banks_at = HEADER.wrapping_add(s.wrapping_mul(32));
        if d.len() != banks_at.wrapping_add(k.wrapping_mul(BANK)) {
            return Err(Error::BadLength);
        }
        let ids = d.get(HEADER..banks_at).unwrap_or(&[]);
        let banks = d.get(banks_at..).unwrap_or(&[]);
        Ok(Self {
            samples: ids.as_chunks().0,
            banks: banks.as_chunks().0,
            body: d.get(BODY..).unwrap_or(&[]),
        })
    }

    /// The sample ids, in order of first use.
    pub fn samples(&self) -> &'a [SampleId] {
        self.samples
    }

    /// Number of key banks.
    pub fn banks(&self) -> usize {
        self.banks.len()
    }

    /// Key bank `i`, in canonical order.
    pub fn bank(&self, i: usize) -> Option<KeyBank> {
        self.banks.get(i).map(KeyBank::from_bytes)
    }

    /// The waveform id.
    pub fn id(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(DOMAIN_TAG);
        h.update(self.body);
        h.finish()
    }
}

/// Writes the canonical file for `banks`, whose `sample` fields index `ids` (in any order;
/// duplicates and unused ids are allowed), into `out`, and returns its length. `out` needs
/// [`file_len`] bytes, at most 20 + 72 x K; its contents are unspecified after an error.
pub fn build(banks: &[KeyBank], ids: &[SampleId], out: &mut [u8]) -> Result<usize, Error> {
    let len = file_len(banks, ids)?;
    let out = out.get_mut(..len).ok_or(Error::BufferTooSmall)?;
    out.fill(0);
    // key banks at their place, first with the caller's indices, then sorted
    let s = (len.wrapping_sub(HEADER).wrapping_sub(banks.len().wrapping_mul(BANK))) / 32;
    let (head, rest) = out.split_at_mut_checked(HEADER).ok_or(Error::BufferTooSmall)?;
    let (list, area) = rest
        .split_at_mut_checked(s.wrapping_mul(32))
        .ok_or(Error::BufferTooSmall)?;
    let slots = area.as_chunks_mut::<BANK>().0;
    for (slot, b) in slots.iter_mut().zip(banks) {
        *slot = b.to_bytes();
    }
    // insertion sort: the sorts in core keep a panic path for comparators that are not total
    for i in 1..slots.len() {
        for j in (1..=i).rev() {
            let Some([a, b]) = slots.get_mut(j.wrapping_sub(1)..=j) else {
                break;
            };
            match order(a, b, ids) {
                Ordering::Less => break,
                Ordering::Equal => return Err(Error::DuplicateBank),
                Ordering::Greater => core::mem::swap(a, b),
            }
        }
    }
    // sample ids in order of first use, and the key banks re-pointed at them
    let list = list.as_chunks_mut::<32>().0;
    let mut n = 0usize;
    for slot in slots.iter_mut() {
        let id = ids.get(usize::from(u16_at(slot, 8))).ok_or(Error::BadSampleIndex)?;
        let at = list.get(..n).and_then(|l| l.iter().position(|x| x == id)).unwrap_or(n);
        if at == n {
            if let Some(x) = list.get_mut(n) {
                *x = *id;
            }
            n = n.wrapping_add(1);
        }
        crate::hash::copy_into(slot.get_mut(8..).unwrap_or(&mut []), &(at as u16).to_le_bytes());
    }
    let fields: [(usize, &[u8]); 5] = [
        (0, MAGIC),
        (4, &1u16.to_le_bytes()),
        (8, &(len as u32).to_le_bytes()),
        (16, &(s as u16).to_le_bytes()),
        (18, &(banks.len() as u16).to_le_bytes()),
    ];
    for (at, v) in fields {
        crate::hash::copy_into(head.get_mut(at..).unwrap_or(&mut []), v);
    }
    let crc = crc32c(out.get(BODY..).unwrap_or(&[]));
    crate::hash::copy_into(out.get_mut(12..).unwrap_or(&mut []), &crc.to_le_bytes());
    Ok(len)
}

/// Length of the file [`build`] writes for `banks` and `ids`, after checking every key bank.
pub fn file_len(banks: &[KeyBank], ids: &[SampleId]) -> Result<usize, Error> {
    if !(1..=MAX_BANKS).contains(&banks.len()) {
        return Err(Error::BadBankCount);
    }
    for b in banks {
        b.check(ids.len())?;
    }
    let id = |b: &KeyBank| ids.get(usize::from(b.sample));
    let s = (0..banks.len())
        .filter(|&i| !banks.iter().take(i).any(|x| Some(id(x)) == banks.get(i).map(id)))
        .count();
    // s <= k <= 1,024
    Ok(HEADER
        .wrapping_add(s.wrapping_mul(32))
        .wrapping_add(banks.len().wrapping_mul(BANK)))
}
