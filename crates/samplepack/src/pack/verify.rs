// SPDX-License-Identifier: MIT
//! Full verification: zero padding, every frame decoded with its CRCs checked and its header
//! compared with the record, the window, the MD5 and the recomputed sample id.

use super::{find, rd_u32, rd_u64, Entry, Error, IndexEntry, Pack, Record, RecordInfo, FOOTER, PAGE, PREFIX};
use crate::flac::{self, Scratch, FLAG_CHECK_CRC, MAX_BLOCK};
use crate::hash::Md5;
use crate::id::{IdHasher, SampleId};

/// Working memory for verification and for writing records (about 64 KiB). Contents between
/// calls are irrelevant.
#[repr(C)]
pub struct VerifyScratch {
    /// Decoder scratch.
    pub flac: Scratch,
    /// Decoded channels.
    pub ch: [[i16; MAX_BLOCK]; 2],
    /// One block, interleaved little-endian bytes.
    pub bytes: [u8; MAX_BLOCK * 4],
}

impl VerifyScratch {
    /// A zeroed scratch.
    pub const fn new() -> Self {
        Self {
            flac: Scratch::new(),
            ch: [[0; MAX_BLOCK]; 2],
            bytes: [0; MAX_BLOCK * 4],
        }
    }
}

impl Default for VerifyScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// Checks the parts of a frame header the decoder does not: fixed blocking strategy, the coded
/// frame number equal to `index` in its shortest form, and a coded sample rate equal to `rate`.
/// The frame has already decoded, so its header is well formed.
pub(crate) fn check_frame_header(frame: &[u8], index: u32, rate: u32) -> Result<(), Error> {
    let bad = Error::FrameMismatch;
    let b1 = frame.get(1).copied().ok_or(bad)?;
    let b2 = frame.get(2).copied().ok_or(bad)?;
    if b1 & 1 != 0 {
        return Err(bad);
    }
    let mut num = [0u8; 7];
    let n = flac::coded_number(index, &mut num);
    if frame.get(4..4usize.wrapping_add(n)) != num.get(..n) {
        return Err(bad);
    }
    let mut pos = 4usize.wrapping_add(n);
    pos = pos.wrapping_add(match b2 >> 4 {
        6 => 1,
        7 => 2,
        _ => 0,
    });
    let at = |i: usize| u32::from(frame.get(pos.wrapping_add(i)).copied().unwrap_or(0));
    let coded = match b2 & 15 {
        0 => rate,
        c @ 1..=11 => flac::SAMPLE_RATES.get(usize::from(c)).copied().unwrap_or(0),
        12 => at(0).wrapping_mul(1000),
        13 => (at(0) << 8) | at(1),
        _ => ((at(0) << 8) | at(1)).wrapping_mul(10),
    };
    if coded != rate {
        return Err(bad);
    }
    Ok(())
}

fn zero(d: &[u8]) -> bool {
    d.iter().all(|&b| b == 0)
}

/// Decodes the `info.blocks` frames that `frames` yields, checking each as section 6.5 requires
/// (CRCs, block size, channels, exact length, frame header), hands every block's audio to `sink`
/// as interleaved 16-bit little-endian bytes, and returns the MD5 and the sample id of the audio.
pub(crate) fn decode_audio<'f>(
    info: &RecordInfo,
    frames: impl Iterator<Item = Option<&'f [u8]>>,
    vs: &mut VerifyScratch,
    mut sink: impl FnMut(&[u8]) -> Result<(), Error>,
) -> Result<([u8; 16], SampleId), Error> {
    let mut md5 = Md5::new();
    let mut id = IdHasher::new(info.rate, info.channels as u8, 16);
    let VerifyScratch {
        flac: fs,
        ch: [l, r],
        bytes,
    } = vs;
    for (blk, frame) in (0..info.blocks).zip(frames) {
        let frame = frame.ok_or(Error::BadFrameTable)?;
        let fi = flac::decode_frame(frame, FLAG_CHECK_CRC, fs, l, r)?;
        if fi.samples != info.block_frames(blk) || fi.channels != info.channels || fi.consumed as usize != frame.len() {
            return Err(Error::FrameMismatch);
        }
        check_frame_header(frame, blk, info.rate)?;
        let out = bytes
            .get_mut(
                ..(fi.samples as usize)
                    .wrapping_mul(info.channels as usize)
                    .wrapping_mul(2),
            )
            .unwrap_or(&mut []);
        if info.channels == 1 {
            for (o, s) in out.as_chunks_mut::<2>().0.iter_mut().zip(l.iter()) {
                *o = s.to_le_bytes();
            }
        } else {
            for (o, (a, c)) in out.as_chunks_mut::<4>().0.iter_mut().zip(l.iter().zip(r.iter())) {
                let [a0, a1] = a.to_le_bytes();
                let [c0, c1] = c.to_le_bytes();
                *o = [a0, a1, c0, c1];
            }
        }
        md5.update(out);
        id.update_bytes(out);
        sink(out)?;
    }
    Ok((md5.finish(), id.finish()))
}

