// SPDX-License-Identifier: MIT
//! Reading: opening a pack, recovering the committed length, the index, records and frames.

use super::{
    entry_crc, page_up, rd_id, rd_u16, rd_u32, rd_u64, usize_of, Error, IndexEntry, RecordInfo, FOOTER, MAGIC_FOOTER,
    MAGIC_HEADER, MAGIC_RECORD, MAGIC_TOMBSTONE, MAGIC_TRAILER, MAJOR, MAX_CREDIT, MAX_PACK, MAX_RECORDS, MAX_WINDOW,
    PAGE, PREFIX, RECORD_FIXED, TOMBSTONE_CHECKED, TRAILER_ENTRY, TRAILER_FIXED, WINDOW_STEP,
};
use crate::flac::{self, Scratch};
use crate::hash::crc32c;
use crate::id::SampleId;

/// The common entry prefix (section "Entries").
#[derive(Clone, Copy)]
pub(crate) struct Prefix {
    pub(crate) minor: u16,
    pub(crate) length: usize,
    pub(crate) checked: usize,
}

fn magic_of(d: &[u8]) -> [u8; 4] {
    d.first_chunk::<4>().copied().unwrap_or([0; 4])
}

/// Validates the prefix of the entry at the start of `d`, where `d` runs to the end of the region
/// the entry must fit in. The major version is the caller's to check.
pub(crate) fn prefix(d: &[u8]) -> Result<Prefix, Error> {
    let p = prefix_fields(d)?;
    if entry_crc(d, p.checked) != Some(rd_u32(d, 20)) {
        return Err(Error::BadChecksum);
    }
    Ok(p)
}

/// [`prefix`] without the checksum.
fn prefix_fields(d: &[u8]) -> Result<Prefix, Error> {
    if d.len() < PREFIX {
        return Err(Error::BadEntry);
    }
    let length = rd_u64(d, 8);
    let checked = rd_u32(d, 16) as usize;
    let length = usize::try_from(length).map_err(|_| Error::BadEntry)?;
    if length < PAGE || !length.is_multiple_of(PAGE) || length > d.len() || checked < PREFIX || checked > length {
        return Err(Error::BadEntry);
    }
    Ok(Prefix {
        minor: rd_u16(d, 6),
        length,
        checked,
    })
}

/// Checks the pack header; returns its minor version.
fn check_header(data: &[u8]) -> Result<u16, Error> {
    if data.len() < PAGE || magic_of(data) != MAGIC_HEADER {
        return Err(Error::NotPack);
    }
    if rd_u16(data, 4) != MAJOR {
        return Err(Error::UnsupportedVersion);
    }
    let p = prefix(data)?;
    if p.length != PAGE || (p.minor == 0 && p.checked != PREFIX) {
        return Err(Error::BadEntry);
    }
    Ok(p.minor)
}

/// Checks one live-record entry against the end of the previous one and the trailer offset;
/// returns the end of this record.
pub(crate) fn check_entry(e: &IndexEntry, prev_end: u64, limit: u64) -> Result<u64, Error> {
    let p = PAGE as u64;
    if !e.offset.is_multiple_of(p)
        || !e.length.is_multiple_of(p)
        || e.length < p
        || !e.preload.is_multiple_of(p)
        || e.preload < p
        || e.preload > e.length
        || e.offset < p
    {
        return Err(Error::BadOffset);
    }
    if e.offset < prev_end {
        return Err(Error::Overlap);
    }
    let end = e.offset.checked_add(e.length).ok_or(Error::BadOffset)?;
    if end > limit {
        return Err(Error::BadOffset);
    }
    Ok(end)
}

fn trailer_entry(t: &[u8], i: usize) -> IndexEntry {
    let at = TRAILER_ENTRY.wrapping_mul(i).wrapping_add(TRAILER_FIXED);
    IndexEntry {
        id: rd_id(t, at),
        offset: rd_u64(t, at.wrapping_add(32)),
        length: rd_u64(t, at.wrapping_add(40)),
        preload: rd_u64(t, at.wrapping_add(48)),
    }
}

