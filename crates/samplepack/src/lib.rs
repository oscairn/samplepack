// SPDX-License-Identifier: MIT
//! Reader and writer for Oscairn Sample Packs (OSP, `.osp`). `docs/format.md` is the
//! specification.
//!
//! Module boundaries:
//!
//! * [`flac`]: decoder for a restricted subset of standard FLAC frames.
//! * [`pack`]: the container (records, tombstones, trailers, index, verification, writing). It
//!   calls [`flac`], [`hash`] and [`id`].
//! * [`id`]: sample ids (SHA-256 over decoded PCM).
//! * [`hash`]: CRC-32C, SHA-256 and MD5.
//! * [`waveform`]: Oscairn Waveform files (`docs/waveform.md`). It uses [`hash`] and [`id`] only.
//!
//! The crate is `no_std`, allocates nothing and has no dependencies. No value taken from the
//! input can cause a panic: all access is checked, all arithmetic is explicit.
#![no_std]
#![deny(unsafe_code)]

pub mod flac;
pub mod hash;
pub mod id;
pub mod pack;
pub mod waveform;
