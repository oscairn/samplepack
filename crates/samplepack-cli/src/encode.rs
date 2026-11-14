// SPDX-License-Identifier: MIT
//! A small FLAC encoder that writes only the subset the reader accepts: 16-bit, 1 or 2 channels,
//! fixed block size, constant / verbatim / fixed (order 0 to 4) / LPC (order 1 to 4) subframes,
//! Rice residuals with partition orders 0 to 8, and the four stereo decorrelation modes.
//!
//! Every choice (predictor, partitioning, Rice parameters, stereo mode) is made by exact coded
//! size. LPC analysis uses f64 restricted to operations IEEE 754 defines
//! exactly (+, -, *, /, floor, abs, comparisons, integer conversions) in a fixed order: Rust neither
//! reorders nor fuses them (no FMA contraction) and no library function (sin, cos, log, ...) is
//! called, so the output is bit-identical on every target. Output depends only on the input.

use samplepack::flac::{self, crc16, crc8};

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    /// Appends the low `count` (0..=32) bits of `v`.
    fn put(&mut self, count: u32, v: u64) {
        if count == 0 {
            return;
        }
        self.acc = (self.acc << count) | (v & ((1u64 << count) - 1));
        self.n += count;
        while self.n >= 8 {
            self.out.push((self.acc >> (self.n - 8)) as u8);
            self.n -= 8;
        }
    }
    fn unary(&mut self, mut zeros: u64) {
        while zeros >= 32 {
            self.put(32, 0);
            zeros -= 32;
        }
        self.put(zeros as u32 + 1, 1);
    }
    fn align(&mut self) {
        if self.n > 0 {
            self.put(8 - self.n, 0);
        }
    }
}

const MAX_FIXED: usize = 4;
const MAX_LPC: usize = 4;
const MAX_PORDER: u32 = 8;
/// Rice parameters considered: 0..K (method 1 allows up to 30; residuals here stay far below).
const K: usize = 26;
/// LPC coefficient precision in bits.
const PRECISION: u32 = 15;
/// Residuals beyond this magnitude disqualify a predictor (keeps every value in 32 bits).
const RES_LIMIT: i64 = 1 << 30;

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

enum Pred {
    Fixed(usize),
    Lpc { q: Vec<i32>, shift: u32 },
}

impl Pred {
    fn order(&self) -> usize {
        match self {
            Pred::Fixed(k) => *k,
            Pred::Lpc { q, .. } => q.len(),
        }
    }
    fn header_bits(&self, bps: u32) -> u64 {
        let warm = u64::from(bps) * self.order() as u64;
        match self {
            Pred::Fixed(_) => 8 + warm,
            Pred::Lpc { q, .. } => 8 + warm + 4 + 5 + u64::from(PRECISION) * q.len() as u64,
        }
    }
}

/// Residual of `x` under `p` for `i >= order` (entries below are 0), or None if out of range.
fn residual(x: &[i32], p: &Pred) -> Option<Vec<i64>> {
    let n = x.len();
    let mut r = vec![0i64; n];
    match p {
        Pred::Fixed(k) => {
            let mut cur: Vec<i64> = x.iter().map(|&v| i64::from(v)).collect();
            for o in 1..=*k {
                for i in (o..n).rev() {
                    cur[i] -= cur[i - 1];
                }
            }
            r[*k..].copy_from_slice(&cur[*k..]);
        }
        Pred::Lpc { q, shift, .. } => {
            let m = q.len();
            for i in m..n {
                let mut s = 0i64;
                for (j, c) in q.iter().enumerate() {
                    s += i64::from(*c) * i64::from(x[i - 1 - j]);
                }
                r[i] = i64::from(x[i]) - (s >> shift);
            }
        }
    }
    if r.iter().any(|v| v.abs() >= RES_LIMIT) {
        return None;
    }
    Some(r)
}

/// A partitioning of a residual: total bits, partition order, parameters, 5-bit parameters.
struct RicePlan {
    bits: u64,
    porder: u32,
    params: Vec<u32>,
    wide: bool,
}