/// An opened pack: header, footer and trailer checked, every trailer entry in bounds, aligned and
/// in file order without overlap. Record headers are checked when a record is accessed.
#[derive(Clone, Copy, Debug)]
pub struct Pack<'a> {
    data: &'a [u8],
    trailer: usize,
    count: usize,
    minor: u16,
}

impl<'a> Pack<'a> {
    /// Opens the committed pack `data` (the whole file). Fails with [`Error::NoFooter`] when the
    /// file does not end in a valid footer; then [`recover`] gives the committed length.
    pub fn open(data: &'a [u8]) -> Result<Self, Error> {
        let minor = check_header(data)?;
        let len = data.len();
        if len > MAX_PACK {
            return Err(Error::TooLarge);
        }
        if len == PAGE {
            return Ok(Self {
                data,
                trailer: len,
                count: 0,
                minor,
            });
        }
        if !len.is_multiple_of(PAGE) {
            return Err(Error::NoFooter);
        }
        let foot = data.get(len.wrapping_sub(FOOTER)..).unwrap_or(&[]);
        if magic_of(foot) != MAGIC_FOOTER || Some(crc32c(foot.get(..28).unwrap_or(&[]))) != Some(rd_u32(foot, 28)) {
            return Err(Error::NoFooter);
        }
        let toff = rd_u64(foot, 8);
        let tlen = rd_u64(foot, 16);
        if !toff.is_multiple_of(PAGE as u64) || toff < PAGE as u64 || toff.checked_add(tlen) != Some(len as u64) {
            return Err(Error::BadFooter);
        }
        let toff = usize_of(toff)?;
        let t = data.get(toff..).unwrap_or(&[]);
        if magic_of(t) != MAGIC_TRAILER || rd_u64(t, 8) != tlen {
            return Err(Error::BadFooter);
        }
        if rd_u16(t, 4) != MAJOR {
            return Err(Error::UnsupportedVersion);
        }
        let p = prefix(t)?;
        let count = rd_u32(t, 24) as usize;
        let need = count
            .checked_mul(TRAILER_ENTRY)
            .and_then(|v| v.checked_add(TRAILER_FIXED))
            .ok_or(Error::BadTrailer)?;
        let fits = need.checked_add(FOOTER).is_some_and(|v| v <= p.length);
        if count > MAX_RECORDS || !fits || p.checked < need {
            return Err(Error::BadTrailer);
        }
        if p.minor == 0 {
            if p.checked != need || page_up((need as u64).wrapping_add(FOOTER as u64)) != Some(p.length as u64) {
                return Err(Error::BadTrailer);
            }
            if rd_u32(t, 28) != 0 || rd_u32(foot, 4) != 0 || rd_u32(foot, 24) != 0 {
                return Err(Error::ReservedNonZero);
            }
        }
        let mut prev = PAGE as u64;
        for i in 0..count {
            prev = check_entry(&trailer_entry(t, i), prev, toff as u64)?;
        }
        Ok(Self {
            data,
            trailer: toff,
            count,
            minor,
        })
    }

    /// The pack bytes.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
    /// Live records listed by the trailer.
    pub fn records(&self) -> usize {
        self.count
    }
    /// Offset of the trailer (the pack length for an empty pack).
    pub fn trailer_offset(&self) -> usize {
        self.trailer
    }
    /// Minor version of the pack header.
    pub fn minor(&self) -> u16 {
        self.minor
    }

