// SPDX-License-Identifier: MIT
//! Waveform files (`docs/waveform.md`): the worked example (bytes and id from
//! `tools/waveform-example.py`, an independent computation), canonical form, every refusal,
//! every single-byte corruption and every truncation.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use samplepack::hash::crc32c;
use samplepack::id::sample_id;
use samplepack::waveform::{build, file_len, Error, KeyBank, Waveform, MAX_BYTES};

const EXAMPLE: &str = "\
4f50574601000000cc000000af010c8b020003001e272d15841dbddadd5503de\
c6a3ac43020b40c9c0a730cc5cd2f66101301338d9fca2910054e1d97a59a316\
ed2202be78ac726618907e72c581266ed13d4891003b017f3000000000000000\
0000000000000000f0d8000000000000000000000000000000000000003b017f\
3000000001000000000000000000000010270000000000000000000000000000\
000000003c7f017f48000100010000001efbffffa8fdffff0000000000000000\
0000000064000000d0070000";
const EXAMPLE_ID: &str = "0ab2c142fc10f5f50f76c044e10942059087d87d02ff9acb6fa2edb298529e3f";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The example's sample ids and key banks, in the order the example gives them.
fn given() -> ([[u8; 32]; 2], [KeyBank; 3]) {
    let pcm = [0, 1, -1, 32767];
    let ids = [sample_id(44100, 1, &pcm), sample_id(48000, 1, &pcm)];
    let full = KeyBank {
        key_hi: 59,
        vel_lo: 1,
        vel_hi: 127,
        root: 48,
        ..KeyBank::default()
    };
    let banks = [
        KeyBank {
            key_lo: 60,
            key_hi: 127,
            root: 72,
            loop_mode: 1,
            sample: 1,
            tune: -1250,
            level: -600,
            loop_start: 100,
            loop_end: 2000,
            ..full
        },
        KeyBank { pan: -10000, ..full },
        KeyBank {
            sample: 1,
            pan: 10000,
            ..full
        },
    ];
    (ids, banks)
}

fn example() -> Vec<u8> {
    (0..EXAMPLE.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&EXAMPLE[i..i + 2], 16).unwrap())
        .collect()
}

fn built(banks: &[KeyBank], ids: &[[u8; 32]]) -> Result<Vec<u8>, Error> {
    let mut out = vec![0xAA; 20 + 72 * banks.len()];
    let n = build(banks, ids, &mut out)?;
    out.truncate(n);
    Ok(out)
}

#[test]
fn worked_example() {
    let (ids, banks) = given();
    let d = built(&banks, &ids).unwrap();
    assert_eq!(d, example());
    let w = Waveform::parse(&d).unwrap();
    assert_eq!(hex(&w.id()), EXAMPLE_ID);
    assert_eq!(w.samples(), &[ids[0], ids[1]]);
    assert_eq!(w.banks(), 3);
    assert_eq!(w.bank(2), Some(banks[0]));
}

#[test]
fn round_trip() {
    let d = example();
    let w = Waveform::parse(&d).unwrap();
    let banks: Vec<KeyBank> = (0..w.banks()).map(|i| w.bank(i).unwrap()).collect();
    assert_eq!(built(&banks, w.samples()).unwrap(), d);
    assert_eq!(w.bank(3), None);
}

#[test]
fn canonical_order_does_not_depend_on_input_order() {
    let (ids, b) = given();
    for p in [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]] {
        assert_eq!(built(&[b[p[0]], b[p[1]], b[p[2]]], &ids).unwrap(), example());
    }
    // the caller's id list in another order, with a duplicate and an unused id
    let other = [[7u8; 32], ids[1], ids[0], ids[1]];
    let moved = [
        KeyBank { sample: 3, ..b[0] },
        KeyBank { sample: 2, ..b[1] },
        KeyBank { sample: 1, ..b[2] },
    ];
    assert_eq!(built(&moved, &other).unwrap(), example());
}

#[test]
fn largest_file() {
    let ids: Vec<[u8; 32]> = (0..1024u32).map(|i| sample_id(i + 1, 1, &[0])).collect();
    let banks: Vec<KeyBank> = (0..1024u16)
        .map(|i| KeyBank {
            vel_lo: 1,
            vel_hi: 127,
            sample: i,
            ..KeyBank::default()
        })
        .collect();
    let d = built(&banks, &ids).unwrap();
    assert_eq!(d.len(), MAX_BYTES);
    assert_eq!(file_len(&banks, &ids), Ok(MAX_BYTES));
    let w = Waveform::parse(&d).unwrap();
    assert_eq!(w.samples().len(), 1024);
}

