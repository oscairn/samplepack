// SPDX-License-Identifier: MIT
//! Pack container: round trips, every rejection reason, torn-tail recovery, copy semantics and
//! sample ids. Packs are made from corpus records (synthetic signals) with the writer functions.

use samplepack::flac::{self, Scratch};
use samplepack::hash::crc32c;
use samplepack::id::{sample_id, IdHasher};
use samplepack::pack::{
    self, find, recover, verify_pack, verify_record, Entry, Error, IndexEntry, Pack, Record, RecordParams,
    VerifyScratch, PAGE,
};
use samplepack_tests::{append, build, record, stream, Item};

const ST: &str = "sweep-st-b1024-l4-r44100";
const MONO: &str = "sweep-mono-b1024-l4-r44100";
const NOISE: &str = "noise-st-b1024-l4-r44100";

fn vs() -> Box<VerifyScratch> {
    Box::new(VerifyScratch::new())
}

fn rd32(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}
fn wr32(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn wr64(d: &mut [u8], at: usize, v: u64) {
    d[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// Recomputes the checksum of the entry starting at `at`.
fn reseal(d: &mut [u8], at: usize) {
    let checked = rd32(d, at + 16) as usize;
    let mut body = d[at..at + checked].to_vec();
    body[20..24].fill(0);
    wr32(d, at + 20, crc32c(&body));
}
/// Recomputes the footer checksum (last 32 bytes).
fn reseal_footer(d: &mut [u8]) {
    let f = d.len() - 32;
    let c = crc32c(&d[f..f + 28]);
    wr32(d, f + 28, c);
}
fn trailer_at(d: &[u8]) -> usize {
    let f = d.len() - 32;
    u64::from_le_bytes(d[f + 8..f + 16].try_into().unwrap()) as usize
}

fn index_of(p: &Pack) -> Vec<IndexEntry> {
    let mut v = vec![IndexEntry::default(); p.records()];
    p.index(&mut v).unwrap();
    v
}

#[test]
fn round_trip_decode_and_ids() {
    let (a, b, c) = (
        record(ST, 4096, "credit: synthetic"),
        record(MONO, 0, ""),
        record(NOISE, 16384, ""),
    );
    let pk = build(&[&a, &b, &c]);
    let p = Pack::open(&pk).unwrap();
    assert_eq!(p.records(), 3);
    let idx = index_of(&p);
    let mut vsb = vs();
    assert_eq!(verify_pack(&p, &mut vsb, &mut [IndexEntry::default(); 8]), Ok(3));
    let mut scratch = Box::new(Scratch::new());
    for name in [ST, MONO, NOISE] {
        let s = stream(name);
        let id = sample_id(s.rate, s.channels, &s.pcm);
        let e = find(&idx, &id).expect("id of the PCM finds the record");
        let r = p.record(e).unwrap();
        assert_eq!(r.info().frames, 5000);
        assert_eq!(r.info().offset % PAGE as u64, 0);
        assert_eq!(
            (r.info().offset as usize + r.info().head_len as usize) % PAGE,
            0,
            "window page-aligned"
        );
        // MD5 equals the stock encoder's STREAMINFO MD5 of the same audio
        let file = std::fs::read(samplepack_tests::data_dir().join(format!("{name}.flac"))).unwrap();
        assert_eq!(r.info().md5, file[26..42]);
        let ch = s.channels as usize;
        let (mut l, mut rr) = (vec![0i16; 5000], vec![0i16; 5000]);
        assert_eq!(r.decode(0, r.info().blocks, 0, &mut scratch, &mut l, &mut rr), Ok(5000));
        for i in 0..5000 {
            assert_eq!(l[i], s.pcm[i * ch]);
            if ch == 2 {
                assert_eq!(rr[i], s.pcm[i * 2 + 1]);
            }
        }
        // window = first window_frames of the PCM
        let w: Vec<i16> = r.window().chunks(2).map(|x| i16::from_le_bytes([x[0], x[1]])).collect();
        assert_eq!(w.len(), r.info().window_frames as usize * ch);
        assert_eq!(&w[..], &s.pcm[..w.len()]);
        // block range decode
        let n = r
            .decode(2, 3, flac::FLAG_CHECK_CRC, &mut scratch, &mut l, &mut rr)
            .unwrap();
        assert_eq!(n, 5000 - 2048);
        assert_eq!(l[0], s.pcm[2048 * ch]);
        assert_eq!(pack::record_sample_id(&r, &mut vsb), Ok(id));
    }
    assert_eq!(p.record(&idx[0]).unwrap().info().window_frames % 1024, 0);
    let r = Record::parse(&a, 0).unwrap();
    assert_eq!(r.credit(), "credit: synthetic");
    assert_eq!(r.info().window_frames, 4096);
    assert_eq!(
        Record::parse(&c, 0).unwrap().info().window_frames,
        5000,
        "window holds the whole short sample"
    );
}

#[test]
fn sample_id_rules() {
    let s = stream(ST);
    let id = sample_id(44100, 2, &s.pcm);
    assert_ne!(id, sample_id(48000, 2, &s.pcm), "rate is part of the id");
    assert_ne!(id, sample_id(44100, 1, &s.pcm), "channel count is part of the id");
    // the canonical byte layout: tag, rate, channels, bits, PCM little-endian
    let mut bytes = b"oscairn-sample-1\0".to_vec();
    bytes.extend(44100u32.to_le_bytes());
    bytes.extend([2, 16]);
    for v in &s.pcm {
        bytes.extend(v.to_le_bytes());
    }
    assert_eq!(id, samplepack::hash::Sha256::digest(&bytes));
    let mut h = IdHasher::new(44100, 2, 16);
    for chunk in s.pcm.chunks(333) {
        h.update(chunk);
    }
    assert_eq!(h.finish(), id);
    // the record of differently encoded FLAC of the same audio has the same id
    let a = Record::parse(&record(ST, 4096, ""), 0).unwrap().info().id;
    let b = Record::parse(&record("sweep-st-b1024-l8-nomidside-r44100", 4096, ""), 0)
        .unwrap()
        .info()
        .id;
    let pcm_b = stream("sweep-st-b1024-l8-nomidside-r44100").pcm;
    assert_eq!(a, id);
    assert_eq!(b == a, pcm_b == s.pcm);
}

#[test]
fn copy_is_byte_for_byte() {
    let (a, b) = (record(ST, 4096, "x"), record(MONO, 2048, ""));
    let p1 = build(&[&a, &b]);
    let p2 = build(&[&b]);
    let src = Pack::open(&p1).unwrap();
    let e = src.entries().next().unwrap();
    let bytes = src.record(&e).unwrap().bytes();
    let mut p3 = p2.clone();
    append(&mut p3, &[Item::Rec(bytes)]);
    let dst = Pack::open(&p3).unwrap();
    let idx = index_of(&dst);
    let r = dst.record(find(&idx, &e.id).unwrap()).unwrap();
    assert_ne!(r.info().offset, e.offset);
    assert_eq!(r.bytes(), bytes);
    assert_eq!(verify_pack(&dst, &mut vs(), &mut [IndexEntry::default(); 4]), Ok(2));
}

#[test]
fn tombstones_and_rebuild() {
    let (a, b) = (record(ST, 4096, ""), record(MONO, 4096, ""));
    let ida = Record::parse(&a, 0).unwrap().info().id;
    let mut pk = build(&[&a, &b]);
    append(&mut pk, &[Item::Tomb(ida)]);
    let p = Pack::open(&pk).unwrap();
    assert_eq!(p.records(), 1);
    let mut idx = vec![IndexEntry::default(); 3];
    assert_eq!(p.rebuild_index(&mut idx), Ok(1));
    assert_eq!(idx[0], p.entries().next().unwrap());
    assert_eq!(p.rebuild_index(&mut idx[..2]), Err((Error::BufferTooSmall, 3)));
    // re-adding the removed sample after its tombstone is valid
    append(&mut pk, &[Item::Rec(&a)]);
    let p = Pack::open(&pk).unwrap();
    assert_eq!(verify_pack(&p, &mut vs(), &mut [IndexEntry::default(); 4]), Ok(2));
    let kinds: Vec<char> = p
        .scan()
        .map(|e| match e.unwrap() {
            Entry::Record(_) => 'R',
            Entry::Tombstone { .. } => 'T',
            Entry::Trailer { .. } => 'I',
            Entry::Unknown => '?',
        })
        .collect();
    assert_eq!(kinds.iter().collect::<String>(), "RRITIRI");
}

#[test]
fn unknown_entries_and_newer_minor_are_skipped() {
    let a = record(ST, 4096, "");
    let mut pk = build(&[&a]);
    // an entry of a type this version does not know, inserted before the trailer
    let t = trailer_at(&pk);
    let mut x = vec![0u8; PAGE];
    x[..4].copy_from_slice(b"OSPX");
    x[4] = 1;
    x[6] = 3;
    wr64(&mut x, 8, PAGE as u64);
    wr32(&mut x, 16, 40);
    x[30] = 0xAB;
    reseal(&mut x, 0);
    let mut q: Vec<u8> = pk[..t].to_vec();
    q.extend(&x);
    let p0 = Pack::open(&pk).unwrap();
    let entries: Vec<IndexEntry> = p0.entries().collect();
    let mut tr = vec![0u8; pack::trailer_len(1).unwrap()];
    pack::write_trailer(&entries, q.len() as u64, &mut tr).unwrap();
    q.extend(tr);
    let p = Pack::open(&q).unwrap();
    assert!(p.scan().any(|e| matches!(e, Ok(Entry::Unknown))));
    assert_eq!(verify_pack(&p, &mut vs(), &mut [IndexEntry::default(); 2]), Ok(1));
    // a record of a newer minor version may use its reserved fields
    let r0 = 4096;
    pk[r0 + 6] = 1;
    pk[r0 + 118] = 7;
    reseal(&mut pk, r0);
    let p = Pack::open(&pk).unwrap();
    let e = p.entries().next().unwrap();
    assert_eq!(p.record(&e).unwrap().info().minor, 1);
    // the same at minor 0 is rejected
    pk[r0 + 6] = 0;
    reseal(&mut pk, r0);
    assert_eq!(Pack::open(&pk).unwrap().record(&e).unwrap_err(), Error::ReservedNonZero);
}

/// The append is truncated at every byte position; recovery always lands on the last commit.
#[test]
fn torn_tail_recovers_at_every_byte() {
    let (a, b, c) = (record(ST, 4096, ""), record(MONO, 1024, ""), record(NOISE, 0, ""));
    let ida = Record::parse(&a, 0).unwrap().info().id;
    let mut pk = build(&[&a]);
    let s1 = pk.len();
    let live1: Vec<IndexEntry> = Pack::open(&pk).unwrap().entries().collect();
    append(&mut pk, &[Item::Tomb(ida), Item::Rec(&b), Item::Rec(&c)]);
    let s2 = pk.len();
    for cut in PAGE..s2 {
        let d = &pk[..cut];
        let want = if cut < s1 { PAGE } else { s1 };
        assert_eq!(recover(d), Ok(want), "cut at {cut}");
        if cut % PAGE != 0 || (cut != s1 && cut != PAGE) {
            assert!(Pack::open(d).is_err(), "open must refuse a torn pack (cut {cut})");
        }
        let p = Pack::open(&d[..want]).unwrap();
        if want == s1 {
            assert_eq!(p.entries().collect::<Vec<_>>(), live1);
        } else {
            assert_eq!(p.records(), 0);
        }
    }
    assert_eq!(recover(&pk), Ok(s2));
    assert_eq!(Pack::open(&pk).unwrap().records(), 2);
    // garbage after the commit (an append whose bytes were never written properly)
    let mut g = pk[..s1].to_vec();
    g.extend((0..3 * PAGE).map(|i| (i * 31 % 251) as u8));
    assert_eq!(Pack::open(&g).unwrap_err(), Error::NoFooter);
    assert_eq!(recover(&g), Ok(s1));
    // shorter than a page: not a pack
    assert_eq!(recover(&pk[..100]), Err(Error::NotPack));
}

#[test]
fn every_rejection_reason() {
    let a = record(ST, 4096, "credit");
    let b = record(MONO, 0, "");
    let good = build(&[&a, &b]);
    let p = Pack::open(&good).unwrap();
    let e0 = p.entries().next().unwrap();
    let e1 = p.entries().nth(1).unwrap();
    let r0 = e0.offset as usize;
    let r1 = e1.offset as usize;
    let t = trailer_at(&good);
    let open = |d: &[u8]| Pack::open(d).map(|_| ());
    let rec0 = |d: &[u8]| Pack::open(d).and_then(|p| p.record(&e0).map(|_| ()));
    let rec1 = |d: &[u8]| Pack::open(d).and_then(|p| p.record(&e1).map(|_| ()));
    let verify1 = |d: &[u8]| {
        let p = Pack::open(d)?;
        verify_record(&p.record(&e1)?, &mut vs())
    };
    let verify0 = |d: &[u8]| {
        let p = Pack::open(d)?;
        verify_record(&p.record(&e0)?, &mut vs())
    };
    let edit = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut d = good.clone();
        f(&mut d);
        d
    };

    // pack header
    assert_eq!(open(&good[..100]), Err(Error::NotPack));
    assert_eq!(open(&edit(&|d| d[0] = b'X')), Err(Error::NotPack));
    assert_eq!(open(&edit(&|d| d[4] = 2)), Err(Error::UnsupportedVersion));
    assert_eq!(open(&edit(&|d| d[7] = 1)), Err(Error::BadChecksum));
    assert_eq!(
        verify_pack(
            &Pack::open(&edit(&|d| d[25] = 1)).unwrap(),
            &mut vs(),
            &mut [IndexEntry::default(); 4]
        ),
        Err(Error::Padding)
    );
    assert_eq!(
        open(&edit(&|d| {
            wr64(d, 8, 8192);
            reseal(d, 0)
        })),
        Err(Error::BadEntry)
    );
    // footer and trailer
    assert_eq!(open(&good[..good.len() - 1]), Err(Error::NoFooter));
    assert_eq!(open(&edit(&|d| *d.last_mut().unwrap() ^= 1)), Err(Error::NoFooter));
    let huge = {
        let mut v = vec![0u8; pack::MAX_PACK + PAGE];
        v[..PAGE].copy_from_slice(&good[..PAGE]);
        v
    };
    assert_eq!(open(&huge), Err(Error::TooLarge));
    drop(huge);
    let f = good.len() - 32;
    assert_eq!(
        open(&edit(&|d| {
            wr64(d, f + 8, t as u64 - PAGE as u64);
            reseal_footer(d)
        })),
        Err(Error::BadFooter)
    );
    assert_eq!(
        open(&edit(&|d| {
            d[f + 4] = 1;
            reseal_footer(d)
        })),
        Err(Error::ReservedNonZero)
    );
    // a footer whose trailer length disagrees with the trailer entry's: a page after the trailer
    // ending in a footer that claims the trailer runs to the end of the file
    let mut long = good.clone();
    let mut foot = good[f..].to_vec();
    wr64(&mut foot, 16, (good.len() - t + PAGE) as u64);
    long.resize(good.len() + PAGE - 32, 0);
    long.extend(foot);
    reseal_footer(&mut long);
    assert_eq!(open(&long), Err(Error::BadFooter));
    assert_eq!(recover(&long), Ok(good.len()));
    assert_eq!(open(&edit(&|d| d[t + 40] ^= 1)), Err(Error::BadChecksum));
    assert_eq!(open(&edit(&|d| d[t + 4] = 2)), Err(Error::UnsupportedVersion));
    assert_eq!(
        open(&edit(&|d| {
            wr32(d, t + 24, 3);
            reseal(d, t)
        })),
        Err(Error::BadTrailer)
    );
    assert_eq!(
        open(&edit(&|d| {
            wr32(d, t + 28, 1);
            reseal(d, t)
        })),
        Err(Error::ReservedNonZero)
    );
    let ent = |i: usize| t + 32 + 56 * i;
    assert_eq!(
        open(&edit(&|d| {
            wr64(d, ent(0) + 32, e0.offset + 1);
            reseal(d, t)
        })),
        Err(Error::BadOffset)
    );
    assert_eq!(
        open(&edit(&|d| {
            wr64(d, ent(1) + 40, (t - r1 + PAGE) as u64);
            reseal(d, t)
        })),
        Err(Error::BadOffset)
    );
    assert_eq!(
        open(&edit(&|d| {
            let (x, y) = (d[ent(0)..ent(1)].to_vec(), d[ent(1)..ent(2)].to_vec());
            d[ent(0)..ent(1)].copy_from_slice(&y);
            d[ent(1)..ent(2)].copy_from_slice(&x);
            reseal(d, t)
        })),
        Err(Error::Overlap)
    );
    // duplicate ids in a trailer
    let dup = build(&[&a, &a]);
    let pd = Pack::open(&dup).unwrap();
    assert_eq!(pd.index(&mut [IndexEntry::default(); 2]), Err(Error::DuplicateId));
    assert_eq!(
        pd.rebuild_index(&mut [IndexEntry::default(); 2]),
        Err((Error::DuplicateId, 2))
    );
    assert_eq!(pd.index(&mut [IndexEntry::default(); 1]), Err(Error::BufferTooSmall));
    // tombstone for something not live
    let mut nl = build(&[&a]);
    append(&mut nl, &[Item::Tomb([7; 32])]);
    let pn = Pack::open(&nl).unwrap();
    assert_eq!(
        pn.rebuild_index(&mut [IndexEntry::default(); 2]),
        Err((Error::NotLive, 2))
    );
    // padding after the checked bytes of a tombstone and of a trailer
    let ts = nl.len() - 2 * PAGE;
    for at in [ts + 100, nl.len() - 100] {
        let mut d = nl.clone();
        d[at] = 1;
        assert_eq!(
            verify_pack(&Pack::open(&d).unwrap(), &mut vs(), &mut [IndexEntry::default(); 2]),
            Err(Error::Padding)
        );
    }
    // malformed tombstone
    let mut bt = nl.clone();
    wr32(&mut bt, ts + 16, 60);
    reseal(&mut bt, ts);
    let pb = Pack::open(&bt).unwrap();
    assert_eq!(
        pb.rebuild_index(&mut [IndexEntry::default(); 2]),
        Err((Error::BadEntry, 0))
    );
    // entry pointing at the tombstone: not a record
    let at_t = IndexEntry {
        offset: ts as u64,
        length: PAGE as u64,
        ..e0
    };
    assert_eq!(pn.record(&at_t).unwrap_err(), Error::NotRecord);
    // record headers
    assert_eq!(rec0(&edit(&|d| d[r0 + 40] ^= 1)), Err(Error::BadChecksum));
    assert_eq!(rec0(&edit(&|d| d[r0 + 4] = 2)), Err(Error::UnsupportedVersion));
    let field = |at: usize, v: u32| {
        edit(&move |d| {
            wr32(d, r0 + at, v);
            reseal(d, r0)
        })
    };
    let byte = |at: usize, v: u8| {
        edit(&move |d| {
            d[r0 + at] = v;
            reseal(d, r0)
        })
    };
    assert_eq!(
        rec0(&field(16, 100)),
        Err(Error::BadEntry),
        "checked shorter than the fixed header"
    );
    assert_eq!(rec0(&byte(85, 24)), Err(Error::BadFormat), "24-bit");
    assert_eq!(rec0(&byte(84, 3)), Err(Error::BadFormat), "3 channels");
    assert_eq!(rec0(&field(80, 0)), Err(Error::BadFormat), "rate 0");
    assert_eq!(rec0(&field(96, 1000)), Err(Error::BadFormat), "block size");
    assert_eq!(rec0(&byte(86, 1)), Err(Error::ReservedNonZero), "flags");
    assert_eq!(rec0(&byte(120, 1)), Err(Error::ReservedNonZero));
    assert_eq!(rec0(&field(24, 8192)), Err(Error::BadLayout), "head length");
    assert_eq!(
        rec0(&field(92, 3000)),
        Err(Error::BadLayout),
        "window not a multiple of 1024"
    );
    assert_eq!(rec0(&field(100, 6)), Err(Error::BadLayout), "block count");
    assert_eq!(rec0(&field(88, 0)), Err(Error::BadLayout), "no frames");
    let tab = r0 + 128 + 8; // after "credit" (6 bytes, padded to 8)
    assert_eq!(
        rec0(&edit(&|d| {
            let v = rd32(d, tab + 8);
            wr32(d, tab + 4, v);
            reseal(d, r0)
        })),
        Err(Error::BadFrameTable)
    );
    assert_eq!(rec0(&field(108, 1)), Err(Error::BadFrameTable), "smallest frame");
    assert_eq!(rec0(&byte(128, 0xFF)), Err(Error::BadCredit), "not UTF-8");
    assert_eq!(rec0(&byte(117, 0x10)), Err(Error::BadCredit), "credit too long");
    let wrong_id = IndexEntry { id: [9; 32], ..e0 };
    assert_eq!(p.record(&wrong_id).unwrap_err(), Error::Mismatch);
    let wrong_len = IndexEntry {
        length: e0.length + PAGE as u64,
        ..e0
    };
    assert_eq!(p.record(&wrong_len).unwrap_err(), Error::Mismatch);
    assert_eq!(rec1(&good), Ok(()));
    // full verification
    assert_eq!(verify0(&good), Ok(()));
    assert_eq!(
        verify0(&edit(&|d| {
            d[r0 + 128 + 7] = 1; // between the 6-byte credit and the frame table
            reseal(d, r0)
        })),
        Err(Error::Padding)
    );
    let tail = r0 + e0.length as usize - 1;
    assert_eq!(verify0(&edit(&|d| d[tail] = 1)), Err(Error::Padding));
    let win = r0 + rd32(&good, r0 + 24) as usize;
    assert_eq!(verify0(&edit(&|d| d[win + 10] ^= 1)), Err(Error::WindowMismatch));
    let mut wp = record(NOISE, 16384, ""); // 5000 stereo frames: 20000 window bytes padded to 20480
    let wstart = rd32(&wp, 24) as usize;
    wp[wstart + 20000] = 1;
    assert_eq!(
        verify_record(&Record::parse(&wp, 0).unwrap(), &mut vs()),
        Err(Error::Padding),
        "window padding"
    );
    assert_eq!(
        verify0(&edit(&|d| {
            d[r0 + 64] ^= 1;
            reseal(d, r0)
        })),
        Err(Error::Md5Mismatch)
    );
    let mut idm = good[r0..r0 + e0.length as usize].to_vec();
    idm[32] ^= 1;
    reseal(&mut idm, 0);
    assert_eq!(
        verify_record(&Record::parse(&idm, 0).unwrap(), &mut vs()),
        Err(Error::IdMismatch)
    );
    let frames0 = win + rd32(&good, r0 + 28) as usize;
    assert_eq!(
        verify0(&edit(&|d| d[frames0 + 30] ^= 1)),
        Err(Error::Frame(flac::Error::FrameCrc))
    );
    // a mono record (no window) claiming two channels decodes mono frames: frame mismatch
    assert_eq!(
        verify1(&edit(&|d| {
            d[r1 + 84] = 2;
            reseal(d, r1)
        })),
        Err(Error::FrameMismatch)
    );
    // trailer that leaves out a live record
    let mut tm = good[..t].to_vec();
    let mut tr = vec![0u8; pack::trailer_len(1).unwrap()];
    pack::write_trailer(&[e0], t as u64, &mut tr).unwrap();
    tm.extend(tr);
    let pt = Pack::open(&tm).unwrap();
    assert_eq!(
        verify_pack(&pt, &mut vs(), &mut [IndexEntry::default(); 4]),
        Err(Error::TrailerMismatch)
    );
    // decode arguments
    let r = p.record(&e0).unwrap();
    let mut s = Box::new(Scratch::new());
    let (mut l, mut rr) = (vec![0i16; 5000], vec![0i16; 5000]);
    assert_eq!(r.decode(4, 2, 0, &mut s, &mut l, &mut rr), Err(Error::InvalidArgument));
    assert_eq!(
        r.decode(0, 2, 0, &mut s, &mut l[..2047], &mut rr),
        Err(Error::BufferTooSmall)
    );
    // codes are distinct
    let mut codes: Vec<i32> = [
        Error::NotPack,
        Error::UnsupportedVersion,
        Error::NoFooter,
        Error::TooLarge,
        Error::BadChecksum,
        Error::BadEntry,
        Error::BadFooter,
        Error::BadTrailer,
        Error::BadOffset,
        Error::Overlap,
        Error::ReservedNonZero,
        Error::BadFormat,
        Error::BadLayout,
        Error::BadFrameTable,
        Error::BadCredit,
        Error::DuplicateId,
        Error::NotLive,
        Error::Mismatch,
        Error::NotFound,
        Error::FrameMismatch,
        Error::Padding,
        Error::WindowMismatch,
        Error::Md5Mismatch,
        Error::IdMismatch,
        Error::TrailerMismatch,
        Error::NotRecord,
    ]
    .iter()
    .map(|e| e.code())
    .collect();
    codes.sort();
    codes.dedup();
    assert_eq!(codes.len(), 26);
}

#[test]
fn writer_rejects_bad_input() {
    let s = stream(MONO);
    let p = RecordParams {
        rate: s.rate,
        channels: s.channels,
        frames: s.frames,
        block_size: s.block,
        window: 4096,
        credit: "",
        table: &s.table,
        frame_data: &s.data,
    };
    let mut out = vec![0u8; 4 * pack::record_len(&p).unwrap()];
    let mut v = vs();
    assert!(pack::write_record(&p, &mut v, &mut out).is_ok());
    assert_eq!(
        pack::write_record(&RecordParams { window: 1000, ..p }, &mut v, &mut out),
        Err(Error::InvalidArgument)
    );
    assert_eq!(
        pack::write_record(&RecordParams { frames: 4000, ..p }, &mut v, &mut out),
        Err(Error::InvalidArgument)
    );
    let long = "x".repeat(1025);
    assert_eq!(
        pack::write_record(&RecordParams { credit: &long, ..p }, &mut v, &mut out),
        Err(Error::InvalidArgument)
    );
    assert_eq!(
        pack::write_record(&RecordParams { channels: 2, ..p }, &mut v, &mut out),
        Err(Error::FrameMismatch)
    );
    assert_eq!(
        pack::write_record(&RecordParams { rate: 48000, ..p }, &mut v, &mut out),
        Err(Error::FrameMismatch)
    );
    assert_eq!(
        pack::write_record(&p, &mut v, &mut out[..100]),
        Err(Error::BufferTooSmall)
    );
    let e = IndexEntry {
        id: [0; 32],
        offset: 4096,
        length: 8192,
        preload: 4096,
    };
    let mut t = vec![0u8; 8192];
    assert_eq!(pack::write_trailer(&[e, e], 20480, &mut t), Err(Error::Overlap));
    assert_eq!(pack::write_trailer(&[e], 8192, &mut t), Err(Error::BadOffset));
    assert_eq!(pack::write_trailer(&[e], 12288, &mut t), Ok(4096));
}