    /// The trailer's entries, in file order.
    pub fn entries(&self) -> impl Iterator<Item = IndexEntry> + 'a {
        let t = self.data.get(self.trailer..).unwrap_or(&[]);
        (0..self.count).map(move |i| trailer_entry(t, i))
    }

    /// Copies the trailer's entries into `out`, sorted by sample id, for [`find`]. Returns the
    /// number written; [`Error::BufferTooSmall`] if `out` holds fewer than [`Pack::records`].
    pub fn index(&self, out: &mut [IndexEntry]) -> Result<usize, Error> {
        let dst = out.get_mut(..self.count).ok_or(Error::BufferTooSmall)?;
        for (d, e) in dst.iter_mut().zip(self.entries()) {
            *d = e;
        }
        sort(dst, |a, b| a.id < b.id);
        if dst.windows(2).any(|w| matches!(w, [a, b] if a.id == b.id)) {
            return Err(Error::DuplicateId);
        }
        Ok(self.count)
    }

    /// Rebuilds the live set from the entries alone (records add, tombstones remove, in file
    /// order) into `out`, sorted by sample id. Every record header is checked on the way.
    /// Returns the number of live records, or the error and the number of slots needed.
    pub fn rebuild_index(&self, out: &mut [IndexEntry]) -> Result<usize, (Error, usize)> {
        let mut n = 0usize;
        let mut it = self.scan();
        for e in &mut it {
            let item = match e {
                Ok(Entry::Record(r)) => r.entry(),
                Ok(Entry::Tombstone { offset, id }) => IndexEntry {
                    id,
                    offset: offset as u64,
                    length: 0,
                    preload: 0,
                },
                Ok(_) => continue,
                Err((_, err)) => return Err((err, 0)),
            };
            if let Some(slot) = out.get_mut(n) {
                *slot = item;
            }
            n = n.wrapping_add(1);
        }
        let all = out.get_mut(..n).ok_or((Error::BufferTooSmall, n))?;
        sort(all, |a, b| (a.id, a.offset) < (b.id, b.offset));
        let mut w = 0usize;
        let mut live: Option<IndexEntry> = None;
        for i in 0..n {
            let cur = all.get(i).copied().unwrap_or_default();
            if live.is_some_and(|l| l.id != cur.id) {
                if let (Some(slot), Some(l)) = (all.get_mut(w), live) {
                    *slot = l;
                }
                w = w.wrapping_add(1);
                live = None;
            }
            if cur.length == 0 {
                // a tombstone
                if live.is_none() {
                    return Err((Error::NotLive, n));
                }
                live = None;
            } else {
                if live.is_some() {
                    return Err((Error::DuplicateId, n));
                }
                live = Some(cur);
            }
        }
        if let (Some(slot), Some(l)) = (all.get_mut(w), live) {
            *slot = l;
            w = w.wrapping_add(1);
        }
        Ok(w)
    }

    /// The record an index entry points at, with its header checked and compared with the entry.
    pub fn record(&self, e: &IndexEntry) -> Result<Record<'a>, Error> {
        Record::at(self.data.get(..self.trailer).unwrap_or(&[]), e)
    }

    /// Walks every entry after the pack header, in file order. Stops after the first error.
    pub fn scan(&self) -> EntryIter<'a> {
        EntryIter {
            data: self.data,
            pos: PAGE,
            done: false,
        }
    }
}

/// The committed length of a pack whose tail may be torn: the largest page-aligned prefix of
/// `data` that [`Pack::open`] accepts, or one page (an empty pack) when no trailer survived.
/// A host truncates the file to this length; a read-only reader opens `&data[..length]`.
///
/// Errors are returned, and nothing may be truncated, when the pack header is invalid or when a
/// prefix ending in a well-formed footer is invalid for a reason a torn write cannot cause.
pub fn recover(data: &[u8]) -> Result<usize, Error> {
    check_header(data)?;
    let mut p = data.len().min(MAX_PACK) & !(PAGE - 1);
    while p > PAGE {
        match Pack::open(data.get(..p).unwrap_or(&[])) {
            Ok(_) => return Ok(p),
            Err(Error::NoFooter | Error::BadFooter | Error::BadChecksum | Error::BadEntry) => {}
            Err(e) => return Err(e),
        }
        p = p.wrapping_sub(PAGE);
    }
    Ok(PAGE)
}

/// Looks a sample id up in an index sorted by [`Pack::index`] or [`Pack::rebuild_index`].
pub fn find<'i>(index: &'i [IndexEntry], id: &SampleId) -> Option<&'i IndexEntry> {
    find_position(index, id).and_then(|i| index.get(i))
}

