// SPDX-License-Identifier: MIT
//! Decodes the corpus in `tests/data` (stock-encoder FLAC files and what `flac -d` made of them)
//! and checks sample-for-sample equality, frame tables, rejections and buffer handling.

use samplepack::flac::{self, Error, FrameEntry, Scratch, FLAG_CHECK_CRC, MAX_BLOCK};
use std::fs;
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn names() -> Vec<String> {
    fs::read_to_string(data_dir().join("manifest.txt"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn load(name: &str) -> (Vec<u8>, Vec<i16>) {
    let f = fs::read(data_dir().join(format!("{name}.flac"))).unwrap();
    let p = fs::read(data_dir().join(format!("{name}.pcm"))).unwrap();
    (f, p.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
}

#[test]
fn corpus_matches_flac_decoder() {
    let mut scratch = Box::new(Scratch::new());
    let mut assigns = [0usize; 16];
    let mut files = 0;
    let mut frames_total = 0;
    for name in names() {
        let (file, pcm) = load(&name);
        let first = flac::first_frame(&file).unwrap();
        let data = &file[first..];
        let mut table = vec![FrameEntry::default(); 4096];
        let scan = flac::scan(data, FLAG_CHECK_CRC, &mut scratch, &mut table);
        if name.contains("-l12-") {
            assert_eq!(
                scan.status,
                Err(Error::Unsupported),
                "{name}: orders above 8 must be rejected"
            );
            continue;
        }
        assert_eq!(scan.status, Ok(()), "{name}");
        assert_eq!(scan.consumed, data.len(), "{name}");
        let mono = name.contains("-mono-");
        let ch = if mono { 1 } else { 2 };
        assert_eq!(scan.total_samples as usize * ch, pcm.len(), "{name}");
        let (mut l, mut r) = (vec![0i16; MAX_BLOCK], vec![0i16; MAX_BLOCK]);
        let mut pos = 0;
        for e in &table[..scan.frames] {
            let frame = &data[e.offset as usize..];
            assigns[usize::from(frame[3] >> 4)] += 1;
            for flags in [0, FLAG_CHECK_CRC] {
                let info = flac::decode_frame(frame, flags, &mut scratch, &mut l, &mut r).unwrap();
                assert_eq!((info.samples, info.consumed), (e.samples, e.length), "{name}");
                assert_eq!(info.channels as usize, ch);
            }
            for i in 0..e.samples as usize {
                assert_eq!(l[i], pcm[(pos + i) * ch], "{name} frame at {} sample {i}", e.offset);
                if !mono {
                    assert_eq!(r[i], pcm[(pos + i) * 2 + 1], "{name} frame at {} sample {i}", e.offset);
                }
            }
            pos += e.samples as usize;
            frames_total += 1;
        }
        assert_eq!(pos * ch, pcm.len());
        files += 1;
    }
    println!(
        "{files} files, {frames_total} frames, channel assignments (code: frames): {:?}",
        assigns.iter().enumerate().filter(|(_, n)| **n > 0).collect::<Vec<_>>()
    );
    for code in [0usize, 1, 8, 9, 10] {
        assert!(assigns[code] > 0, "corpus has no frames with channel assignment {code}");
    }
}

fn first_frame_of(name: &str) -> Vec<u8> {
    let (file, _) = load(name);
    let first = flac::first_frame(&file).unwrap();
    let mut scratch = Box::new(Scratch::new());
    let mut table = [FrameEntry::default(); 1];
    let _ = flac::scan(&file[first..], 0, &mut scratch, &mut table);
    file[first..first + table[0].length as usize].to_vec()
}

#[test]
fn rejections_and_buffers() {
    let mut s = Box::new(Scratch::new());
    let frame = first_frame_of("sweep-st-b1024-l4-r44100");
    let (mut l, mut r) = (vec![0x5555i16; MAX_BLOCK], vec![0x5555i16; MAX_BLOCK]);
    assert!(flac::decode_frame(&frame, FLAG_CHECK_CRC, &mut s, &mut l, &mut r).is_ok());

    let run = |f: &[u8], flags| {
        let (mut l, mut r) = (vec![0x5555i16; MAX_BLOCK], vec![0x5555i16; MAX_BLOCK]);
        let mut s = Box::new(Scratch::new());
        let res = flac::decode_frame(f, flags, &mut s, &mut l, &mut r);
        if res.is_err() {
            assert!(
                l.iter().chain(r.iter()).all(|&v| v == 0x5555),
                "outputs written on error"
            );
        }
        res.map(|_| ())
    };
    let with = |i: usize, v: u8| {
        let mut f = frame.clone();
        f[i] = v;
        f
    };
    assert_eq!(run(&with(0, 0xFE), 0), Err(Error::BadSync));
    assert_eq!(run(&with(1, 0xF0), 0), Err(Error::BadSync));
    assert_eq!(run(&with(3, frame[3] | 1), 0), Err(Error::ReservedValue));
    assert_eq!(run(&with(2, (frame[2] & 0xF0) | 15), 0), Err(Error::ReservedValue));
    assert_eq!(run(&with(3, 0xB0 | (frame[3] & 0x0F)), 0), Err(Error::ReservedValue));
    assert_eq!(run(&with(3, 0x20 | (frame[3] & 0x0F)), 0), Err(Error::Unsupported)); // 3 channels
    assert_eq!(run(&with(3, (frame[3] & 0xF1) | (2 << 1)), 0), Err(Error::Unsupported)); // 12-bit
    assert_eq!(run(&with(3, (frame[3] & 0xF1) | (6 << 1)), 0), Err(Error::Unsupported)); // 24-bit
    assert_eq!(
        run(&with(3, (frame[3] & 0xF1) | (3 << 1)), 0),
        Err(Error::ReservedValue)
    );
    assert_eq!(run(&with(2, frame[2] & 0x0F), 0), Err(Error::ReservedValue)); // block size code 0
    assert_eq!(run(&with(4, 0x80), 0), Err(Error::ReservedValue)); // continuation byte as lead
    assert_eq!(run(&with(4, 0xFF), 0), Err(Error::ReservedValue));
    assert_eq!(run(&with(4, 0xC2), 0), Err(Error::ReservedValue)); // 2-byte number, bad continuation
    assert_eq!(run(&with(2, (frame[2] & 0x0F) | 0xF0), 0), Err(Error::Unsupported)); // 32768 samples
    assert_eq!(run(&frame[..5], 0), Err(Error::Truncated));
    assert_eq!(run(&[], 0), Err(Error::Truncated));
    // header CRC-8 sits after the 1-byte frame number
    assert_eq!(run(&with(5, frame[5] ^ 1), FLAG_CHECK_CRC), Err(Error::HeaderCrc));
    assert!(run(&with(5, frame[5] ^ 1), 0).is_ok());
    let last = frame.len() - 1;
    assert_eq!(run(&with(last, frame[last] ^ 1), FLAG_CHECK_CRC), Err(Error::FrameCrc));
    assert!(run(&with(last, frame[last] ^ 1), 0).is_ok());
    // every proper prefix of a frame fails, whatever the flags
    for cut in 0..frame.len() {
        assert!(run(&frame[..cut], 0).is_err(), "prefix {cut}");
    }
    assert_eq!(run(&frame, 2), Err(Error::InvalidArgument));
    // buffers smaller than the block size
    let mut small = [0x5555i16; 1023];
    assert_eq!(
        flac::decode_frame(&frame, 0, &mut s, &mut small, &mut r),
        Err(Error::BufferTooSmall)
    );
    assert!(small.iter().all(|&v| v == 0x5555));
    assert_eq!(
        flac::decode_frame(&frame, 0, &mut s, &mut l, &mut small),
        Err(Error::BufferTooSmall)
    );
    assert_eq!(
        flac::decode_frame(&frame, 0, &mut s, &mut [], &mut []),
        Err(Error::BufferTooSmall)
    );
    // a mono frame needs no second buffer
    let mono = first_frame_of("sweep-mono-b1024-l4-r44100");
    assert!(flac::decode_frame(&mono, 0, &mut s, &mut l, &mut []).is_ok());
    // exact-size buffers work and extra bytes after the frame are ignored
    let mut padded = frame.clone();
    padded.extend_from_slice(&[0xAA; 40]);
    let (mut l1, mut r1) = (vec![0i16; 1024], vec![0i16; 1024]);
    let info = flac::decode_frame(&padded, FLAG_CHECK_CRC, &mut s, &mut l1, &mut r1).unwrap();
    assert_eq!(info.consumed as usize, frame.len());
}

#[test]
fn scan_table_limits_and_stream_header() {
    let (file, _) = load("sweep-st-b1024-l4-r44100");
    let first = flac::first_frame(&file).unwrap();
    let mut s = Box::new(Scratch::new());
    let mut tiny = [FrameEntry::default(); 2];
    let res = flac::scan(&file[first..], 0, &mut s, &mut tiny);
    assert_eq!((res.frames, res.status), (2, Err(Error::BufferTooSmall)));
    let res = flac::scan(
        &file[first..file.len() - 3],
        0,
        &mut s,
        &mut [FrameEntry::default(); 64],
    );
    assert!(res.status.is_err() && res.frames == 4);
    assert_eq!(flac::first_frame(b"RIFF....").unwrap_err(), Error::BadStream);
    assert_eq!(flac::first_frame(&file[..40]).unwrap_err(), Error::BadStream);
    assert_eq!(flac::first_frame(&[]).unwrap_err(), Error::BadStream);
}
