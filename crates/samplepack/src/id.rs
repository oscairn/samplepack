// SPDX-License-Identifier: MIT
//! Sample ids: SHA-256 over a domain tag, the sample rate, the channel count, the bit depth and
//! the decoded PCM (see `docs/format.md`, "Sample id"). The id of a record is computed by
//! [`crate::pack::record_sample_id`], which decodes the record's frames and feeds them here.

use crate::hash::Sha256;

/// Length of a sample id in bytes.
pub const ID_BYTES: usize = 32;
/// The domain tag that starts every hashed string (17 bytes, the last one zero).
pub const DOMAIN_TAG: &[u8; 17] = b"oscairn-sample-1\0";

/// A sample id.
pub type SampleId = [u8; ID_BYTES];

/// Streaming sample-id computation. Feed every frame of the sample, channels interleaved, in
/// order, then call [`IdHasher::finish`].
#[derive(Clone)]
pub struct IdHasher {
    sha: Sha256,
}

impl IdHasher {
    /// Starts an id for audio at `rate` Hz with `channels` channels of `bits`-bit samples. Version 1
    /// of the format has only 16-bit audio.
    pub fn new(rate: u32, channels: u8, bits: u8) -> Self {
        let mut sha = Sha256::new();
        sha.update(DOMAIN_TAG);
        sha.update(&rate.to_le_bytes());
        sha.update(&[channels, bits]);
        Self { sha }
    }

    /// Feeds PCM already in the canonical byte layout (16-bit signed little-endian, interleaved).
    pub fn update_bytes(&mut self, le: &[u8]) {
        self.sha.update(le);
    }

    /// Feeds interleaved 16-bit samples.
    pub fn update(&mut self, samples: &[i16]) {
        let mut buf = [0u8; 512];
        for chunk in samples.chunks(256) {
            let mut n = 0usize;
            for (d, s) in buf.as_chunks_mut::<2>().0.iter_mut().zip(chunk) {
                *d = s.to_le_bytes();
                n = n.wrapping_add(2);
            }
            self.sha.update(buf.get(..n).unwrap_or(&[]));
        }
    }

    /// The id.
    pub fn finish(self) -> SampleId {
        self.sha.finish()
    }
}

/// The id of 16-bit audio given as interleaved samples (`pcm.len()` = frames x channels).
pub fn sample_id(rate: u32, channels: u8, pcm: &[i16]) -> SampleId {
    let mut h = IdHasher::new(rate, channels, 16);
    h.update(pcm);
    h.finish()
}