/// Checks the padding, decodes every frame of `rec` comparing the window on the way, and returns
/// the MD5 and the sample id of the decoded audio.
fn decode_pass(rec: &Record, vs: &mut VerifyScratch) -> Result<([u8; 16], SampleId), Error> {
    let i = rec.info();
    let b = rec.bytes();
    let checked = rd_u32(b, 16) as usize;
    let h = i.head_len as usize;
    let window = rec.window();
    let fstart = h.wrapping_add(i.window_bytes as usize);
    let fend = fstart.wrapping_add(i.frame_bytes as usize);
    let credit_end = (i.credit_offset as usize).wrapping_add(i.credit_len as usize);
    let pads = [
        (credit_end, i.table_offset as usize),
        (checked, h),
        (h.wrapping_add(window.len()), fstart),
        (fend, b.len()),
    ];
    if pads.iter().any(|&(s, e)| !b.get(s..e).is_some_and(zero)) {
        return Err(Error::Padding);
    }
    let mut wpos = 0usize;
    let sums = decode_audio(i, (0..i.blocks).map(|blk| rec.frame(blk)), vs, |out| {
        let k = out.len().min(window.len().saturating_sub(wpos));
        if window.get(wpos..wpos.wrapping_add(k)) != out.get(..k) {
            return Err(Error::WindowMismatch);
        }
        wpos = wpos.wrapping_add(k);
        Ok(())
    })?;
    if wpos != window.len() {
        return Err(Error::WindowMismatch);
    }
    Ok(sums)
}

/// Full verification of one record (section "Verification"): padding, frames, window, MD5 and
/// sample id. Costs one full decode plus SHA-256 and MD5 of the audio.
pub fn verify_record(rec: &Record, vs: &mut VerifyScratch) -> Result<(), Error> {
    let (md5, id) = decode_pass(rec, vs)?;
    if md5 != rec.info().md5 {
        return Err(Error::Md5Mismatch);
    }
    if id != rec.info().id {
        return Err(Error::IdMismatch);
    }
    Ok(())
}

/// The sample id computed from a record's decoded audio (the record is checked as by
/// [`verify_record`] except for the stated MD5 and id).
pub fn record_sample_id(rec: &Record, vs: &mut VerifyScratch) -> Result<SampleId, Error> {
    decode_pass(rec, vs).map(|(_, id)| id)
}

/// Full verification of a committed pack: everything [`Pack::open`] checks, every entry's
/// structure and padding, the live set rebuilt from the records equal to the trailer, and
/// [`verify_record`] on every live record. `index` is working memory for the rebuilt index
/// (at least as many slots as the pack has records and tombstones). Returns the live count.
pub fn verify_pack(pack: &Pack, vs: &mut VerifyScratch, index: &mut [IndexEntry]) -> Result<usize, Error> {
    let data = pack.data();
    if pack.minor() == 0 && !data.get(PREFIX..PAGE).is_some_and(zero) {
        return Err(Error::Padding);
    }
    for e in pack.scan() {
        // The bytes after the checked ones, to the end of a tombstone's page or to a trailer's footer.
        let (at, end) = match e.map_err(|(_, err)| err)? {
            Entry::Tombstone { offset, .. } => (offset, PAGE),
            Entry::Trailer { offset } => (
                offset,
                (rd_u64(data, offset.wrapping_add(8)) as usize).wrapping_sub(FOOTER),
            ),
            _ => continue,
        };
        let checked = rd_u32(data, at.wrapping_add(16)) as usize;
        if !data
            .get(at.wrapping_add(checked)..at.wrapping_add(end))
            .is_some_and(zero)
        {
            return Err(Error::Padding);
        }
    }
    let n = pack.rebuild_index(index).map_err(|(e, _)| e)?;
    let live = index.get(..n).unwrap_or(&[]);
    if n != pack.records() || pack.entries().any(|e| find(live, &e.id) != Some(&e)) {
        return Err(Error::TrailerMismatch);
    }
    for e in live {
        verify_record(&pack.record(e)?, vs)?;
    }
    Ok(n)
}
