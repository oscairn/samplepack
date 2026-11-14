// SPDX-License-Identifier: MIT
//! A small FLAC encoder that writes only the subset the reader accepts: 16-bit, 1 or 2 channels,
//! fixed block size, fixed predictors of order 0 to 4 (or constant / verbatim subframes), Rice
//! residuals with partition orders 0 to 8, and the four stereo decorrelation modes. Output depends
//! only on the input samples, so packing is deterministic.

use samplepack::flac::{crc16, crc8};

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            n: 0,
        }
    }
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

const MAX_ORDER: usize = 4;
const MAX_PORDER: u32 = 8;

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// Residuals of fixed predictors of every order 0..=4: `res[k][i]` for `i >= k`.
fn residuals(x: &[i32]) -> [Vec<i64>; MAX_ORDER + 1] {
    let mut r: [Vec<i64>; MAX_ORDER + 1] = Default::default();
    r[0] = x.iter().map(|&v| i64::from(v)).collect();
    for k in 1..=MAX_ORDER {
        let prev = &r[k - 1];
        let mut cur = vec![0i64; x.len()];
        for i in k..x.len() {
            cur[i] = prev[i] - prev[i - 1];
        }
        r[k] = cur;
    }
    r
}

/// Estimated Rice cost of `count` values summing to `sum`, and the parameter (0..=30) that gives it.
fn rice_param(sum: u64, count: u64) -> (u64, u32) {
    let mut best = (u64::MAX, 0);
    for p in 0..=30u32 {
        let c = count * (u64::from(p) + 1) + (sum >> p);
        if c < best.0 {
            best = (c, p);
        }
    }
    best
}

#[derive(Clone)]
struct Plan {
    order: usize,
    porder: u32,
    params: Vec<u32>,
    bits: u64,
}

/// Best partitioning for the order-`k` residual of an `n`-sample block, by estimated size.
fn plan_residual(res: &[i64], k: usize, bps: u32) -> Option<Plan> {
    let n = res.len();
    let mut best: Option<Plan> = None;
    for po in 0..=MAX_PORDER {
        let parts = 1usize << po;
        if !n.is_multiple_of(parts) || n / parts < k {
            continue;
        }
        let psize = n / parts;
        let mut params = Vec::with_capacity(parts);
        let mut bits = 2 + 4 + u64::from(bps) * k as u64;
        for p in 0..parts {
            let start = if p == 0 { k } else { p * psize };
            let end = (p + 1) * psize;
            let sum: u64 = res[start..end].iter().map(|&v| zigzag(v)).sum();
            let (c, prm) = rice_param(sum, (end - start) as u64);
            params.push(prm);
            bits += c + 5;
        }
        if best.as_ref().is_none_or(|b| bits < b.bits) {
            best = Some(Plan {
                order: k,
                porder: po,
                params,
                bits,
            });
        }
    }
    best
}

enum Sub {
    Constant(i32),
    Verbatim,
    Fixed(Plan),
}

/// The cheapest subframe for `x` at `bps` bits, and its estimated size in bits.
fn plan_subframe(x: &[i32], bps: u32) -> (Sub, u64) {
    let n = x.len() as u64;
    if x.iter().all(|&v| v == x[0]) {
        return (Sub::Constant(x[0]), 8 + u64::from(bps));
    }
    let mut best = (Sub::Verbatim, 8 + n * u64::from(bps));
    let res = residuals(x);
    for (k, r) in res.iter().enumerate() {
        if k > x.len() {
            break;
        }
        if let Some(p) = plan_residual(r, k, bps) {
            if 8 + p.bits < best.1 {
                best = (Sub::Fixed(p.clone()), 8 + p.bits);
            }
        }
    }
    best
}

fn write_subframe(w: &mut Bits, x: &[i32], bps: u32, sub: &Sub) {
    let mask = |v: i32| u64::from(v as u32) & ((1u64 << bps) - 1);
    match sub {
        Sub::Constant(v) => {
            w.put(8, 0);
            w.put(bps, mask(*v));
        }
        Sub::Verbatim => {
            w.put(8, 1 << 1);
            for &v in x {
                w.put(bps, mask(v));
            }
        }
        Sub::Fixed(p) => {
            w.put(8, ((8 + p.order) as u64) << 1);
            for &v in &x[..p.order] {
                w.put(bps, mask(v));
            }
            let res = residuals(x);
            let r = &res[p.order];
            let wide = p.params.iter().any(|&k| k > 14);
            w.put(2, u64::from(wide));
            w.put(4, u64::from(p.porder));
            let psize = x.len() >> p.porder;
            for (i, &k) in p.params.iter().enumerate() {
                w.put(if wide { 5 } else { 4 }, u64::from(k));
                let start = if i == 0 { p.order } else { i * psize };
                for &v in &r[start..(i + 1) * psize] {
                    let u = zigzag(v);
                    w.unary(u >> k);
                    w.put(k, u);
                }
            }
        }
    }
}