// --- refusals of the reader ---

/// Sets the size field and the CRC to agree with the bytes.
fn reseal(d: &mut [u8]) {
    let n = d.len() as u32;
    d[8..12].copy_from_slice(&n.to_le_bytes());
    let c = crc32c(&d[16..]);
    d[12..16].copy_from_slice(&c.to_le_bytes());
}

/// The example with `edit` applied and resealed is refused with `e`.
fn refused(e: Error, edit: impl Fn(&mut Vec<u8>)) {
    let mut d = example();
    edit(&mut d);
    reseal(&mut d);
    assert_eq!(Waveform::parse(&d).map(|_| ()), Err(e));
}

const BANK0: usize = 84;
const BANK1: usize = 124;
const BANK2: usize = 164;

#[test]
fn refuses_wrong_magic() {
    refused(Error::NotWaveform, |d| d[3] = b'X');
    assert_eq!(Waveform::parse(b"OPW").map(|_| ()), Err(Error::NotWaveform));
}

#[test]
fn refuses_wrong_version() {
    refused(Error::UnsupportedVersion, |d| d[4] = 2);
}

#[test]
fn refuses_flags() {
    refused(Error::BadFlags, |d| d[7] = 0x80);
}

#[test]
fn refuses_wrong_length() {
    refused(Error::BadLength, |d| d.extend([0; 40]));
    refused(Error::BadLength, |d| d.truncate(d.len() - 32));
    let mut d = example();
    d.truncate(19);
    assert_eq!(Waveform::parse(&d).map(|_| ()), Err(Error::BadLength));
    let mut big = example();
    big.resize(MAX_BYTES + 1, 0);
    reseal(&mut big);
    assert_eq!(Waveform::parse(&big).map(|_| ()), Err(Error::BadLength));
}

#[test]
fn refuses_wrong_size_field() {
    let mut d = example();
    d[8] += 1;
    assert_eq!(Waveform::parse(&d).map(|_| ()), Err(Error::BadSize));
}

#[test]
fn refuses_wrong_checksum() {
    let mut d = example();
    d[12] ^= 1;
    assert_eq!(Waveform::parse(&d).map(|_| ()), Err(Error::BadChecksum));
}

#[test]
fn refuses_sample_count() {
    refused(Error::BadSampleCount, |d| {
        d[16..18].copy_from_slice(&0u16.to_le_bytes())
    });
    refused(Error::BadSampleCount, |d| {
        d[16..18].copy_from_slice(&1025u16.to_le_bytes())
    });
}

#[test]
fn refuses_bank_count() {
    refused(Error::BadBankCount, |d| d[18..20].copy_from_slice(&0u16.to_le_bytes()));
    refused(Error::BadBankCount, |d| {
        d[18..20].copy_from_slice(&1025u16.to_le_bytes())
    });
}

#[test]
fn refuses_reserved_bytes() {
    for at in [7, 10, 11, 22, 23] {
        refused(Error::ReservedNonZero, |d| d[BANK1 + at] = 1);
    }
}

#[test]
fn refuses_fields_out_of_range() {
    let cases: [(usize, &[u8]); 12] = [
        (1, &[128]),                          // highest key
        (2, &[0]),                            // lowest velocity
        (3, &[128]),                          // highest velocity
        (4, &[128]),                          // root
        (5, &[0x80]),                         // transpose -128
        (6, &[2]),                            // loop
        (12, &1_200_001i32.to_le_bytes()),    // tune
        (12, &(-1_200_001i32).to_le_bytes()), // tune
        (16, &4_801i32.to_le_bytes()),        // level
        (16, &(-14_401i32).to_le_bytes()),    // level
        (20, &10_001i16.to_le_bytes()),       // pan
        (20, &(-10_001i16).to_le_bytes()),    // pan
    ];
    for (at, v) in cases {
        refused(Error::BadRange, |d| {
            d[BANK1 + at..BANK1 + at + v.len()].copy_from_slice(v)
        });
    }
    // the limits themselves are accepted (on a bank whose order they do not change)
    let mut d = example();
    for (at, v) in [(12, 1_200_000i32), (16, 4_800)] {
        d[BANK2 + at..BANK2 + at + 4].copy_from_slice(&v.to_le_bytes());
    }
    reseal(&mut d);
    Waveform::parse(&d).unwrap();
}

