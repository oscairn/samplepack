// SPDX-License-Identifier: AGPL-3.0-or-later
//! MSB-first bit reader over a byte slice.
//!
//! Reads are bounded by the end of the slice: bytes past the end read as zero, and the reader
//! latches `bad` once it has loaded more than 8 bytes past the end (everything after that reads
//! as ones). The frame decoder also rejects any frame whose consumed length exceeds the slice, so
//! zero padding never makes a truncated frame decode successfully.

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// Index of the next byte to load into the cache.
    pub(crate) p: usize,
    /// Bit cache, next bit at the top; bits below `n` are zero.
    c: u64,
    /// Number of valid bits in the cache.
    pub(crate) n: u32,
    pub(crate) bad: bool,
}

fn load8(data: &[u8], p: usize) -> u64 {
    if let Some(b) = data.get(p..).and_then(|s| s.first_chunk::<8>()) {
        return u64::from_be_bytes(*b);
    }
    load8_tail(data, p)
}

#[cold]
fn load8_tail(data: &[u8], p: usize) -> u64 {
    let mut b = [0u8; 8];
    for (d, s) in b.iter_mut().zip(data.get(p..).unwrap_or(&[])) {
        *d = *s;
    }
    u64::from_be_bytes(b)
}

impl<'a> BitReader<'a> {
    /// Reader positioned at byte `start` of `data`.
    pub(crate) fn new(data: &'a [u8], start: usize) -> Self {
        Self {
            data,
            p: start,
            c: 0,
            n: 0,
            bad: false,
        }
    }

    fn limit(&self) -> usize {
        self.data.len().wrapping_add(8)
    }

    #[inline]
    fn fill(&mut self) {
        if self.p > self.limit() {
            self.bad = true;
            self.c = !0;
            self.n = 64;
            return;
        }
        let v = load8(self.data, self.p);
        self.c |= v.wrapping_shr(self.n);
        let take = 63u32.wrapping_sub(self.n) >> 3;
        self.p = self.p.wrapping_add(take as usize);
        self.n = self.n.wrapping_add(take << 3);
    }

    /// Reads `bits` (0..=32) bits as an unsigned value.
    #[inline]
    pub(crate) fn get(&mut self, bits: u32) -> u32 {
        if self.n < bits {
            self.fill();
        }
        let v = ((self.c >> 1) >> (63u32.wrapping_sub(bits) & 63)) as u32;
        self.c = self.c.wrapping_shl(bits);
        self.n = self.n.wrapping_sub(bits);
        v
    }

    /// Reads `bits` (1..=32) bits as a two's-complement value.
    #[inline]
    pub(crate) fn get_signed(&mut self, bits: u32) -> i32 {
        let sh = 32u32.wrapping_sub(bits) & 31;
        ((self.get(bits) << sh) as i32) >> sh
    }

    /// Total number of bits consumed from the start of the data.
    pub(crate) fn consumed_bits(&self) -> u64 {
        (self.p as u64).wrapping_mul(8).wrapping_sub(u64::from(self.n))
    }

    #[cold]
    fn rice_slow(&mut self, k: u32) -> u32 {
        let mut q: u32 = 0;
        let z = loop {
            if self.n < 32 {
                self.fill();
            }
            if self.bad {
                return 0;
            }
            let z = if self.c != 0 { self.c.leading_zeros() } else { 64 };
            if z < self.n {
                break z;
            }
            q = q.wrapping_add(self.n);
            self.c = 0;
            self.n = 0;
        };
        self.c = self.c.wrapping_shl(z.wrapping_add(1));
        self.n = self.n.wrapping_sub(z.wrapping_add(1));
        q = q.wrapping_add(z);
        let lo = if k != 0 { self.get(k) } else { 0 };
        q.wrapping_shl(k) | lo
    }

    /// Decodes `out.len()` Rice codes with parameter `k` (0..=30), mapping each to a signed value.
    /// Sets `bad` if the data runs out.
    pub(crate) fn rice_run(&mut self, k: u32, out: &mut [i32]) {
        let mut c = self.c;
        let mut n = self.n;
        let mut p = self.p;
        let limit = self.limit();
        let k1 = k.wrapping_add(1);
        for o in out.iter_mut() {
            if n < 40 {
                let v = match self.data.get(p..).and_then(|s| s.first_chunk::<8>()) {
                    Some(b) => u64::from_be_bytes(*b),
                    None if p > limit => {
                        self.bad = true;
                        return;
                    }
                    None => load8_tail(self.data, p),
                };
                c |= v.wrapping_shr(n);
                let take = 63u32.wrapping_sub(n) >> 3;
                p = p.wrapping_add(take as usize);
                n = n.wrapping_add(take << 3);
            }
            let z = (c | 1).leading_zeros();
            let len = z.wrapping_add(k1);
            let u;
            if len <= n {
                // The top `len` bits of the cache are z zeros, the stop bit and k remainder bits, so
                // as a number they are (1 << k) | remainder; the code value is (z << k) | remainder.
                // len <= n <= 63 here, so every shift amount is below 64.
                let t = c.wrapping_shr(64u32.wrapping_sub(len)) as u32;
                u = t.wrapping_add(z.wrapping_sub(1).wrapping_shl(k));
                c = c.wrapping_shl(len);
                n = n.wrapping_sub(len);
            } else {
                self.c = c;
                self.n = n;
                self.p = p;
                u = self.rice_slow(k);
                if self.bad {
                    return;
                }
                c = self.c;
                n = self.n;
                p = self.p;
            }
            *o = ((u >> 1) as i32) ^ ((u & 1) as i32).wrapping_neg();
        }
        self.c = c;
        self.n = n;
        self.p = p;
    }
}