const RATE_CODES: [(u32, u8); 11] = [
    (88200, 1),
    (176400, 2),
    (192000, 3),
    (8000, 4),
    (16000, 5),
    (22050, 6),
    (24000, 7),
    (32000, 8),
    (44100, 9),
    (48000, 10),
    (96000, 11),
];

fn utf8_number(v: u32) -> Vec<u8> {
    if v < 0x80 {
        return vec![v as u8];
    }
    let n = match v {
        0..=0x7FF => 2,
        0x800..=0xFFFF => 3,
        0x1_0000..=0x1F_FFFF => 4,
        0x20_0000..=0x3FF_FFFF => 5,
        _ => 6,
    };
    let lead = [0u8, 0, 0xC0, 0xE0, 0xF0, 0xF8, 0xFC][n];
    let mut out = vec![0u8; n];
    let mut x = v;
    for i in (1..n).rev() {
        out[i] = 0x80 | (x & 0x3F) as u8;
        x >>= 6;
    }
    out[0] = lead | x as u8;
    out
}

/// Encodes one frame: `chans` holds 1 or 2 channels of `n` samples each.
fn encode_frame(chans: &[Vec<i32>], rate: u32, index: u32) -> Vec<u8> {
    let n = chans[0].len();
    let block_code: Option<u8> = match n {
        192 => Some(1),
        576 => Some(2),
        1152 => Some(3),
        2304 => Some(4),
        256 => Some(8),
        512 => Some(9),
        1024 => Some(10),
        2048 => Some(11),
        4096 => Some(12),
        _ => None,
    };
    let (bcode, bextra): (u8, Vec<u8>) = match block_code {
        Some(c) => (c, vec![]),
        None if n <= 256 => (6, vec![(n - 1) as u8]),
        None => (7, ((n - 1) as u16).to_be_bytes().to_vec()),
    };
    let (rcode, rextra): (u8, Vec<u8>) = match RATE_CODES.iter().find(|(r, _)| *r == rate) {
        Some(&(_, c)) => (c, vec![]),
        None if rate.is_multiple_of(1000) && rate / 1000 <= 255 => (12, vec![(rate / 1000) as u8]),
        None if rate <= 65535 => (13, (rate as u16).to_be_bytes().to_vec()),
        None if rate.is_multiple_of(10) && rate / 10 <= 65535 => (14, ((rate / 10) as u16).to_be_bytes().to_vec()),
        None => (0, vec![]),
    };
    // Stereo: choose among independent, left/side, side/right and mid/side by estimated size.
    let (assign, subs): (u8, Vec<(Vec<i32>, u32)>) = if chans.len() == 1 {
        (0, vec![(chans[0].clone(), 16)])
    } else {
        let (l, r) = (&chans[0], &chans[1]);
        let side: Vec<i32> = l.iter().zip(r).map(|(a, b)| a - b).collect();
        let mid: Vec<i32> = l.iter().zip(r).map(|(a, b)| (a + b) >> 1).collect();
        let cost = |x: &[i32], bps| plan_subframe(x, bps).1;
        let (cl, cr, cs, cm) = (cost(l, 16), cost(r, 16), cost(&side, 17), cost(&mid, 16));
        let options = [(1u8, cl + cr), (8, cl + cs), (9, cs + cr), (10, cm + cs)];
        let mut best = options[0];
        for o in options {
            if o.1 < best.1 {
                best = o;
            }
        }
        let pair = match best.0 {
            1 => vec![(l.clone(), 16), (r.clone(), 16)],
            8 => vec![(l.clone(), 16), (side, 17)],
            9 => vec![(side, 17), (r.clone(), 16)],
            _ => vec![(mid, 16), (side, 17)],
        };
        (best.0, pair)
    };
    let mut head = vec![0xFF, 0xF8, (bcode << 4) | rcode, (assign << 4) | (4 << 1)];
    head.extend(utf8_number(index));
    head.extend(bextra);
    head.extend(rextra);
    head.push(crc8(&head));
    let mut w = Bits::new();
    w.out = head;
    for (x, bps) in &subs {
        let (sub, _) = plan_subframe(x, *bps);
        write_subframe(&mut w, x, *bps, &sub);
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
