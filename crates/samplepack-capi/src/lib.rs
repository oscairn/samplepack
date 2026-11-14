// SPDX-License-Identifier: MIT
//! C interface to the `samplepack` crate (FLAC subset decoder, Oscairn Sample Pack reader, waveform files). The matching header is `include/samplepack.h`; it is
//! written by hand and checked against this file by `tests/c_abi.rs`.
//!
//! This is the only place with `unsafe`: turning caller-supplied pointers and lengths into
//! slices. Everything past that point is safe, checked code in the `samplepack` crate.
#![no_std]

use core::mem::size_of;
use samplepack::flac::{self, Error, FrameEntry, FrameInfo, Scratch};
use samplepack::id::{IdHasher, SampleId};
use samplepack::pack::{self, IndexEntry, Pack, Record, RecordInfo, VerifyScratch};
use samplepack::waveform::{self, KeyBank, Waveform};

/// The decoder never panics. If a panic is reached anyway, abort the process (no unwinding
/// across the C boundary). With the `link-fail-on-panic` feature the handler instead calls a
/// symbol that does not exist, so a program linked with dead-code removal fails to link if any
/// panic path survives optimisation (see `tools/check-no-panic.sh`).
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    #[cfg(feature = "link-fail-on-panic")]
    extern "C" {
        fn samplepack_panic_reachable() -> !;
    }
    #[cfg(not(feature = "link-fail-on-panic"))]
    extern "C" {
        fn abort() -> !;
    }
    // SAFETY: both are argument-less functions that never return; `abort` is the C library's.
    unsafe {
        #[cfg(feature = "link-fail-on-panic")]
        samplepack_panic_reachable();
        #[cfg(not(feature = "link-fail-on-panic"))]
        abort()
    }
}

const OK: i32 = 0;

fn status(r: Result<(), Error>) -> i32 {
    match r {
        Ok(()) => OK,
        Err(e) => e as i32,
    }
}

/// # Safety
/// `p` must be valid for reads of `len` bytes when `len > 0`.
unsafe fn input<'a>(p: *const u8, len: usize) -> Result<&'a [u8], Error> {
    if len == 0 {
        return Ok(&[]);
    }
    if p.is_null() || len > isize::MAX as usize {
        return Err(Error::InvalidArgument);
    }
    // SAFETY: non-null, length bounded, readability is the caller's contract.
    Ok(unsafe { core::slice::from_raw_parts(p, len) })
}

/// A null pointer or zero capacity gives an empty slice; at most `max` elements are used.
///
/// # Safety
/// `p` must be valid for writes of `cap` elements when non-null.
unsafe fn output<'a, T>(p: *mut T, cap: usize, max: usize) -> Result<&'a mut [T], Error> {
    let cap = cap.min(max);
    if p.is_null() || cap == 0 {
        return Ok(&mut []);
    }
    if !p.is_aligned() || cap.checked_mul(size_of::<T>()).is_none_or(|b| b > isize::MAX as usize) {
        return Err(Error::InvalidArgument);
    }
    // SAFETY: non-null, aligned, size bounded, writability is the caller's contract.
    Ok(unsafe { core::slice::from_raw_parts_mut(p, cap) })
}

fn overlaps<A, B>(a: &[A], b: &[B]) -> bool {
    let (a0, b0) = (a.as_ptr() as usize, b.as_ptr() as usize);
    let (a1, b1) = (
        a0.wrapping_add(a.len().wrapping_mul(size_of::<A>())),
        b0.wrapping_add(b.len().wrapping_mul(size_of::<B>())),
    );
    !a.is_empty() && !b.is_empty() && a0 < b1 && b0 < a1
}

fn scratch<'a>(p: *mut Scratch) -> Result<&'a mut Scratch, Error> {
    if p.is_null() || !p.is_aligned() {
        return Err(Error::InvalidArgument);
    }
    // SAFETY: non-null, aligned; the caller guarantees `size_of::<Scratch>()` writable bytes that no
    // other thread uses during the call (documented in the header).
    Ok(unsafe { &mut *p })
}