/// Position of a sample id in a sorted index.
pub fn find_position(index: &[IndexEntry], id: &SampleId) -> Option<usize> {
    let (mut lo, mut hi) = (0usize, index.len());
    while lo < hi {
        let mid = lo.wrapping_add(hi.wrapping_sub(lo) / 2);
        match index.get(mid)?.id.cmp(id) {
            core::cmp::Ordering::Less => lo = mid.wrapping_add(1),
            core::cmp::Ordering::Greater => hi = mid,
            core::cmp::Ordering::Equal => return Some(mid),
        }
    }
    None
}

/// Heap sort, in place. `core`'s sorts keep a panic path for an inconsistent comparison, which
/// the no-panic check (`tools/check-no-panic.sh`) rejects.
fn sort<T>(v: &mut [T], less: impl Fn(&T, &T) -> bool) {
    let lt = |v: &[T], a: usize, b: usize| matches!((v.get(a), v.get(b)), (Some(x), Some(y)) if less(x, y));
    // a < b
    let swap = |v: &mut [T], a: usize, b: usize| {
        if let Some([x, .., y]) = v.get_mut(a..=b) {
            core::mem::swap(x, y);
        }
    };
    let sift = |v: &mut [T], mut root: usize| loop {
        let mut c = root.wrapping_mul(2).wrapping_add(1);
        if c >= v.len() {
            break;
        }
        if lt(v, c, c.wrapping_add(1)) {
            c = c.wrapping_add(1);
        }
        if !lt(v, root, c) {
            break;
        }
        swap(v, root, c);
        root = c;
    };
    for i in (0..v.len() / 2).rev() {
        sift(v, i);
    }
    for end in (1..v.len()).rev() {
        swap(v, 0, end);
        sift(v.get_mut(..end).unwrap_or(&mut []), 0);
    }
}

/// One entry found by [`Pack::scan`].
#[derive(Clone, Copy, Debug)]
pub enum Entry<'a> {
    /// A record (header checked).
    Record(Record<'a>),
    /// A tombstone.
    Tombstone {
        /// Offset in the pack.
        offset: usize,
        /// The sample id it removes.
        id: SampleId,
    },
    /// A trailer.
    Trailer {
        /// Offset in the pack.
        offset: usize,
    },
    /// An entry type this version does not know (skipped; see "Forward compatibility").
    Unknown,
}

/// Iterator returned by [`Pack::scan`]. Yields `Err((offset, error))` once and then ends.
#[derive(Clone, Debug)]
pub struct EntryIter<'a> {
    data: &'a [u8],
    pos: usize,
    done: bool,
}

impl<'a> EntryIter<'a> {
    fn step(&mut self) -> Result<Entry<'a>, Error> {
        let pos = self.pos;
        let d = self.data.get(pos..).unwrap_or(&[]);
        let magic = magic_of(d);
        let known = [MAGIC_HEADER, MAGIC_RECORD, MAGIC_TOMBSTONE, MAGIC_TRAILER].contains(&magic);
        if known && d.len() >= PREFIX && rd_u16(d, 4) != MAJOR {
            return Err(Error::UnsupportedVersion);
        }
        let p = prefix(d)?;
        self.pos = pos.wrapping_add(p.length);
        let body = d.get(..p.length).unwrap_or(&[]);
        Ok(match magic {
            MAGIC_RECORD => Entry::Record(Record::parse(body, pos as u64)?),
            MAGIC_TOMBSTONE => {
                if p.checked < TOMBSTONE_CHECKED
                    || (p.minor == 0 && (p.checked != TOMBSTONE_CHECKED || p.length != PAGE))
                {
                    return Err(Error::BadEntry);
                }
                Entry::Tombstone {
                    offset: pos,
                    id: rd_id(body, PREFIX),
                }
            }
            MAGIC_TRAILER => {
                let foot = body.get(p.length.wrapping_sub(FOOTER)..).unwrap_or(&[]);
                let self_ref = rd_u64(foot, 8) == pos as u64 && rd_u64(foot, 16) == p.length as u64;
                if magic_of(foot) != MAGIC_FOOTER
                    || Some(crc32c(foot.get(..28).unwrap_or(&[]))) != Some(rd_u32(foot, 28))
                {
                    return Err(Error::BadChecksum);
                }
                if !self_ref {
                    return Err(Error::BadFooter);
                }
                Entry::Trailer { offset: pos }
            }
            MAGIC_HEADER => return Err(Error::BadEntry),
            _ => Entry::Unknown,
        })
    }
}