#[test]
fn refuses_lowest_above_highest() {
    refused(Error::LowAboveHigh, |d| d[BANK1] = 60);
    refused(Error::LowAboveHigh, |d| d[BANK1 + 3] = 0);
}

#[test]
fn refuses_sample_index() {
    refused(Error::BadSampleIndex, |d| d[BANK2 + 8] = 2);
}

#[test]
fn refuses_loop_points() {
    refused(Error::BadLoop, |d| d[BANK1 + 36] = 1); // loop end without a loop
    refused(Error::BadLoop, |d| d[BANK1 + 32] = 1); // loop start without a loop
    refused(Error::BadLoop, |d| {
        d[BANK2 + 32..BANK2 + 36].copy_from_slice(&2000u32.to_le_bytes())
    }); // start = end
    refused(Error::BadLoop, |d| {
        d[BANK2 + 24..BANK2 + 28].copy_from_slice(&101u32.to_le_bytes())
    }); // start after loop start
    refused(Error::BadLoop, |d| {
        d[BANK2 + 28..BANK2 + 32].copy_from_slice(&1999u32.to_le_bytes())
    }); // loop end after end
        // loop end equal to end is accepted
    let mut d = example();
    d[BANK2 + 28..BANK2 + 32].copy_from_slice(&2000u32.to_le_bytes());
    reseal(&mut d);
    Waveform::parse(&d).unwrap();
}

#[test]
fn refuses_unsorted_banks() {
    refused(Error::NotSorted, |d| {
        let b0: Vec<u8> = d[BANK0..BANK1].to_vec();
        d.copy_within(BANK2..BANK2 + 40, BANK0);
        d[BANK2..BANK2 + 40].copy_from_slice(&b0);
    });
}

#[test]
fn refuses_equal_banks() {
    refused(Error::DuplicateBank, |d| d.copy_within(BANK1..BANK2, BANK0));
}

#[test]
fn refuses_sample_ids_out_of_first_use_order() {
    // the same waveform with its two ids listed the other way round
    refused(Error::SampleOrder, |d| {
        let a: Vec<u8> = d[20..52].to_vec();
        d.copy_within(52..84, 20);
        d[52..84].copy_from_slice(&a);
        for b in [BANK0, BANK1, BANK2] {
            d[b + 8] ^= 1;
        }
    });
}

#[test]
fn refuses_unused_sample() {
    refused(Error::UnusedSample, |d| {
        d.splice(84..84, [9u8; 32]);
        d[16] = 3;
    });
}

#[test]
fn refuses_duplicate_sample() {
    refused(Error::DuplicateSample, |d| d.copy_within(20..52, 52));
}

#[test]
fn every_corruption_and_truncation_is_refused() {
    let d = example();
    for i in 0..d.len() {
        for v in 0..=255u8 {
            if v != d[i] {
                let mut c = d.clone();
                c[i] = v;
                assert!(Waveform::parse(&c).is_err(), "byte {i} = {v}");
            }
        }
    }
    for n in 0..d.len() {
        assert!(Waveform::parse(&d[..n]).is_err(), "truncated to {n}");
    }
}

// --- refusals of the writer ---

#[test]
fn build_refuses_equal_banks() {
    let (ids, b) = given();
    assert_eq!(built(&[b[0], b[1], b[0]], &ids), Err(Error::DuplicateBank));
    // equal through two indices of one id
    let twice = [ids[0], ids[1], ids[0]];
    assert_eq!(
        built(&[b[1], KeyBank { sample: 2, ..b[1] }], &twice),
        Err(Error::DuplicateBank)
    );
}

#[test]
fn build_refuses_bad_input() {
    let (ids, b) = given();
    assert_eq!(built(&[KeyBank { root: 128, ..b[0] }], &ids), Err(Error::BadRange));
    assert_eq!(
        built(&[KeyBank { sample: 2, ..b[0] }], &ids),
        Err(Error::BadSampleIndex)
    );
    assert_eq!(built(&[KeyBank { loop_mode: 0, ..b[0] }], &ids), Err(Error::BadLoop));
    assert_eq!(built(&[], &ids), Err(Error::BadBankCount));
    assert_eq!(built(&[b[1]; 1025], &ids), Err(Error::BadBankCount));
    let mut out = vec![0; example().len() - 1];
    assert_eq!(build(&b, &ids, &mut out), Err(Error::BufferTooSmall));
}
