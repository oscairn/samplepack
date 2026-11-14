// SPDX-License-Identifier: MIT
//! MD5 (RFC 1321). Not used for identity or security: only as the decoded-audio signature that a
//! standard FLAC file's STREAMINFO carries.

use super::Blocks;

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8,
    0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340,
    0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87,
    0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
    0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039,
    0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92,
    0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
    0xeb86d391,
];

const S: [[u32; 4]; 4] = [[7, 12, 17, 22], [5, 9, 14, 20], [4, 11, 16, 23], [6, 10, 15, 21]];

fn compress(h: &mut [u32; 4], block: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (mi, c) in m.iter_mut().zip(block.as_chunks::<4>().0) {
        *mi = u32::from_le_bytes(*c);
    }
    let [mut a, mut b, mut c, mut d] = *h;
    for (i, k) in K.iter().enumerate() {
        let round = i >> 4;
        let (f, g) = match round {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), i.wrapping_mul(5).wrapping_add(1)),
            2 => (b ^ c ^ d, i.wrapping_mul(3).wrapping_add(5)),
            _ => (c ^ (b | !d), i.wrapping_mul(7)),
        };
        let s = S.get(round & 3).and_then(|r| r.get(i & 3)).copied().unwrap_or(0);
        let x = a
            .wrapping_add(f)
            .wrapping_add(*k)
            .wrapping_add(m.get(g & 15).copied().unwrap_or(0));
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(x.rotate_left(s));
    }
    for (x, v) in h.iter_mut().zip([a, b, c, d]) {
        *x = x.wrapping_add(v);
    }
}

/// Streaming MD5.
#[derive(Clone)]
pub struct Md5 {
    h: [u32; 4],
    blocks: Blocks,
}

impl Md5 {
    /// A fresh hash.
    pub const fn new() -> Self {
        Self {
            h: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            blocks: Blocks::new(),
        }
    }
    /// Feeds bytes.
    pub fn update(&mut self, data: &[u8]) {
        let h = &mut self.h;
        self.blocks.update(data, |b| compress(h, b));
    }
    /// The digest of everything fed so far.
    pub fn finish(mut self) -> [u8; 16] {
        let h = &mut self.h;
        self.blocks.finish(false, |b| compress(h, b));
        let mut out = [0u8; 16];
        for (o, x) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.h) {
            *o = x.to_le_bytes();
        }
        out
    }
    /// MD5 of `data`.
    pub fn digest(data: &[u8]) -> [u8; 16] {
        let mut s = Self::new();
        s.update(data);
        s.finish()
    }
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}
