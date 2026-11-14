// SPDX-License-Identifier: MIT
//! The Oscairn Sample Pack (OSP) container. `docs/format.md` is the specification; the section
//! names in comments refer to it.
//!
//! Reading works on the whole committed pack as one `&[u8]` (normally a read-only mapping of the
//! file) and returns sub-slices of it: nothing is copied or allocated. Writing is a set of pure
//! functions that serialise entries into caller buffers; the caller does the file I/O and the
//! sync ordering of the append protocol.

mod read;
mod verify;
mod write;

pub use read::{find, find_position, recover, Entry, EntryIter, Pack, Record};
pub use verify::{record_sample_id, verify_pack, verify_record, VerifyScratch};
pub use write::{
    record_len, trailer_len, write_pack_header, write_record, write_tombstone, write_trailer, RecordParams,
};

use crate::flac;
use crate::id::SampleId;

/// Alignment of every entry and its length, in bytes.
pub const PAGE: usize = 4096;
/// Largest pack, in bytes (1 GiB).
pub const MAX_PACK: usize = 1 << 30;
/// Most live records one trailer may list.
pub const MAX_RECORDS: usize = 1 << 20;
/// Longest credit string, in bytes.
pub const MAX_CREDIT: usize = 1024;
/// Largest preload window, in frames.
pub const MAX_WINDOW: u32 = 16384;
/// Default preload window, in frames.
pub const DEFAULT_WINDOW: u32 = 4096;
/// Preload windows are 0 or a multiple of this many frames.
pub const WINDOW_STEP: u32 = 1024;
/// The format major version this library reads and writes.
pub const MAJOR: u16 = 1;
/// The format minor version this library writes.
pub const MINOR: u16 = 0;

/// Entry magics.
pub const MAGIC_HEADER: [u8; 4] = *b"OSPH";
/// Record entry.
pub const MAGIC_RECORD: [u8; 4] = *b"OSPR";
/// Tombstone entry.
pub const MAGIC_TOMBSTONE: [u8; 4] = *b"OSPT";
/// Trailer (index) entry.
pub const MAGIC_TRAILER: [u8; 4] = *b"OSPI";
/// Footer, the last 32 bytes of a trailer entry.
pub const MAGIC_FOOTER: [u8; 4] = *b"OSPF";

/// Bytes of the prefix every entry starts with.
pub const PREFIX: usize = 24;
/// Bytes of the fixed part of a record header.
pub const RECORD_FIXED: usize = 128;
/// Bytes of a tombstone's checked part (prefix and sample id).
pub const TOMBSTONE_CHECKED: usize = 56;
/// Bytes before the first trailer entry.
pub const TRAILER_FIXED: usize = 32;
/// Bytes per trailer entry.
pub const TRAILER_ENTRY: usize = 56;
/// Bytes of the footer.
pub const FOOTER: usize = 32;

/// Why a pack, an entry or a record was rejected. [`Error::code`] gives the stable number used by
/// the C interface (`sp_status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A pointer or argument is invalid (C interface), or a writer was given inconsistent input.
    InvalidArgument,
    /// A caller buffer is too small.
    BufferTooSmall,
    /// A frame failed to decode (the decoder's own code).
    Frame(flac::Error),
    /// Not a pack: shorter than the pack header, or the wrong magic.
    NotPack,
    /// A major version other than 1.
    UnsupportedVersion,
    /// The pack does not end in a valid footer: the tail is uncommitted (see [`recover`]).
    NoFooter,
    /// The pack is larger than [`MAX_PACK`].
    TooLarge,
    /// An entry's CRC-32C does not match.
    BadChecksum,
    /// An entry prefix is malformed: wrong magic for its place, length or checked length out of range.
    BadEntry,
    /// The footer's checksum is good but its fields are inconsistent.
    BadFooter,
    /// The trailer's count, length or reserved fields are inconsistent.
    BadTrailer,
    /// An offset, length or preload span is unaligned or outside the allowed range.
    BadOffset,
    /// Records overlap, or the trailer does not list them in file order.
    Overlap,
    /// A reserved field is not zero.
    ReservedNonZero,
    /// Sample rate, channels, bit depth, flags or block size outside version 1.
    BadFormat,
    /// Head, window or record length, window frames or block count not as the format requires.
    BadLayout,
    /// The frame table is not strictly increasing from 0 to the frame bytes, or disagrees with the
    /// stated smallest and largest frame.
    BadFrameTable,
    /// Credit longer than [`MAX_CREDIT`] or not UTF-8.
    BadCredit,
    /// Two live records with the same sample id.
    DuplicateId,
    /// A tombstone for a sample id that is not live at that point.
    NotLive,
    /// A record disagrees with the index entry that points at it.
    Mismatch,
    /// No record with that sample id.
    NotFound,
    /// A frame's block size, channels, frame number, sample rate or length disagrees with the record.
    FrameMismatch,
    /// Padding bytes are not zero.
    Padding,
    /// The preload window differs from the decoded frames.
    WindowMismatch,
    /// The MD5 of the decoded audio differs from the record's.
    Md5Mismatch,
    /// The sample id of the decoded audio differs from the record's.
    IdMismatch,
    /// The live set rebuilt from the records differs from the trailer.
    TrailerMismatch,
    /// The entry at that offset is not a record.
    NotRecord,
}

