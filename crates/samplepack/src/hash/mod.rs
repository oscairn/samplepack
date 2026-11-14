// SPDX-License-Identifier: MIT
//! Checksums and hashes used by the pack format: CRC-32C (entry checksums), SHA-256 (sample ids)
//! and MD5 (the decoded-audio signature that FLAC's STREAMINFO carries). All are streaming,
//! allocation-free and panic-free.

mod crc32c;
mod md5;
mod sha256;

#[cfg(test)]
pub(crate) use crc32c::CHECKSUMMED;
pub use crc32c::{crc32c, Crc32c};
pub use md5::Md5;
pub use sha256::Sha256;

/// Copies `src` into the front of `dst` as far as both reach; returns the number of bytes copied.
pub(crate) fn copy_into(dst: &mut [u8], src: &[u8]) -> usize {
    let mut n = 0usize;
    for (d, s) in dst.iter_mut().zip(src) {
        *d = *s;
        n = n.wrapping_add(1);
    }
    n
}

/// Shared block buffering for the two Merkle-Damgard hashes (64-byte blocks, 64-bit bit length).
#[derive(Clone)]
pub(crate) struct Blocks {
    buf: [u8; 64],
    n: usize,
    len: u64,
}

impl Blocks {
    pub(crate) const fn new() -> Self {
        Self {
            buf: [0; 64],
            n: 0,
            len: 0,
        }
    }

    /// Feeds `data`, calling `f` on every complete 64-byte block.
    pub(crate) fn update(&mut self, mut data: &[u8], mut f: impl FnMut(&[u8; 64])) {
        self.len = self.len.wrapping_add(data.len() as u64);
        if self.n > 0 {
            let took = copy_into(self.buf.get_mut(self.n..).unwrap_or(&mut []), data);
            self.n = self.n.wrapping_add(took);
            data = data.get(took..).unwrap_or(&[]);
            if self.n < 64 {
                return;
            }
            f(&self.buf);
            self.n = 0;
        }
        let (blocks, rest) = data.as_chunks::<64>();
        for b in blocks {
            f(b);
        }
        self.n = copy_into(&mut self.buf, rest);
    }

    /// Appends the padding (0x80, zeros, bit length in the given byte order) and feeds it.
    pub(crate) fn finish(&mut self, big_endian: bool, mut f: impl FnMut(&[u8; 64])) {
        let bits = self.len.wrapping_mul(8);
        let len = if big_endian {
            bits.to_be_bytes()
        } else {
            bits.to_le_bytes()
        };
        let pad_len = if self.n < 56 {
            56usize.wrapping_sub(self.n)
        } else {
            120usize.wrapping_sub(self.n)
        };
        let mut pad = [0u8; 72];
        pad[0] = 0x80;
        let tail = pad.get_mut(pad_len..pad_len.wrapping_add(8)).unwrap_or(&mut []);
        copy_into(tail, &len);
        let saved = self.len;
        self.update(pad.get(..pad_len.wrapping_add(8)).unwrap_or(&[]), &mut f);
        self.len = saved;
    }
}