fn max_porder(n: usize, order: usize) -> Option<u32> {
    (0..=MAX_PORDER)
        .rev()
        .find(|&po| n.is_multiple_of(1 << po) && (n >> po) >= order && (n >> po) > 0)
}

/// The exactly smallest partitioning: for every finest partition and every parameter k the sum of
/// u >> k is kept; coarser partitions are sums of their children.
fn rice_exact(res: &[i64], order: usize) -> Option<RicePlan> {
    let n = res.len();
    let top = max_porder(n, order)?;
    let parts = 1usize << top;
    let psize = n >> top;
    let mut sums = vec![[0u64; K]; parts];
    let mut counts = vec![0u64; parts];
    for p in 0..parts {
        let start = if p == 0 { order } else { p * psize };
        let s = &mut sums[p];
        for &v in &res[start..(p + 1) * psize] {
            let u = zigzag(v);
            for (k, acc) in s.iter_mut().enumerate() {
                *acc += u >> k;
            }
        }
        counts[p] = ((p + 1) * psize - start) as u64;
    }
    let mut best: Option<RicePlan> = None;
    let mut po = top;
    loop {
        let (mut b4, mut b5) = (6u64, 6u64);
        let (mut p4, mut p5) = (Vec::with_capacity(sums.len()), Vec::with_capacity(sums.len()));
        for (s, &c) in sums.iter().zip(&counts) {
            let cost = |k: usize| c * (k as u64 + 1) + s[k];
            let k4 = (0..15).min_by_key(|&k| (cost(k), k)).unwrap_or(0);
            let k5 = (0..K).min_by_key(|&k| (cost(k), k)).unwrap_or(0);
            b4 += 4 + cost(k4);
            b5 += 5 + cost(k5);
            p4.push(k4 as u32);
            p5.push(k5 as u32);
        }
        let cand = if b4 <= b5 {
            RicePlan {
                bits: b4,
                porder: po,
                params: p4,
                wide: false,
            }
        } else {
            RicePlan {
                bits: b5,
                porder: po,
                params: p5,
                wide: true,
            }
        };
        if best.as_ref().is_none_or(|b| cand.bits < b.bits) {
            best = Some(cand);
        }
        if po == 0 {
            break;
        }
        po -= 1;
        sums = sums
            .chunks(2)
            .map(|c| {
                let mut m = c[0];
                for (a, b) in m.iter_mut().zip(&c[1]) {
                    *a += b;
                }
                m
            })
            .collect();
        counts = counts.chunks(2).map(|c| c[0] + c[1]).collect();
    }
    best
}

/// Autocorrelation, lags 0..=MAX_LPC, of `x` under a Welch window (a parabola, so no library
/// function is needed).
fn autocorr(x: &[i32]) -> [f64; MAX_LPC + 1] {
    let n = x.len();
    let half = n as f64 / 2.0;
    let xs: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| {
            let t = (i as f64 + 0.5 - half) / half;
            f64::from(v) * (1.0 - t * t)
        })
        .collect();
    let mut r = [0.0f64; MAX_LPC + 1];
    for (lag, out) in r.iter_mut().enumerate() {
        let mut s = 0.0f64;
        for i in lag..n {
            s += xs[i] * xs[i - lag];
        }
        *out = s;
    }
    r
}

/// Levinson-Durbin recursion: coefficients a (x[n] ~ sum a[j] x[n-1-j]) for orders 1..=MAX_LPC.
fn levinson(r: &[f64; MAX_LPC + 1]) -> Vec<Vec<f64>> {
    let mut out = Vec::new();
    let mut a: Vec<f64> = Vec::new();
    let mut e = r[0];
    for m in 1..=MAX_LPC {
        if e <= 0.0 {
            break;
        }
        let mut acc = r[m];
        for (j, aj) in a.iter().enumerate() {
            acc -= aj * r[m - 1 - j];
        }
        let k = acc / e;
        let prev = a.clone();
        for j in 0..prev.len() {
            a[j] = prev[j] - k * prev[prev.len() - 1 - j];
        }
        a.push(k);
        e *= 1.0 - k * k;
        out.push(a.clone());
    }
    out
}

