// SPDX-License-Identifier: MIT
//! C interface to the `samplepack` crate. The matching header is `include/samplepack.h`; it is
//! written by hand and checked against this file by `crates/samplepack-capi/tests`.
//!
//! This is the only place with `unsafe`: turning caller-supplied pointers and lengths into
//! slices. Everything past that point is safe, checked code in the `samplepack` crate.
#![no_std]

use core::mem::size_of;
use samplepack::flac::{self, Error, FrameEntry, FrameInfo, Scratch};

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

fn overlaps<T>(a: &[T], b: &[T]) -> bool {
    let (a0, b0) = (a.as_ptr() as usize, b.as_ptr() as usize);
    let (a1, b1) = (
        a0.wrapping_add(a.len().wrapping_mul(size_of::<T>())),
        b0.wrapping_add(b.len().wrapping_mul(size_of::<T>())),
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
