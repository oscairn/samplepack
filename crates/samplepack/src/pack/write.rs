// SPDX-License-Identifier: MIT
//! Writing: pure functions that serialise entries into caller buffers. The caller writes them at
//! the offsets the append protocol gives (section "Appending") and syncs in the stated order.

use super::read::check_entry;
use super::verify::decode_audio;
use super::{
    entry_crc, page_up, Error, IndexEntry, Record, RecordInfo, VerifyScratch, FOOTER, MAGIC_FOOTER, MAGIC_HEADER,
    MAGIC_RECORD, MAGIC_TOMBSTONE, MAGIC_TRAILER, MAJOR, MAX_CREDIT, MAX_PACK, MAX_RECORDS, MAX_WINDOW, MINOR, PAGE,
    PREFIX, RECORD_FIXED, TOMBSTONE_CHECKED, TRAILER_ENTRY, TRAILER_FIXED, WINDOW_STEP,
};
use crate::flac;
use crate::hash::{copy_into, crc32c};
use crate::id::SampleId;

fn put(out: &mut [u8], at: usize, v: &[u8]) {
    copy_into(out.get_mut(at..).unwrap_or(&mut []), v);
}

/// Writes an entry prefix with a zero checksum.
fn put_prefix(out: &mut [u8], magic: [u8; 4], length: usize, checked: usize) {
    put(out, 0, &magic);
    put(out, 4, &MAJOR.to_le_bytes());
    put(out, 6, &MINOR.to_le_bytes());
    put(out, 8, &(length as u64).to_le_bytes());
    put(out, 16, &(checked as u32).to_le_bytes());
    put(out, 20, &[0; 4]);
}

/// Fills in the checksum of the entry at the start of `out`.
fn seal(out: &mut [u8], checked: usize) {
    let c = entry_crc(out, checked).unwrap_or(0);
    put(out, 20, &c.to_le_bytes());
}

/// Zeroes `out[..len]`, or fails if `out` is shorter.
fn region(out: &mut [u8], len: usize) -> Result<&mut [u8], Error> {
    let r = out.get_mut(..len).ok_or(Error::BufferTooSmall)?;
    r.fill(0);
    Ok(r)
}

/// Writes the 4,096-byte pack header into `out[..4096]`.
pub fn write_pack_header(out: &mut [u8]) -> Result<(), Error> {
    let r = region(out, PAGE)?;
    put_prefix(r, MAGIC_HEADER, PAGE, PREFIX);
    seal(r, PREFIX);
    Ok(())
}

/// Writes a tombstone for `id` into `out[..4096]`.
pub fn write_tombstone(id: &SampleId, out: &mut [u8]) -> Result<(), Error> {
    let r = region(out, PAGE)?;
    put_prefix(r, MAGIC_TOMBSTONE, PAGE, TOMBSTONE_CHECKED);
    put(r, PREFIX, id);
    seal(r, TOMBSTONE_CHECKED);
    Ok(())
}

/// Bytes of a trailer (with its footer) listing `count` records.
pub fn trailer_len(count: usize) -> Result<usize, Error> {
    if count > MAX_RECORDS {
        return Err(Error::InvalidArgument);
    }
    let need = (count as u64)
        .wrapping_mul(TRAILER_ENTRY as u64)
        .wrapping_add((TRAILER_FIXED + FOOTER) as u64);
    page_up(need).map(|v| v as usize).ok_or(Error::InvalidArgument)
}