/// Decodes the frame at the start of `data`; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_flac_decode_frame(
    data: *const u8,
    len: usize,
    flags: u32,
    scratch_ptr: *mut Scratch,
    ch0: *mut i16,
    ch1: *mut i16,
    capacity: usize,
    info: *mut FrameInfo,
) -> i32 {
    if info.is_null() || !info.is_aligned() {
        return Error::InvalidArgument as i32;
    }
    // SAFETY: non-null and aligned; the caller provides a writable `sp_flac_frame_info`.
    unsafe { info.write(FrameInfo::default()) };
    let run = || -> Result<FrameInfo, Error> {
        // SAFETY: forwarded caller contract.
        let (data, o0, o1) = unsafe {
            (
                input(data, len)?,
                output(ch0, capacity, flac::MAX_BLOCK)?,
                output(ch1, capacity, flac::MAX_BLOCK)?,
            )
        };
        if overlaps(o0, o1) {
            return Err(Error::InvalidArgument);
        }
        flac::decode_frame(data, flags, scratch(scratch_ptr)?, o0, o1)
    };
    match run() {
        Ok(i) => {
            // SAFETY: as above.
            unsafe { info.write(i) };
            OK
        }
        Err(e) => e as i32,
    }
}

/// Totals reported by [`sp_flac_scan`].
#[repr(C)]
pub struct ScanSummary {
    frames: u64,
    consumed: u64,
    total_samples: u64,
}

/// Finds frame boundaries; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_flac_scan(
    data: *const u8,
    len: usize,
    flags: u32,
    scratch_ptr: *mut Scratch,
    table: *mut FrameEntry,
    table_capacity: usize,
    summary: *mut ScanSummary,
) -> i32 {
    if summary.is_null() || !summary.is_aligned() {
        return Error::InvalidArgument as i32;
    }
    // SAFETY: non-null and aligned; the caller provides a writable summary.
    unsafe {
        summary.write(ScanSummary {
            frames: 0,
            consumed: 0,
            total_samples: 0,
        })
    };
    // SAFETY: forwarded caller contract.
    let args = unsafe { input(data, len).and_then(|d| output(table, table_capacity, usize::MAX).map(|t| (d, t))) };
    let (d, t) = match args {
        Ok(v) => v,
        Err(e) => return e as i32,
    };
    let s = match scratch(scratch_ptr) {
        Ok(s) => s,
        Err(e) => return e as i32,
    };
    let r = flac::scan(d, flags, s, t);
    // SAFETY: as above.
    unsafe {
        summary.write(ScanSummary {
            frames: r.frames as u64,
            consumed: r.consumed as u64,
            total_samples: r.total_samples,
        })
    };
    status(r.status)
}

/// Finds the first frame of a `.flac` file; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_flac_first_frame(data: *const u8, len: usize, offset: *mut u64) -> i32 {
    if offset.is_null() || !offset.is_aligned() {
        return Error::InvalidArgument as i32;
    }
    // SAFETY: non-null and aligned.
    unsafe { offset.write(0) };
    // SAFETY: forwarded caller contract.
    match unsafe { input(data, len) }.and_then(flac::first_frame) {
        Ok(o) => {
            // SAFETY: as above.
            unsafe { offset.write(o as u64) };
            OK
        }
        Err(e) => e as i32,
    }
}

// ---------------------------------------------------------------------------------------------
// Packs. Every function takes the committed pack as (data, len): normally a read-only mapping of
// the whole file, or of its first `committed` bytes after sp_pack_recover.

fn pcode(r: Result<(), pack::Error>) -> i32 {
    match r {
        Ok(()) => OK,
        Err(e) => e.code(),
    }
}

/// # Safety
/// `p` must be null or valid for writes of one `T`.
unsafe fn out_ref<'a, T>(p: *mut T) -> Result<&'a mut T, pack::Error> {
    if p.is_null() || !p.is_aligned() {
        return Err(pack::Error::InvalidArgument);
    }
    // SAFETY: non-null, aligned; writability is the caller's contract.
    Ok(unsafe { &mut *p })
}

/// # Safety
/// `p` must be null or valid for reads of one `T`.
unsafe fn in_ref<'a, T>(p: *const T) -> Result<&'a T, pack::Error> {
    if p.is_null() || !p.is_aligned() {
        return Err(pack::Error::InvalidArgument);
    }
    // SAFETY: non-null, aligned; readability is the caller's contract.
    Ok(unsafe { &*p })
}