impl<'a> Iterator for EntryIter<'a> {
    type Item = Result<Entry<'a>, (usize, Error)>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pos >= self.data.len() {
            return None;
        }
        let at = self.pos;
        let r = self.step();
        if r.is_err() {
            self.done = true;
        }
        Some(r.map_err(|e| (at, e)))
    }
}

/// A record with a checked header: zero-copy access to its window, frame table and frames.
#[derive(Clone, Copy, Debug)]
pub struct Record<'a> {
    bytes: &'a [u8],
    info: RecordInfo,
}

fn is_block_size(b: u32) -> bool {
    b.is_power_of_two() && (256..=flac::MAX_BLOCK as u32).contains(&b)
}

impl<'a> Record<'a> {
    /// The record `e` points at in `data` (the pack up to its trailer).
    pub fn at(data: &'a [u8], e: &IndexEntry) -> Result<Self, Error> {
        Self::locate(data, e, true)
    }

    /// As [`Record::at`], but without the checks an earlier `at` of the same entry made over
    /// the whole head (checksum, credit, frame table): constant time, for repeated access
    /// through the C interface. Every other header field is still checked.
    pub fn reopen(data: &'a [u8], e: &IndexEntry) -> Result<Self, Error> {
        Self::locate(data, e, false)
    }

    fn locate(data: &'a [u8], e: &IndexEntry, full: bool) -> Result<Self, Error> {
        if !e.offset.is_multiple_of(PAGE as u64) || e.offset < PAGE as u64 {
            return Err(Error::BadOffset);
        }
        let start = usize_of(e.offset)?;
        let end = start.checked_add(usize_of(e.length)?).ok_or(Error::BadOffset)?;
        let bytes = data.get(start..end).ok_or(Error::BadOffset)?;
        let r = Self::check(bytes, e.offset, full)?;
        if r.info.id != e.id || r.preload() != e.preload {
            return Err(Error::Mismatch);
        }
        Ok(r)
    }

    /// Checks the header of the record that is exactly `bytes`, found at `offset` in its pack.
    pub fn parse(bytes: &'a [u8], offset: u64) -> Result<Self, Error> {
        Self::check(bytes, offset, true)
    }

