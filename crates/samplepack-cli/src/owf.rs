// SPDX-License-Identifier: MIT
//! `samplepack waveform`: show and check waveform files, and make one from a WAV file
//! (`docs/waveform.md`).

use crate::packio::{self, hex, Item};
use crate::{audio, encode_record, known, opt, parse, usage};
use samplepack::pack::{VerifyScratch, DEFAULT_WINDOW};
use samplepack::waveform::{self, KeyBank, Waveform};
use std::path::Path;

fn err(e: waveform::Error) -> String {
    format!("{e:?} (code {})", e.code())
}

pub fn cmd(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("show") => show(&args[1..], true),
        Some("check") => show(&args[1..], false),
        Some("from-wav") => from_wav(&args[1..]),
        _ => Err(usage().into()),
    }
}

fn show(args: &[String], all: bool) -> Result<(), String> {
    let (opts, pos) = parse(args, &[])?;
    known(&opts, &[])?;
    let [path] = pos.as_slice() else {
        return Err(usage().into());
    };
    let d = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let w = Waveform::parse(&d).map_err(|e| format!("{path}: {}", err(e)))?;
    if !all {
        println!("{path}: valid, waveform id {}", hex(&w.id()));
        return Ok(());
    }
    println!(
        "id {}\nsamples {}\nbanks {}",
        hex(&w.id()),
        w.samples().len(),
        w.banks()
    );
    for (i, s) in w.samples().iter().enumerate() {
        println!("sample {i} {}", hex(s));
    }
    println!("# key_lo key_hi vel_lo vel_hi root transpose loop sample tune level pan start end loop_start loop_end");
    for b in (0..w.banks()).filter_map(|i| w.bank(i)) {
        println!(
            "bank {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
            b.key_lo,
            b.key_hi,
            b.vel_lo,
            b.vel_hi,
            b.root,
            b.transpose,
            b.loop_mode,
            b.sample,
            b.tune,
            b.level,
            b.pan,
            b.start,
            b.end,
            b.loop_start,
            b.loop_end
        );
    }
    Ok(())
}

/// The key bank for a whole sample of `frames` frames: every key and velocity, root and tune
/// from the WAV `smpl` chunk (root 60 without one), its first loop if that is a forward loop.
fn smpl_bank(smpl: Option<&[u8]>, frames: usize) -> Result<KeyBank, String> {
    let mut b = KeyBank {
        key_hi: 127,
        vel_lo: 1,
        vel_hi: 127,
        root: 60,
        ..KeyBank::default()
    };
    let Some(c) = smpl else { return Ok(b) };
    let at = |i: usize| audio::u32le(c, 4 * i).ok_or("smpl chunk shorter than 36 bytes");
    let (note, fraction, loops) = (at(3)?, at(4)?, at(7)?);
    if u64::from(loops) * 24 + 36 > c.len() as u64 {
        return Err(format!("smpl chunk too short for its {loops} loops"));
    }
    b.root = u8::try_from(note)
        .ok()
        .filter(|n| *n <= 127)
        .ok_or(format!("smpl MIDI unity note {note} out of range"))?;
    // the sample sounds `fraction` / 2^32 of a semitone above the unity note: tune it down by that
    b.tune = -(((u64::from(fraction) * 10_000 + (1 << 31)) >> 32) as i32);
    if loops > 0 && at(10)? == 0 {
        let (start, end) = (at(11)?, at(12)?);
        if start > end || end as usize >= frames {
            return Err(format!("smpl loop {start} to {end} is not inside the {frames} frames"));
        }
        (b.loop_mode, b.loop_start, b.loop_end) = (1, start, end + 1);
    }
    Ok(b)
}

fn from_wav(args: &[String]) -> Result<(), String> {
    let (opts, pos) = parse(args, &["--pack", "--out"])?;
    known(&opts, &["--pack", "--out"])?;
    let ([input], Some(pack), Some(dir)) = (pos.as_slice(), opt(&opts, "--pack"), opt(&opts, "--out")) else {
        return Err(usage().into());
    };
    let d = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let a = audio::read_wav(&d, input)?;
    let bank = smpl_bank(audio::wav_chunk(&d, b"smpl", input)?, a.pcm.len() / a.channels)
        .map_err(|e| format!("{input}: {e}"))?;
    let (id, rec) = encode_record(&a, input, DEFAULT_WINDOW, "", &mut Box::new(VerifyScratch::new()))?;
    let mut owf = vec![0u8; waveform::MAX_BYTES];
    let n = waveform::build(&[bank], &[id], &mut owf).map_err(err)?;
    owf.truncate(n);
    let wid = hex(&Waveform::parse(&owf).map_err(err)?.id());

    let path = Path::new(pack);
    if !path.exists() {
        packio::create(path, &[Item::Record(&rec)])?;
    } else if packio::load(path)?.pack()?.entries().any(|e| e.id == id) {
        eprintln!("{pack}: already holds sample {}", hex(&id));
    } else {
        packio::append(path, &[Item::Record(&rec)])?;
    }
    let out = Path::new(dir).join(format!("{wid}.owf"));
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
    std::fs::write(&out, owf).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("waveform {wid}\nsample {}", hex(&id));
    Ok(())
}
