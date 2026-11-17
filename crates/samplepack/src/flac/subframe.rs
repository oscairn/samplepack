// SPDX-License-Identifier: MIT
//! Subframes: constant, verbatim, fixed predictors (order 0..=4) and LPC (order 1..=32), with
//! Rice / Rice2 residuals. All arithmetic on sample values wraps in 32 bits (64 bits for wide
//! LPC sums), so malformed streams give defined, reproducible output.

use super::bits::BitReader;
use super::{Error, MAX_LPC_ORDER};
use core::cell::Cell;

/// Residual of one subframe into `s[order..]`.
fn residual(r: &mut BitReader, s: &mut [i32], order: usize) -> Result<(), Error> {
    let n = s.len();
    let method = r.get(2);
    if method > 1 {
        return Err(Error::BadResidual);
    }
    let (pbits, esc) = if method == 1 { (5, 31) } else { (4, 15) };
    let porder = r.get(4);
    let psize = n >> porder;
    if (psize << porder) != n || psize < order {
        return Err(Error::BadResidual);
    }
    let mut i = order;
    let mut pe = 0usize;
    for _ in 0..(1usize << porder) {
        pe = pe.wrapping_add(psize);
        let k = r.get(pbits);
        let part = s.get_mut(i..pe).ok_or(Error::BadResidual)?;
        if k == esc {
            let b = r.get(5);
            for x in part.iter_mut() {
                *x = if b != 0 { r.get_signed(b) } else { 0 };
            }
        } else {
            r.rice_run(k, part);
        }
        i = pe;
        if r.bad {
            return Err(Error::Overrun);
        }
    }
    Ok(())
}

fn fixed_predict(s: &mut [i32], order: usize) {
    let cells = Cell::from_mut(s).as_slice_of_cells();
    match order {
        1 => {
            for w in cells.windows(2) {
                if let [p1, x] = w {
                    x.set(x.get().wrapping_add(p1.get()));
                }
            }
        }
        2 => {
            for w in cells.windows(3) {
                if let [p2, p1, x] = w {
                    x.set(x.get().wrapping_add(p1.get().wrapping_mul(2)).wrapping_sub(p2.get()));
                }
            }
        }
        3 => {
            for w in cells.windows(4) {
                if let [p3, p2, p1, x] = w {
                    let d = p1.get().wrapping_sub(p2.get()).wrapping_mul(3).wrapping_add(p3.get());
                    x.set(x.get().wrapping_add(d));
                }
            }
        }
        4 => {
            for w in cells.windows(5) {
                if let [p4, p3, p2, p1, x] = w {
                    let d = p1
                        .get()
                        .wrapping_add(p3.get())
                        .wrapping_mul(4)
                        .wrapping_sub(p2.get().wrapping_mul(6))
                        .wrapping_sub(p4.get());
                    x.set(x.get().wrapping_add(d));
                }
            }
        }
        _ => {}
    }
}

/// `q` holds the coefficients reversed: `q[0]` applies to the oldest sample of the window.
/// Inlined, so the orders with a literal `order` below get unrolled loops.
#[inline(always)]
fn lpc(s: &mut [i32], q: &[i32], order: usize, sh: u32, wide: bool) {
    let cells = Cell::from_mut(s).as_slice_of_cells();
    let q = q.get(..order).unwrap_or(&[]);
    for w in cells.windows(order.wrapping_add(1)) {
        if let Some((x, hist)) = w.split_last() {
            let d = if wide {
                let mut a = 0u64;
                for (h, c) in hist.iter().zip(q) {
                    a = a.wrapping_add(i64::from(*c).wrapping_mul(i64::from(h.get())) as u64);
                }
                (a as i64).wrapping_shr(sh) as i32
            } else {
                let mut a = 0u32;
                for (h, c) in hist.iter().zip(q) {
                    a = a.wrapping_add((*c as u32).wrapping_mul(h.get() as u32));
                }
                (a as i32).wrapping_shr(sh)
            };
            x.set(x.get().wrapping_add(d));
        }
    }
}

/// Decodes one subframe of `s.len()` samples whose nominal sample width is `bps` bits.
pub(super) fn subframe(r: &mut BitReader, s: &mut [i32], bps: u32) -> Result<(), Error> {
    let n = s.len();
    if r.get(1) != 0 {
        return Err(Error::BadSubframe);
    }
    let ty = r.get(6);
    let mut bps = bps;
    let mut wasted = 0u32;
    if r.get(1) != 0 {
        wasted = 1;
        while r.get(1) == 0 {
            wasted = wasted.wrapping_add(1);
            if wasted >= bps || r.bad {
                return Err(if r.bad { Error::Overrun } else { Error::BadSubframe });
            }
        }
        bps = bps.wrapping_sub(wasted);
    }
    match ty {
        0 => {
            let v = r.get_signed(bps);
            s.fill(v);
        }
        1 => {
            for x in s.iter_mut() {
                *x = r.get_signed(bps);
            }
        }
        8..=12 => {
            let order = (ty as usize).wrapping_sub(8);
            if order > n {
                return Err(Error::BadSubframe);
            }
            for x in s.iter_mut().take(order) {
                *x = r.get_signed(bps);
            }
            residual(r, s, order)?;
            fixed_predict(s, order);
        }
        32..=63 => {
            let order = (ty as usize).wrapping_sub(31);
            if order > MAX_LPC_ORDER {
                return Err(Error::Unsupported);
            }
            if order > n {
                return Err(Error::BadSubframe);
            }
            for x in s.iter_mut().take(order) {
                *x = r.get_signed(bps);
            }
            let prec = r.get(4).wrapping_add(1);
            if prec == 16 {
                return Err(Error::BadSubframe);
            }
            let sh = r.get_signed(5);
            if sh < 0 {
                return Err(Error::BadSubframe);
            }
            let sh = sh as u32;
            let mut q = [0i32; MAX_LPC_ORDER];
            for x in q.iter_mut().take(order).rev() {
                *x = r.get_signed(prec);
            }
            residual(r, s, order)?;
            // 32-bit sums are exact when bps + prec + ceil(log2(order)) <= 32 (as libFLAC decides).
            let lg = usize::BITS.wrapping_sub((order.wrapping_sub(1)).leading_zeros());
            let wide = bps.wrapping_add(prec).wrapping_add(lg) > 32;
            match order {
                1 => lpc(s, &q, 1, sh, wide),
                2 => lpc(s, &q, 2, sh, wide),
                3 => lpc(s, &q, 3, sh, wide),
                4 => lpc(s, &q, 4, sh, wide),
                5 => lpc(s, &q, 5, sh, wide),
                6 => lpc(s, &q, 6, sh, wide),
                7 => lpc(s, &q, 7, sh, wide),
                8 => lpc(s, &q, 8, sh, wide),
                _ => lpc(s, &q, order, sh, wide),
            }
        }
        _ => return Err(Error::BadSubframe),
    }
    if wasted != 0 {
        for x in s.iter_mut() {
            *x = ((*x as u32).wrapping_shl(wasted)) as i32;
        }
    }
    if r.bad {
        Err(Error::Overrun)
    } else {
        Ok(())
    }
}