/// Quantises coefficients to PRECISION-bit integers with the largest shift (0..=15) that fits,
/// carrying each rounding error into the next coefficient.
fn quantise(c: &[f64]) -> Option<Pred> {
    let lim = f64::from((1u32 << (PRECISION - 1)) - 1);
    let cmax = c.iter().fold(0.0f64, |m, v| if v.abs() > m { v.abs() } else { m });
    if cmax == 0.0 || !cmax.is_finite() {
        return None;
    }
    let shift = (0..=15u32).rev().find(|&s| cmax * f64::from(1u32 << s) <= lim)?;
    let scale = f64::from(1u32 << shift);
    let mut err = 0.0f64;
    let mut q = Vec::with_capacity(c.len());
    for v in c {
        let t = v * scale + err;
        let qi = (t + 0.5).floor().clamp(-lim - 1.0, lim);
        err = t - qi;
        q.push(qi as i32);
    }
    Some(Pred::Lpc { q, shift })
}

enum Sub {
    Constant(i32),
    Verbatim,
    Coded(Pred, RicePlan, Vec<i64>),
}

/// The smallest subframe for `x` at `bps` bits among fixed orders 0..=4 and LPC orders 1..=4.
fn best_subframe(x: &[i32], bps: u32) -> (Sub, u64) {
    let n = x.len();
    if x.iter().all(|&v| v == x[0]) {
        return (Sub::Constant(x[0]), 8 + u64::from(bps));
    }
    let mut preds: Vec<Pred> = (0..=MAX_FIXED.min(n)).map(Pred::Fixed).collect();
    if n > MAX_LPC {
        preds.extend(levinson(&autocorr(x)).iter().filter_map(|c| quantise(c)));
    }
    let mut best = (Sub::Verbatim, 8 + n as u64 * u64::from(bps));
    for p in preds {
        let Some(r) = residual(x, &p) else { continue };
        let Some(plan) = rice_exact(&r, p.order()) else {
            continue;
        };
        let bits = p.header_bits(bps) + plan.bits;
        if bits < best.1 {
            best = (Sub::Coded(p, plan, r), bits);
        }
    }
    best
}

fn write_subframe(w: &mut Bits, x: &[i32], bps: u32, sub: &Sub) {
    let mask = |v: i64| (v as u64) & ((1u64 << bps) - 1);
    match sub {
        Sub::Constant(v) => {
            w.put(8, 0);
            w.put(bps, mask(i64::from(*v)));
        }
        Sub::Verbatim => {
            w.put(8, 1 << 1);
            for &v in x {
                w.put(bps, mask(i64::from(v)));
            }
        }
        Sub::Coded(p, plan, r) => {
            let order = p.order();
            match p {
                Pred::Fixed(k) => w.put(8, ((8 + k) as u64) << 1),
                Pred::Lpc { q, .. } => w.put(8, ((32 + q.len() - 1) as u64) << 1),
            }
            for &v in &x[..order] {
                w.put(bps, mask(i64::from(v)));
            }
            if let Pred::Lpc { q, shift } = p {
                w.put(4, u64::from(PRECISION - 1));
                w.put(5, u64::from(*shift));
                for &c in q {
                    w.put(PRECISION, (c as u64) & ((1u64 << PRECISION) - 1));
                }
            }
            w.put(2, u64::from(plan.wide));
            w.put(4, u64::from(plan.porder));
            let psize = x.len() >> plan.porder;
            for (i, &k) in plan.params.iter().enumerate() {
                w.put(if plan.wide { 5 } else { 4 }, u64::from(k));
                let start = if i == 0 { order } else { i * psize };
                for &v in &r[start..(i + 1) * psize] {
                    let u = zigzag(v);
                    w.unary(u >> k);
                    w.put(k, u);
                }
            }
        }
    }
}