/// # Safety
/// `p` must be valid for reads of `len` bytes when `len > 0`.
unsafe fn pack_bytes<'a>(p: *const u8, len: usize) -> Result<&'a [u8], pack::Error> {
    // SAFETY: forwarded caller contract.
    unsafe { input(p, len) }.map_err(|_| pack::Error::InvalidArgument)
}

/// # Safety
/// `p` must be null or valid for reads of `n` elements.
unsafe fn in_slice<'a, T>(p: *const T, n: usize) -> Result<&'a [T], pack::Error> {
    if n == 0 {
        return Ok(&[]);
    }
    if p.is_null() || !p.is_aligned() || n.checked_mul(size_of::<T>()).is_none_or(|b| b > isize::MAX as usize) {
        return Err(pack::Error::InvalidArgument);
    }
    // SAFETY: non-null, aligned, size bounded; readability is the caller's contract.
    Ok(unsafe { core::slice::from_raw_parts(p, n) })
}

/// Summary of an opened pack.
#[repr(C)]
#[derive(Default)]
pub struct PackInfo {
    length: u64,
    trailer_offset: u64,
    records: u64,
    minor: u32,
    reserved: u32,
}

/// Opens and checks a pack; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_pack_open(data: *const u8, len: usize, info: *mut PackInfo) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let info = out_ref(info)?;
        *info = PackInfo::default();
        let p = Pack::open(pack_bytes(data, len)?)?;
        *info = PackInfo {
            length: p.data().len() as u64,
            trailer_offset: p.trailer_offset() as u64,
            records: p.records() as u64,
            minor: u32::from(p.minor()),
            reserved: 0,
        };
        Ok(())
    };
    pcode(run())
}

/// Committed length of a pack with a torn or uncommitted tail; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_pack_recover(data: *const u8, len: usize, committed: *mut u64) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let c = out_ref(committed)?;
        *c = 0;
        *c = pack::recover(pack_bytes(data, len)?)? as u64;
        Ok(())
    };
    pcode(run())
}

/// # Safety
/// Pointers must be valid as documented in the header.
unsafe fn index_common(
    data: *const u8,
    len: usize,
    table: *mut IndexEntry,
    capacity: usize,
    count: *mut usize,
    rebuild: bool,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let count = out_ref(count)?;
        *count = 0;
        let p = Pack::open(pack_bytes(data, len)?)?;
        let t = output(table, capacity, usize::MAX).map_err(|_| pack::Error::InvalidArgument)?;
        if rebuild {
            match p.rebuild_index(t) {
                Ok(n) => *count = n,
                Err((e, need)) => {
                    if e == pack::Error::BufferTooSmall {
                        *count = need;
                    }
                    return Err(e);
                }
            }
        } else {
            if t.len() < p.records() {
                *count = p.records();
                return Err(pack::Error::BufferTooSmall);
            }
            *count = p.index(t)?;
        }
        Ok(())
    };
    pcode(run())
}

/// Builds the index from the trailer; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_pack_index(
    data: *const u8,
    len: usize,
    table: *mut IndexEntry,
    capacity: usize,
    count: *mut usize,
) -> i32 {
    // SAFETY: forwarded caller contract.
    unsafe { index_common(data, len, table, capacity, count, false) }
}

/// Rebuilds the index from the records and tombstones; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_pack_rebuild_index(
    data: *const u8,
    len: usize,
    table: *mut IndexEntry,
    capacity: usize,
    count: *mut usize,
) -> i32 {
    // SAFETY: forwarded caller contract.
    unsafe { index_common(data, len, table, capacity, count, true) }
}

/// Finds a sample id in an index; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_index_find(
    table: *const IndexEntry,
    count: usize,
    id: *const SampleId,
    position: *mut usize,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let pos = out_ref(position)?;
        *pos = 0;
        let t = in_slice(table, count)?;
        let id = in_ref(id)?;
        *pos = pack::find_position(t, id).ok_or(pack::Error::NotFound)?;
        Ok(())
    };
    pcode(run())
}

