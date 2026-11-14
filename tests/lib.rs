// SPDX-License-Identifier: MIT
//! Helpers shared by the tests: records made from the corpus files (stock-encoder FLAC of
//! synthetic signals) and packs assembled from them with the library's writer functions.

use samplepack::flac::{self, FrameEntry, Scratch};
use samplepack::pack::{self, IndexEntry, Record, RecordParams, VerifyScratch, PAGE};
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

/// Rate, channels, frames, block size, frame table and frames of a corpus file.
pub struct Stream {
    pub rate: u32,
    pub channels: u8,
    pub frames: u32,
    pub block: u32,
    pub table: Vec<u32>,
    pub data: Vec<u8>,
    pub pcm: Vec<i16>,
}

pub fn stream(name: &str) -> Stream {
    let file = std::fs::read(data_dir().join(format!("{name}.flac"))).unwrap();
    let pcm: Vec<i16> = std::fs::read(data_dir().join(format!("{name}.pcm")))
        .unwrap()
        .chunks(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let si = &file[8..42];
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
    let channels = ((si[12] >> 1) & 7) + 1;
    let block = u32::from(u16::from_be_bytes([si[2], si[3]]));
    let first = flac::first_frame(&file).unwrap();
    let data = file[first..].to_vec();
    let mut s = Box::new(Scratch::new());
    let mut t = vec![FrameEntry::default(); 4096];
    let r = flac::scan(&data, 0, &mut s, &mut t);
    assert_eq!(r.status, Ok(()));
    let mut table: Vec<u32> = t[..r.frames].iter().map(|e| e.offset as u32).collect();
    table.push(r.consumed as u32);
    Stream {
        rate,
        channels,
        frames: r.total_samples as u32,
        block,
        table,
        data,
        pcm,
    }
}

/// A record of the corpus file `name`.
pub fn record(name: &str, window: u32, credit: &str) -> Vec<u8> {
    let s = stream(name);
    let p = RecordParams {
        rate: s.rate,
        channels: s.channels,
        frames: s.frames,
        block_size: s.block,
        window,
        credit,
        table: &s.table,
        frame_data: &s.data,
    };
    let mut out = vec![0u8; pack::record_len(&p).unwrap()];
    let mut vs = Box::new(VerifyScratch::new());
    pack::write_record(&p, &mut vs, &mut out).unwrap();
    out
}

/// What goes into a pack: a record's bytes, or a tombstone for a sample id.
pub enum Item<'a> {
    Rec(&'a [u8]),
    Tomb([u8; 32]),
}

/// Appends items and one trailer to `pack` (which must be committed), as the append protocol does.
pub fn append(pack: &mut Vec<u8>, items: &[Item]) {
    let mut live: Vec<IndexEntry> = if pack.is_empty() {
        let mut h = vec![0u8; PAGE];
        pack::write_pack_header(&mut h).unwrap();
        pack.extend(h);
        vec![]
    } else {
        pack::Pack::open(pack).unwrap().entries().collect()
    };
    for it in items {
        let off = pack.len() as u64;
        match it {
            Item::Rec(b) => {
                live.push(Record::parse(b, off).unwrap().entry());
                pack.extend_from_slice(b);
            }
            Item::Tomb(id) => {
                live.retain(|e| &e.id != id);
                let mut t = vec![0u8; PAGE];
                pack::write_tombstone(id, &mut t).unwrap();
                pack.extend(t);
            }
        }
    }
    live.sort_by_key(|e| e.offset);
    let mut t = vec![0u8; pack::trailer_len(live.len()).unwrap()];
    let off = pack.len() as u64;
    pack::write_trailer(&live, off, &mut t).unwrap();
    pack.extend(t);
}

/// A new pack holding `records`.
pub fn build(records: &[&[u8]]) -> Vec<u8> {
    let mut p = Vec::new();
    let items: Vec<Item> = records.iter().map(|r| Item::Rec(r)).collect();
    append(&mut p, &items);
    p
}