/// Writes a trailer listing `entries` (every live record, in file order) for a trailer placed at
/// `trailer_offset`, into `out[..trailer_len(entries.len())]`. Returns the bytes written. The
/// pack's length after this trailer is `trailer_offset` plus that.
pub fn write_trailer(entries: &[IndexEntry], trailer_offset: u64, out: &mut [u8]) -> Result<usize, Error> {
    let len = trailer_len(entries.len())?;
    if !trailer_offset.is_multiple_of(PAGE as u64) || trailer_offset < PAGE as u64 {
        return Err(Error::InvalidArgument);
    }
    if trailer_offset
        .checked_add(len as u64)
        .is_none_or(|end| end > MAX_PACK as u64)
    {
        return Err(Error::TooLarge);
    }
    let mut prev = PAGE as u64;
    for e in entries {
        prev = check_entry(e, prev, trailer_offset)?;
    }
    let r = region(out, len)?;
    let checked = TRAILER_FIXED.wrapping_add(entries.len().wrapping_mul(TRAILER_ENTRY));
    put_prefix(r, MAGIC_TRAILER, len, checked);
    put(r, 24, &(entries.len() as u32).to_le_bytes());
    for (i, e) in entries.iter().enumerate() {
        let at = TRAILER_FIXED.wrapping_add(i.wrapping_mul(TRAILER_ENTRY));
        put(r, at, &e.id);
        put(r, at.wrapping_add(32), &e.offset.to_le_bytes());
        put(r, at.wrapping_add(40), &e.length.to_le_bytes());
        put(r, at.wrapping_add(48), &e.preload.to_le_bytes());
    }
    seal(r, checked);
    let f = len.wrapping_sub(FOOTER);
    put(r, f, &MAGIC_FOOTER);
    put(r, f.wrapping_add(8), &trailer_offset.to_le_bytes());
    put(r, f.wrapping_add(16), &(len as u64).to_le_bytes());
    let c = crc32c(r.get(f..f.wrapping_add(28)).unwrap_or(&[]));
    put(r, f.wrapping_add(28), &c.to_le_bytes());
    Ok(len)
}

/// What a record is made from. The frames must be one FLAC stream of the subset with a fixed
/// block size, numbered from 0; `table` holds `blocks + 1` offsets into `frame_data`.
#[derive(Clone, Copy, Debug)]
pub struct RecordParams<'a> {
    /// Sample rate in Hz.
    pub rate: u32,
    /// Channels (1 or 2).
    pub channels: u8,
    /// Frames (samples per channel).
    pub frames: u32,
    /// Frames per FLAC block.
    pub block_size: u32,
    /// Preload window setting in frames: 0 or a multiple of 1,024 up to 16,384. The record holds
    /// `min(frames, window)` frames.
    pub window: u32,
    /// Credit string (at most 1,024 bytes).
    pub credit: &'a str,
    /// Frame table: offsets of each frame in `frame_data`, then its length.
    pub table: &'a [u32],
    /// The FLAC frames.
    pub frame_data: &'a [u8],
}

struct Layout {
    window_frames: u32,
    table_off: usize,
    checked: usize,
    head: usize,
    wbytes: usize,
    total: usize,
}

fn layout(p: &RecordParams) -> Result<Layout, Error> {
    let bad = Error::InvalidArgument;
    let blocks = p.table.len().checked_sub(1).ok_or(bad)?;
    let bs = p.block_size;
    if !(1..=2).contains(&p.channels)
        || !(1..=1_048_575).contains(&p.rate)
        || !bs.is_power_of_two()
        || !(256..=flac::MAX_BLOCK as u32).contains(&bs)
        || p.frames == 0
        || blocks as u64 != u64::from(p.frames).div_ceil(u64::from(bs))
        || p.credit.len() > MAX_CREDIT
        || !p.window.is_multiple_of(WINDOW_STEP)
        || p.window > MAX_WINDOW
        || p.table.last().map(|&v| v as usize) != Some(p.frame_data.len())
    {
        return Err(bad);
    }
    let window_frames = p.window.min(p.frames);
    if window_frames < p.frames && !window_frames.is_multiple_of(bs) {
        return Err(bad);
    }
    let table_off = RECORD_FIXED.wrapping_add(p.credit.len().next_multiple_of(4));
    let checked = table_off.wrapping_add(p.table.len().wrapping_mul(4));
    let head = page_up(checked as u64).ok_or(bad)? as usize;
    let wbytes = page_up(
        u64::from(window_frames)
            .wrapping_mul(u64::from(p.channels))
            .wrapping_mul(2),
    )
    .ok_or(bad)? as usize;
    let total = page_up(
        (head as u64)
            .wrapping_add(wbytes as u64)
            .wrapping_add(p.frame_data.len() as u64),
    )
    .ok_or(bad)?;
    if total > (MAX_PACK as u64).wrapping_sub(PAGE as u64).wrapping_sub(PAGE as u64) {
        return Err(Error::TooLarge);
    }
    Ok(Layout {
        window_frames,
        table_off,
        checked,
        head,
        wbytes,
        total: total as usize,
    })
}

