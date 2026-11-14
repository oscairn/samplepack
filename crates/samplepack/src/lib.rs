// SPDX-License-Identifier: AGPL-3.0-or-later
//! Reader for sample packs.
//!
//! Module boundaries:
//!
//! * [`flac`]: decoder for a restricted subset of standard FLAC frames (implemented).
//! * `pack` (planned): the container (index, records, trailers). It will call [`flac`] and
//!   nothing else in this crate.
//! * `id` (planned): sample-id hashing. Independent of the decoder.
//!
//! The crate is `no_std`, allocates nothing and has no dependencies. No value taken from the
//! input can cause a panic: all access is checked, all arithmetic is explicit.
#![no_std]
#![deny(unsafe_code)]

pub mod flac;