/// Encodes one frame: `chans` holds 1 or 2 channels of `n` samples each.
fn encode_frame(chans: &[Vec<i32>], rate: u32, index: u32) -> Vec<u8> {
    let n = chans[0].len();
    let (bcode, bextra): (u8, Vec<u8>) = match flac::BLOCK_SIZES.iter().position(|&b| usize::from(b) == n) {
        Some(c) => (c as u8, vec![]),
        None if n <= 256 => (6, vec![(n - 1) as u8]),
        None => (7, ((n - 1) as u16).to_be_bytes().to_vec()),
    };
    let (rcode, rextra): (u8, Vec<u8>) = match flac::SAMPLE_RATES.iter().position(|&r| r == rate) {
        Some(c) => (c as u8, vec![]),
        None if rate.is_multiple_of(1000) && rate / 1000 <= 255 => (12, vec![(rate / 1000) as u8]),
        None if rate <= 65535 => (13, (rate as u16).to_be_bytes().to_vec()),
        None if rate.is_multiple_of(10) && rate / 10 <= 65535 => (14, ((rate / 10) as u16).to_be_bytes().to_vec()),
        None => (0, vec![]),
    };
    // Stereo: the channel assignment with the smallest exact size.
    let (assign, subs): (u8, Vec<(Vec<i32>, u32, Sub)>) = if chans.len() == 1 {
        let (s, _) = best_subframe(&chans[0], 16);
        (0, vec![(chans[0].clone(), 16, s)])
    } else {
        let (l, r) = (&chans[0], &chans[1]);
        let side: Vec<i32> = l.iter().zip(r).map(|(a, b)| a - b).collect();
        let mid: Vec<i32> = l.iter().zip(r).map(|(a, b)| (a + b) >> 1).collect();
        let (sl, cl) = best_subframe(l, 16);
        let (sr, cr) = best_subframe(r, 16);
        let (ss, cs) = best_subframe(&side, 17);
        let (sm, cm) = best_subframe(&mid, 16);
        let options = [(1u8, cl + cr), (8, cl + cs), (9, cs + cr), (10, cm + cs)];
        let mut best = options[0];
        for o in options {
            if o.1 < best.1 {
                best = o;
            }
        }
        let pair = match best.0 {
            1 => vec![(l.clone(), 16, sl), (r.clone(), 16, sr)],
            8 => vec![(l.clone(), 16, sl), (side, 17, ss)],
            9 => vec![(side, 17, ss), (r.clone(), 16, sr)],
            _ => vec![(mid, 16, sm), (side, 17, ss)],
        };
        (best.0, pair)
    };
    let mut head = vec![0xFF, 0xF8, (bcode << 4) | rcode, (assign << 4) | (4 << 1)];
    let mut num = [0u8; 7];
    let len = flac::coded_number(index, &mut num);
    head.extend(&num[..len]);
    head.extend(bextra);
    head.extend(rextra);
    head.push(crc8(&head));
    let mut w = Bits {
        out: head,
        acc: 0,
        n: 0,
    };
    for (x, bps, sub) in &subs {
        write_subframe(&mut w, x, *bps, sub);
    }
    w.align();
    let c = crc16(&w.out);
    w.out.extend(c.to_be_bytes());
    w.out
}

/// Encodes interleaved 16-bit PCM into FLAC frames of `block` samples; returns the frames and
/// the frame table (`blocks + 1` offsets).
pub fn encode(pcm: &[i16], channels: usize, rate: u32, block: usize) -> (Vec<u8>, Vec<u32>) {
    let frames = pcm.len() / channels;
    let mut data = Vec::new();
    let mut table = vec![0u32];
    let mut index = 0u32;
    let mut start = 0;
    while start < frames {
        let n = block.min(frames - start);
        let chans: Vec<Vec<i32>> = (0..channels)
            .map(|c| (0..n).map(|i| i32::from(pcm[(start + i) * channels + c])).collect())
            .collect();
        data.extend(encode_frame(&chans, rate, index));
        table.push(data.len() as u32);
        index += 1;
        start += n;
    }
    (data, table)
}