/// Bytes of the record [`write_record`] makes from `p`.
pub fn record_len(p: &RecordParams) -> Result<usize, Error> {
    layout(p).map(|l| l.total)
}

/// Writes a complete record into `out[..record_len(p)]`: header, credit, frame table, preload
/// window, frames and padding. The header is checked as [`Record::parse`] does, and every frame is
/// decoded and checked as full verification does (CRCs, block sizes, numbering, rate) to fill the
/// window and to compute the MD5 and the sample id. Returns the record's fields (its `offset` is
/// 0: the record does not know where it will be placed).
pub fn write_record(p: &RecordParams, vs: &mut VerifyScratch, out: &mut [u8]) -> Result<RecordInfo, Error> {
    let l = layout(p)?;
    let r = region(out, l.total)?;
    put_prefix(r, MAGIC_RECORD, l.total, l.checked);
    put(r, 24, &(l.head as u32).to_le_bytes());
    put(r, 28, &(l.wbytes as u32).to_le_bytes());
    put(r, 80, &p.rate.to_le_bytes());
    put(r, 84, &[p.channels, 16]);
    put(r, 88, &p.frames.to_le_bytes());
    put(r, 92, &l.window_frames.to_le_bytes());
    put(r, 96, &p.block_size.to_le_bytes());
    put(r, 100, &((p.table.len().wrapping_sub(1)) as u32).to_le_bytes());
    put(r, 104, &(p.frame_data.len() as u32).to_le_bytes());
    let lens = p.table.windows(2).map(|w| match w {
        [a, b] => b.wrapping_sub(*a),
        _ => 0,
    });
    put(r, 108, &lens.clone().min().unwrap_or(0).to_le_bytes());
    put(r, 112, &lens.max().unwrap_or(0).to_le_bytes());
    put(r, 116, &(p.credit.len() as u16).to_le_bytes());
    put(r, RECORD_FIXED, p.credit.as_bytes());
    for (i, v) in p.table.iter().enumerate() {
        put(r, l.table_off.wrapping_add(i.wrapping_mul(4)), &v.to_le_bytes());
    }
    put(r, l.head.wrapping_add(l.wbytes), p.frame_data);
    seal(r, l.checked);
    // The header so far (id and MD5 still zero) must pass the reader's checks.
    let info = *Record::parse(r, 0)?.info();
    let wlen = (l.window_frames as usize)
        .wrapping_mul(usize::from(p.channels))
        .wrapping_mul(2);
    let mut wpos = 0usize;
    let frames = p.table.windows(2).map(|w| match w {
        [a, b] => p.frame_data.get(*a as usize..*b as usize),
        _ => None,
    });
    let (md5, id) = decode_audio(&info, frames, vs, |audio| {
        let k = audio.len().min(wlen.saturating_sub(wpos));
        put(r, l.head.wrapping_add(wpos), audio.get(..k).unwrap_or(&[]));
        wpos = wpos.wrapping_add(k);
        Ok(())
    })?;
    put(r, 32, &id);
    put(r, 64, &md5);
    seal(r, l.checked);
    Ok(RecordInfo { id, md5, ..info })
}