    fn check(bytes: &'a [u8], offset: u64, full: bool) -> Result<Self, Error> {
        if magic_of(bytes) != MAGIC_RECORD {
            return Err(Error::NotRecord);
        }
        if rd_u16(bytes, 4) != MAJOR {
            return Err(Error::UnsupportedVersion);
        }
        let p = if full { prefix(bytes)? } else { prefix_fields(bytes)? };
        if p.length != bytes.len() {
            return Err(Error::Mismatch);
        }
        if p.checked < RECORD_FIXED {
            return Err(Error::BadEntry);
        }
        let b = bytes;
        let info = RecordInfo {
            offset,
            length: p.length as u64,
            id: rd_id(b, 32),
            md5: b
                .get(64..80)
                .and_then(|s| s.first_chunk::<16>())
                .copied()
                .unwrap_or([0; 16]),
            rate: rd_u32(b, 80),
            channels: u32::from(b.get(84).copied().unwrap_or(0)),
            bits: u32::from(b.get(85).copied().unwrap_or(0)),
            frames: rd_u32(b, 88),
            window_frames: rd_u32(b, 92),
            block_size: rd_u32(b, 96),
            blocks: rd_u32(b, 100),
            frame_bytes: rd_u32(b, 104),
            head_len: rd_u32(b, 24),
            window_bytes: rd_u32(b, 28),
            credit_offset: RECORD_FIXED as u32,
            credit_len: u32::from(rd_u16(b, 116)),
            table_offset: 0,
            min_frame: rd_u32(b, 108),
            max_frame: rd_u32(b, 112),
            minor: u32::from(p.minor),
        };
        let flags = rd_u16(b, 86);
        if p.minor == 0 && (flags != 0 || rd_u16(b, 118) != 0 || rd_u64(b, 120) != 0) {
            return Err(Error::ReservedNonZero);
        }
        if info.bits != 16
            || !(1..=2).contains(&info.channels)
            || !(1..=1_048_575).contains(&info.rate)
            || !is_block_size(info.block_size)
        {
            return Err(Error::BadFormat);
        }
        let (fr, bs, wf) = (u64::from(info.frames), u64::from(info.block_size), info.window_frames);
        if fr == 0 || u64::from(info.blocks) != fr.div_ceil(bs) {
            return Err(Error::BadLayout);
        }
        if info.credit_len as usize > MAX_CREDIT {
            return Err(Error::BadCredit);
        }
        let window_ok = wf == 0
            || (wf == info.frames && wf <= MAX_WINDOW)
            || (wf.is_multiple_of(WINDOW_STEP)
                && wf <= MAX_WINDOW
                && wf < info.frames
                && wf.is_multiple_of(info.block_size));
        if !window_ok {
            return Err(Error::BadLayout);
        }
        // All of these are far below 2^40, so the sums cannot overflow u64.
        let table_off = (RECORD_FIXED as u64).wrapping_add(u64::from(info.credit_len).next_multiple_of(4));
        let table_end = table_off.wrapping_add(u64::from(info.blocks).wrapping_add(1).wrapping_mul(4));
        let wneed = u64::from(wf).wrapping_mul(u64::from(info.channels)).wrapping_mul(2);
        let (h, w) = (u64::from(info.head_len), u64::from(info.window_bytes));
        let checked = p.checked as u64;
        if (p.minor == 0 && checked != table_end)
            || checked < table_end
            || page_up(checked) != Some(h)
            || page_up(wneed) != Some(w)
            || page_up(h.wrapping_add(w).wrapping_add(u64::from(info.frame_bytes))) != Some(p.length as u64)
        {
            return Err(Error::BadLayout);
        }
        let info = RecordInfo {
            table_offset: table_off as u32,
            ..info
        };
        if !full {
            return Ok(Self { bytes, info });
        }
        let credit = b
            .get(RECORD_FIXED..RECORD_FIXED.wrapping_add(info.credit_len as usize))
            .unwrap_or(&[]);
        if core::str::from_utf8(credit).is_err() {
            return Err(Error::BadCredit);
        }
        let table = b.get(table_off as usize..table_end as usize).unwrap_or(&[]);
        let mut prev: Option<u32> = None;
        let (mut lo, mut hi) = (u32::MAX, 0u32);
        for v in table.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)) {
            match prev {
                None if v != 0 => return Err(Error::BadFrameTable),
                Some(pv) if v <= pv => return Err(Error::BadFrameTable),
                Some(pv) => {
                    let len = v.wrapping_sub(pv);
                    lo = lo.min(len);
                    hi = hi.max(len);
                }
                None => {}
            }
            prev = Some(v);
        }
        if prev != Some(info.frame_bytes) || lo != info.min_frame || hi != info.max_frame {
            return Err(Error::BadFrameTable);
        }
        Ok(Self { bytes, info })
    }

    /// The header fields.
    pub fn info(&self) -> &RecordInfo {
        &self.info
    }
    /// The whole record, for a byte-for-byte copy.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
    /// The sample id the header states.
    pub fn id(&self) -> &SampleId {
        &self.info.id
    }
    /// Bytes from the record start to the end of the window region.
    pub fn preload(&self) -> u64 {
        u64::from(self.info.head_len).wrapping_add(u64::from(self.info.window_bytes))
    }
    /// The trailer / index entry for this record.
    pub fn entry(&self) -> IndexEntry {
        IndexEntry {
            id: self.info.id,
            offset: self.info.offset,
            length: self.info.length,
            preload: self.preload(),
        }
    }
    fn sub(&self, at: u32, len: u64) -> &'a [u8] {
        let at = at as usize;
        usize::try_from(len)
            .ok()
            .and_then(|l| self.bytes.get(at..at.checked_add(l)?))
            .unwrap_or(&[])
    }
    /// The credit string.
    pub fn credit(&self) -> &'a str {
        core::str::from_utf8(self.sub(self.info.credit_offset, u64::from(self.info.credit_len))).unwrap_or("")
    }
    /// The preload window: frames `[0, window_frames)`, 16-bit little-endian, interleaved.
    /// Starts on a 4,096-byte boundary of the pack.
    pub fn window(&self) -> &'a [u8] {
        let i = &self.info;
        self.sub(
            i.head_len,
            u64::from(i.window_frames)
                .wrapping_mul(u64::from(i.channels))
                .wrapping_mul(2),
        )
    }
    /// The FLAC frames, one standard stream.
    pub fn frames(&self) -> &'a [u8] {
        let i = &self.info;
        self.sub(i.head_len.wrapping_add(i.window_bytes), u64::from(i.frame_bytes))
    }
    /// The frame table: `blocks + 1` little-endian u32 offsets into [`Record::frames`].
    pub fn frame_table(&self) -> &'a [u8] {
        let i = &self.info;
        self.sub(i.table_offset, u64::from(i.blocks).wrapping_add(1).wrapping_mul(4))
    }
    fn table_at(&self, i: u32) -> Option<usize> {
        let t = self.frame_table();
        let at = (i as usize).checked_mul(4)?;
        t.get(at..)?.first_chunk::<4>().map(|b| u32::from_le_bytes(*b) as usize)
    }
    /// The bytes of FLAC frame `i`.
    pub fn frame(&self, i: u32) -> Option<&'a [u8]> {
        let a = self.table_at(i)?;
        let b = self.table_at(i.checked_add(1)?)?;
        self.frames().get(a..b)
    }

    /// Decodes blocks `[first, first + count)` into `out0` (channel 0) and `out1` (channel 1 of a
    /// stereo record), contiguously. Returns the frames written. `flags` are the decoder's
    /// ([`flac::FLAG_CHECK_CRC`]); play-time callers pass 0. On an error the outputs may hold the
    /// blocks decoded before it.
    pub fn decode(
        &self,
        first: u32,
        count: u32,
        flags: u32,
        scratch: &mut Scratch,
        out0: &mut [i16],
        out1: &mut [i16],
    ) -> Result<usize, Error> {
        let end = first.checked_add(count).ok_or(Error::InvalidArgument)?;
        if end > self.info.blocks {
            return Err(Error::InvalidArgument);
        }
        let total = (first..end).fold(0u64, |a, b| a.wrapping_add(u64::from(self.info.block_frames(b))));
        let total = usize::try_from(total).map_err(|_| Error::BufferTooSmall)?;
        let stereo = self.info.channels == 2;
        if out0.len() < total || (stereo && out1.len() < total) {
            return Err(Error::BufferTooSmall);
        }
        let mut pos = 0usize;
        for b in first..end {
            let frame = self.frame(b).ok_or(Error::BadFrameTable)?;
            let o0 = out0.get_mut(pos..).unwrap_or(&mut []);
            let o1 = if stereo {
                out1.get_mut(pos..).unwrap_or(&mut [])
            } else {
                &mut []
            };
            let fi = flac::decode_frame(frame, flags, scratch, o0, o1)?;
            let want = self.info.block_frames(b);
            if fi.samples != want || fi.channels != self.info.channels {
                return Err(Error::FrameMismatch);
            }
            pos = pos.wrapping_add(want as usize);
        }
        Ok(pos)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    #[test]
    fn sort_orders_every_length() {
        let mut seed = 1u32;
        for n in 0..70 {
            let mut v = [0u32; 70];
            for x in v.iter_mut().take(n) {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                *x = seed >> 26; // few distinct values: many ties
            }
            let (sum, xor) = v.iter().fold((0u32, 0u32), |(s, x), &e| (s.wrapping_add(e), x ^ e));
            super::sort(&mut v[..n], |a, b| a < b);
            assert!(v[..n].windows(2).all(|w| w[0] <= w[1]), "length {n}");
            assert_eq!(
                v.iter().fold((0u32, 0u32), |(s, x), &e| (s.wrapping_add(e), x ^ e)),
                (sum, xor)
            );
        }
    }
}
