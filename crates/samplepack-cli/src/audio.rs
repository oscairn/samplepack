// SPDX-License-Identifier: MIT
//! Audio files in and out: 16-bit PCM WAV and FLAC inputs, standard FLAC output for extract.

use samplepack::flac::{self, Scratch};
use samplepack::pack::Record;
use std::path::Path;
use std::process::Command;

/// Decoded input audio.
pub struct Audio {
    pub rate: u32,
    pub channels: usize,
    /// Interleaved samples.
    pub pcm: Vec<i16>,
}

fn u16le(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*d.get(at..)?.first_chunk::<2>()?))
}
pub fn u32le(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*d.get(at..)?.first_chunk::<4>()?))
}

fn check(rate: u32, channels: usize, bits: u32, what: &str) -> Result<(), String> {
    if bits != 16 {
        return Err(format!(
            "{what}: {bits}-bit audio; version 1 packs hold 16-bit audio only"
        ));
    }
    if !(1..=2).contains(&channels) {
        return Err(format!(
            "{what}: {channels} channels; only mono and stereo are supported"
        ));
    }
    if !(1..=1_048_575).contains(&rate) {
        return Err(format!("{what}: sample rate {rate} Hz out of range"));
    }
    Ok(())
}

/// The body of the first chunk `id` of a RIFF/WAVE file, if it has one.
pub fn wav_chunk<'a>(d: &'a [u8], id: &[u8; 4], what: &str) -> Result<Option<&'a [u8]>, String> {
    let mut pos = 12usize;
    while let Some(len) = u32le(d, pos + 4) {
        let (body, len) = (pos + 8, len as usize);
        if d.get(pos..pos + 4) == Some(id) {
            let name = String::from_utf8_lossy(id);
            return d
                .get(body..body + len)
                .map(Some)
                .ok_or_else(|| format!("{what}: {name} chunk truncated"));
        }
        pos = body + len + (len & 1);
    }
    Ok(None)
}

/// Reads a RIFF/WAVE file of 16-bit PCM (format 1, or extensible with the PCM subformat).
pub fn read_wav(d: &[u8], what: &str) -> Result<Audio, String> {
    let bad = || format!("{what}: not a 16-bit PCM WAV file");
    if d.get(..4) != Some(b"RIFF") || d.get(8..12) != Some(b"WAVE") {
        return Err(bad());
    }
    let fmt = wav_chunk(d, b"fmt ", what)?.ok_or_else(bad)?;
    let mut tag = u16le(fmt, 0).ok_or_else(bad)?;
    let ch = usize::from(u16le(fmt, 2).ok_or_else(bad)?);
    let rate = u32le(fmt, 4).ok_or_else(bad)?;
    let bits = u32::from(u16le(fmt, 14).ok_or_else(bad)?);
    if tag == 0xFFFE {
        tag = u16le(fmt, 24).ok_or_else(bad)?;
    }
    if tag != 1 {
        return Err(bad());
    }
    check(rate, ch, bits, what)?;
    let data = wav_chunk(d, b"data", what)?.ok_or_else(bad)?;
    if data.len() % (2 * ch) != 0 {
        return Err(format!("{what}: data chunk is not whole frames"));
    }
    let pcm = data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
    Ok(Audio {
        rate,
        channels: ch,
        pcm,
    })
}

/// Reads a FLAC file. Frames inside the reader's subset are decoded by the library; anything else
/// (block sizes above 4096) is decoded by the stock `flac` tool.
pub fn read_flac(path: &Path, d: &[u8], what: &str) -> Result<Audio, String> {
    let bad = || format!("{what}: not a FLAC file");
    if d.get(..4) != Some(b"fLaC") || d.get(4).map(|b| b & 0x7F) != Some(0) {
        return Err(bad());
    }
    let si = d.get(8..42).ok_or_else(bad)?;
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
    let channels = usize::from((si[12] >> 1) & 7) + 1;
    let bits = ((u32::from(si[12]) & 1) << 4 | u32::from(si[13]) >> 4) + 1;
    let total = (u64::from(si[13]) & 15) << 32 | u64::from(u32::from_be_bytes([si[14], si[15], si[16], si[17]]));
    check(rate, channels, bits, what)?;
    let first = flac::first_frame(d).map_err(|_| bad())?;
    let frames = d.get(first..).unwrap_or(&[]);
    let mut scratch = Box::new(Scratch::new());
    let (mut l, mut r) = (vec![0i16; flac::MAX_BLOCK], vec![0i16; flac::MAX_BLOCK]);
    let mut pcm = Vec::new();
    let mut pos = 0usize;
    let mut ok = true;
    while pos < frames.len() {
        match flac::decode_frame(&frames[pos..], flac::FLAG_CHECK_CRC, &mut scratch, &mut l, &mut r) {
            Ok(fi) if fi.channels as usize == channels => {
                for i in 0..fi.samples as usize {
                    pcm.push(l[i]);
                    if channels == 2 {
                        pcm.push(r[i]);
                    }
                }
                pos += fi.consumed as usize;
            }
            _ => {
                ok = false;
                break;
            }
        }
    }
    if !ok {
        let out = Command::new("flac")
            .args([
                "-d",
                "-s",
                "-c",
                "--force-raw-format",
                "--endian=little",
                "--sign=signed",
            ])
            .arg(path)
            .output()
            .map_err(|e| format!("{what}: outside the subset and the flac tool could not be run: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "{what}: flac -d failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        pcm = out
            .stdout
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
    }
    if total != 0 && pcm.len() as u64 != total * channels as u64 {
        return Err(format!(
            "{what}: decoded {} frames, STREAMINFO says {total}",
            pcm.len() / channels
        ));
    }
    Ok(Audio { rate, channels, pcm })
}

/// Reads a .wav or .flac input by its content.
pub fn read_audio(path: &Path) -> Result<Audio, String> {
    let what = path.display().to_string();
    let d = std::fs::read(path).map_err(|e| format!("{what}: {e}"))?;
    if d.starts_with(b"fLaC") {
        read_flac(path, &d, &what)
    } else {
        read_wav(&d, &what)
    }
}

/// A standard FLAC file of a record: STREAMINFO (with the record's MD5) and its frames, copied.
pub fn flac_file(rec: &Record) -> Vec<u8> {
    let i = rec.info();
    let mut out = b"fLaC".to_vec();
    out.extend([0x80, 0, 0, 34]);
    let bs = i.block_size as u16;
    out.extend(bs.to_be_bytes());
    out.extend(bs.to_be_bytes());
    out.extend(&i.min_frame.to_be_bytes()[1..]);
    out.extend(&i.max_frame.to_be_bytes()[1..]);
    let packed: u64 = (u64::from(i.rate) << 44)
        | (u64::from(i.channels - 1) << 41)
        | (u64::from(i.bits - 1) << 36)
        | u64::from(i.frames);
    out.extend(packed.to_be_bytes());
    out.extend(i.md5);
    out.extend(rec.frames());
    out
}