/// Checks a record's header and describes it; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_pack_record(
    data: *const u8,
    len: usize,
    entry: *const IndexEntry,
    info: *mut RecordInfo,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let info = out_ref(info)?;
        *info = RecordInfo::default();
        let r = Record::at(pack_bytes(data, len)?, in_ref(entry)?)?;
        *info = *r.info();
        Ok(())
    };
    pcode(run())
}

/// The record an index entry points at, re-read from the pack in constant time.
///
/// # Safety
/// Pointers must be valid as documented in the header.
unsafe fn record<'a>(data: *const u8, len: usize, entry: *const IndexEntry) -> Result<Record<'a>, pack::Error> {
    // SAFETY: forwarded caller contract.
    unsafe { Record::reopen(pack_bytes(data, len)?, in_ref(entry)?) }
}

/// The preload window of a record; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_record_window(
    data: *const u8,
    len: usize,
    entry: *const IndexEntry,
    window: *mut *const u8,
    bytes: *mut usize,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let (w, b) = (out_ref(window)?, out_ref(bytes)?);
        *w = core::ptr::null();
        *b = 0;
        let s = record(data, len, entry)?.window();
        *w = s.as_ptr();
        *b = s.len();
        Ok(())
    };
    pcode(run())
}

/// The frames and frame table of a record; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_record_frames(
    data: *const u8,
    len: usize,
    entry: *const IndexEntry,
    frames: *mut *const u8,
    frames_len: *mut usize,
    table: *mut *const u8,
    table_entries: *mut usize,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let (f, fl, t, tl) = (
            out_ref(frames)?,
            out_ref(frames_len)?,
            out_ref(table)?,
            out_ref(table_entries)?,
        );
        (*f, *fl, *t, *tl) = (core::ptr::null(), 0, core::ptr::null(), 0);
        let r = record(data, len, entry)?;
        let (fs, ts) = (r.frames(), r.frame_table());
        (*f, *fl, *t, *tl) = (fs.as_ptr(), fs.len(), ts.as_ptr(), ts.len() / 4);
        Ok(())
    };
    pcode(run())
}

/// Decodes a range of blocks of a record; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_record_decode(
    data: *const u8,
    len: usize,
    entry: *const IndexEntry,
    first_block: u32,
    blocks: u32,
    flags: u32,
    scratch_ptr: *mut Scratch,
    ch0: *mut i16,
    ch1: *mut i16,
    capacity: usize,
    frames_written: *mut u64,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let fw = out_ref(frames_written)?;
        *fw = 0;
        let r = record(data, len, entry)?;
        let bad = |_| pack::Error::InvalidArgument;
        let (o0, o1) = (
            output(ch0, capacity, usize::MAX).map_err(bad)?,
            output(ch1, capacity, usize::MAX).map_err(bad)?,
        );
        if overlaps(o0, o1) {
            return Err(pack::Error::InvalidArgument);
        }
        let s = scratch(scratch_ptr).map_err(bad)?;
        *fw = r.decode(first_block, blocks, flags, s, o0, o1)? as u64;
        Ok(())
    };
    pcode(run())
}

/// Sample id of interleaved PCM; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_sample_id_pcm(
    rate: u32,
    channels: u32,
    pcm: *const i16,
    frames: usize,
    id: *mut SampleId,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let out = out_ref(id)?;
        *out = [0; 32];
        if !(1..=2).contains(&channels) || rate == 0 {
            return Err(pack::Error::InvalidArgument);
        }
        let n = frames
            .checked_mul(channels as usize)
            .ok_or(pack::Error::InvalidArgument)?;
        let mut h = IdHasher::new(rate, channels as u8, 16);
        h.update(in_slice(pcm, n)?);
        *out = h.finish();
        Ok(())
    };
    pcode(run())
}

fn verify_scratch<'a>(p: *mut VerifyScratch) -> Result<&'a mut VerifyScratch, pack::Error> {
    // SAFETY: as for `scratch`: the caller provides an exclusive, aligned sp_verify_scratch.
    unsafe { out_ref(p) }
}