impl Error {
    /// The stable numeric code (`sp_status` in `include/samplepack.h`).
    pub fn code(self) -> i32 {
        match self {
            Error::InvalidArgument => 1,
            Error::BufferTooSmall => 2,
            Error::Frame(e) => e as i32,
            Error::NotPack => 32,
            Error::UnsupportedVersion => 33,
            Error::NoFooter => 34,
            Error::TooLarge => 35,
            Error::BadChecksum => 36,
            Error::BadEntry => 37,
            Error::BadFooter => 38,
            Error::BadTrailer => 39,
            Error::BadOffset => 40,
            Error::Overlap => 41,
            Error::ReservedNonZero => 42,
            Error::BadFormat => 43,
            Error::BadLayout => 44,
            Error::BadFrameTable => 45,
            Error::BadCredit => 46,
            Error::DuplicateId => 47,
            Error::NotLive => 48,
            Error::Mismatch => 49,
            Error::NotFound => 50,
            Error::FrameMismatch => 51,
            Error::Padding => 52,
            Error::WindowMismatch => 53,
            Error::Md5Mismatch => 54,
            Error::IdMismatch => 55,
            Error::TrailerMismatch => 56,
            Error::NotRecord => 57,
        }
    }
}

impl From<flac::Error> for Error {
    fn from(e: flac::Error) -> Self {
        Error::Frame(e)
    }
}

/// One live record: a trailer entry, and an element of the index the caller builds.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IndexEntry {
    /// Sample id.
    pub id: SampleId,
    /// Offset of the record from the start of the pack.
    pub offset: u64,
    /// Record length in bytes.
    pub length: u64,
    /// Bytes from the record start to the end of its preload window region: the span a loader
    /// reads before the first note.
    pub preload: u64,
}

/// What [`Record`] knows about a record, all taken from its validated header (`sp_record_info`
/// in the C interface).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordInfo {
    /// Offset of the record from the start of the pack.
    pub offset: u64,
    /// Record length in bytes.
    pub length: u64,
    /// Sample id stated by the header (trusted only after full verification).
    pub id: SampleId,
    /// MD5 of the decoded audio stated by the header.
    pub md5: [u8; 16],
    /// Sample rate in Hz.
    pub rate: u32,
    /// Channels (1 or 2).
    pub channels: u32,
    /// Bits per sample (16).
    pub bits: u32,
    /// Frames (samples per channel).
    pub frames: u32,
    /// Frames held in the preload window.
    pub window_frames: u32,
    /// Frames per FLAC block (every block but the last).
    pub block_size: u32,
    /// Number of FLAC frames.
    pub blocks: u32,
    /// Bytes of FLAC frames.
    pub frame_bytes: u32,
    /// Bytes from the record start to the preload window (H).
    pub head_len: u32,
    /// Bytes reserved for the window, padded (W).
    pub window_bytes: u32,
    /// Offset of the credit string within the record.
    pub credit_offset: u32,
    /// Bytes of credit.
    pub credit_len: u32,
    /// Offset of the frame table within the record.
    pub table_offset: u32,
    /// Smallest frame in bytes.
    pub min_frame: u32,
    /// Largest frame in bytes.
    pub max_frame: u32,
    /// Minor version of the record entry.
    pub minor: u32,
}

impl RecordInfo {
    /// Frames (samples per channel) in block `i`; 0 past the end.
    pub fn block_frames(&self, i: u32) -> u32 {
        let start = u64::from(i).wrapping_mul(u64::from(self.block_size));
        let left = u64::from(self.frames).saturating_sub(start);
        left.min(u64::from(self.block_size)) as u32
    }
}

// --- little helpers shared by the reader and the writer ---

pub(crate) fn rd_u16(d: &[u8], at: usize) -> u16 {
    d.get(at..)
        .and_then(|s| s.first_chunk::<2>())
        .map_or(0, |b| u16::from_le_bytes(*b))
}
pub(crate) fn rd_u32(d: &[u8], at: usize) -> u32 {
    d.get(at..)
        .and_then(|s| s.first_chunk::<4>())
        .map_or(0, |b| u32::from_le_bytes(*b))
}
pub(crate) fn rd_u64(d: &[u8], at: usize) -> u64 {
    d.get(at..)
        .and_then(|s| s.first_chunk::<8>())
        .map_or(0, |b| u64::from_le_bytes(*b))
}
pub(crate) fn rd_id(d: &[u8], at: usize) -> SampleId {
    d.get(at..)
        .and_then(|s| s.first_chunk::<32>())
        .copied()
        .unwrap_or([0; 32])
}

/// `v` rounded up to a multiple of [`PAGE`], if that fits.
pub(crate) fn page_up(v: u64) -> Option<u64> {
    v.checked_add(PAGE as u64)
        .map(|x| x.wrapping_sub(1) & !(PAGE as u64).wrapping_sub(1))
}

pub(crate) fn usize_of(v: u64) -> Result<usize, Error> {
    usize::try_from(v).map_err(|_| Error::BadOffset)
}

/// CRC-32C of `d[..checked]` with the checksum field (bytes 20..24) read as zero.
pub(crate) fn entry_crc(d: &[u8], checked: usize) -> Option<u32> {
    let body = d.get(..checked)?;
    let mut c = crate::hash::Crc32c::new();
    c.update(body.get(..20)?);
    c.update(&[0; 4]);
    c.update(body.get(PREFIX..)?);
    Some(c.finish())
}
