// SPDX-License-Identifier: MIT
//! CRC-32C (Castagnoli, reflected polynomial 0x82F63B78, initial value and final XOR 0xFFFFFFFF).

#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // const evaluation: overflow or a bad index is a compile error
const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0x82F6_3B78 } else { c >> 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static TAB: [u32; 256] = table();

/// Streaming CRC-32C.
#[derive(Clone, Copy, Debug)]
pub struct Crc32c(u32);

impl Crc32c {
    /// A fresh checksum.
    pub const fn new() -> Self {
        Self(!0)
    }
    /// Feeds bytes.
    pub fn update(&mut self, data: &[u8]) {
        let mut c = self.0;
        for &b in data {
            c = (c >> 8) ^ TAB.get(usize::from(c as u8 ^ b)).copied().unwrap_or(0);
        }
        self.0 = c;
    }
    /// The checksum of everything fed so far.
    pub const fn finish(self) -> u32 {
        !self.0
    }
}

impl Default for Crc32c {
    fn default() -> Self {
        Self::new()
    }
}

/// CRC-32C of `data`.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut c = Crc32c::new();
    c.update(data);
    c.finish()
}