/// Sample id computed from a record's decoded audio; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_record_sample_id(
    data: *const u8,
    len: usize,
    entry: *const IndexEntry,
    vs: *mut VerifyScratch,
    id: *mut SampleId,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let out = out_ref(id)?;
        *out = [0; 32];
        let r = Record::at(pack_bytes(data, len)?, in_ref(entry)?)?;
        *out = pack::record_sample_id(&r, verify_scratch(vs)?)?;
        Ok(())
    };
    pcode(run())
}

/// Full verification of one record; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_record_verify(
    data: *const u8,
    len: usize,
    entry: *const IndexEntry,
    vs: *mut VerifyScratch,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let r = Record::at(pack_bytes(data, len)?, in_ref(entry)?)?;
        pack::verify_record(&r, verify_scratch(vs)?)
    };
    pcode(run())
}

/// Full verification of a whole pack; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_pack_verify(
    data: *const u8,
    len: usize,
    vs: *mut VerifyScratch,
    table: *mut IndexEntry,
    capacity: usize,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let p = Pack::open(pack_bytes(data, len)?)?;
        let t = output(table, capacity, usize::MAX).map_err(|_| pack::Error::InvalidArgument)?;
        pack::verify_pack(&p, verify_scratch(vs)?, t).map(|_| ())
    };
    pcode(run())
}

// ---------------------------------------------------------------------------------------------
// Waveforms (docs/waveform.md).

fn wcode(r: Result<(), waveform::Error>) -> i32 {
    match r {
        Ok(()) => OK,
        Err(e) => e.code(),
    }
}

/// A bad pointer or argument, as a waveform error.
fn arg<T, E>(r: Result<T, E>) -> Result<T, waveform::Error> {
    r.map_err(|_| waveform::Error::InvalidArgument)
}

/// Checks a waveform file in full and gives its counts; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_waveform_check(data: *const u8, len: usize, samples: *mut usize, banks: *mut usize) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let (s, k) = (arg(out_ref(samples))?, arg(out_ref(banks))?);
        (*s, *k) = (0, 0);
        let w = Waveform::parse(arg(input(data, len))?)?;
        (*s, *k) = (w.samples().len(), w.banks());
        Ok(())
    };
    wcode(run())
}

/// Sample id `i` of a checked waveform file; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_waveform_sample(data: *const u8, len: usize, i: usize, id: *mut SampleId) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let out = arg(out_ref(id))?;
        *out = [0; 32];
        let w = Waveform::reopen(arg(input(data, len))?)?;
        *out = *w.samples().get(i).ok_or(waveform::Error::InvalidArgument)?;
        Ok(())
    };
    wcode(run())
}

/// Key bank `i` of a checked waveform file; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_waveform_bank(data: *const u8, len: usize, i: usize, bank: *mut KeyBank) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let out = arg(out_ref(bank))?;
        *out = KeyBank::default();
        let w = Waveform::reopen(arg(input(data, len))?)?;
        *out = w.bank(i).ok_or(waveform::Error::InvalidArgument)?;
        Ok(())
    };
    wcode(run())
}

/// Checks a waveform file in full and computes its id; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_waveform_id(data: *const u8, len: usize, id: *mut SampleId) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let out = arg(out_ref(id))?;
        *out = [0; 32];
        *out = Waveform::parse(arg(input(data, len))?)?.id();
        Ok(())
    };
    wcode(run())
}

/// Writes the canonical file for a list of key banks; see `samplepack.h`.
///
/// # Safety
/// Pointers must be valid as documented in the header.
#[no_mangle]
pub unsafe extern "C" fn sp_waveform_build(
    banks: *const KeyBank,
    count: usize,
    ids: *const SampleId,
    id_count: usize,
    out: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> i32 {
    // SAFETY: forwarded caller contract.
    let run = || unsafe {
        let n = arg(out_ref(written))?;
        *n = 0;
        let (b, i) = (arg(in_slice(banks, count))?, arg(in_slice(ids, id_count))?);
        let o = arg(output(out, capacity, usize::MAX))?;
        if overlaps(o, b) || overlaps(o, i) {
            return Err(waveform::Error::InvalidArgument);
        }
        *n = match waveform::build(b, i, o) {
            Err(waveform::Error::BufferTooSmall) => {
                *n = waveform::file_len(b, i)?;
                return Err(waveform::Error::BufferTooSmall);
            }
            r => r?,
        };
        Ok(())
    };
    wcode(run())
}
