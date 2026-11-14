// SPDX-License-Identifier: MIT
//! The command-line tool end to end, on the synthetic corpus: pack from WAV and FLAC, ids,
//! verify, extract (checked with the stock `flac` tool), copy, combine, remove, compact,
//! determinism and torn-tail handling. Tests that need the `flac` tool fail when it is missing,
//! unless SAMPLEPACK_NO_FLAC=1 is set, in which case they print that they were skipped.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::string_slice
)]

use samplepack::pack::{IndexEntry, Pack};
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_samplepack"))
}

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/data")
        .join(name)
}

fn work(test: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{cmd:?}\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

fn fails(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(!out.status.success(), "{cmd:?} should fail");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn have_flac() -> bool {
    if Command::new("flac")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return true;
    }
    assert!(
        std::env::var("SAMPLEPACK_NO_FLAC").as_deref() == Ok("1"),
        "the flac tool is needed"
    );
    eprintln!("skipped: no flac tool (SAMPLEPACK_NO_FLAC=1)");
    false
}

fn wav(path: &Path, rate: u32, channels: u16, pcm: &[u8]) {
    let mut w = b"RIFF".to_vec();
    w.extend((36 + pcm.len() as u32).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(channels.to_le_bytes());
    w.extend(rate.to_le_bytes());
    w.extend((rate * u32::from(channels) * 2).to_le_bytes());
    w.extend((channels * 2).to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend((pcm.len() as u32).to_le_bytes());
    w.extend(pcm);
    std::fs::write(path, w).unwrap();
}

/// Live entries of a pack file, in file order.
fn entries(path: &Path) -> (Vec<u8>, Vec<IndexEntry>) {
    let d = std::fs::read(path).unwrap();
    let e = Pack::open(&d).unwrap().entries().collect();
    (d, e)
}

fn ids_of(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.len() > 66 && l.as_bytes()[64] == b' ')
        .map(|l| l[..64].to_string())
        .collect()
}

#[test]
fn ids_are_stable_across_containers_and_encodings() {
    let d = work("ids");
    let pcm = std::fs::read(corpus("sweep-st-b1024-l4-r44100.pcm")).unwrap();
    wav(&d.join("a.wav"), 44100, 2, &pcm);
    wav(&d.join("a48.wav"), 48000, 2, &pcm);
    wav(&d.join("amono.wav"), 44100, 1, &pcm);
    let id = |input: &Path, out: &str| ids_of(&run(bin().args(["pack", "-o"]).arg(d.join(out)).arg(input)))[0].clone();
    let from_wav = id(&d.join("a.wav"), "1.osp");
    assert_eq!(
        from_wav,
        id(&corpus("sweep-st-b1024-l4-r44100.flac"), "2.osp"),
        "WAV and FLAC"
    );
    // LPC order 12: outside the reader's subset, decoded with the flac tool
    if have_flac() {
        assert_eq!(
            from_wav,
            id(&corpus("sweep-st-b1024-l12-r44100.flac"), "3.osp"),
            "differently encoded FLAC"
        );
        run(Command::new("flac")
            .args(["-s", "-f", "-0", "-b", "4608", "-o"])
            .arg(d.join("b.flac"))
            .arg(d.join("a.wav")));
        assert_eq!(from_wav, id(&d.join("b.flac"), "4.osp"), "flac -0 -b 4608");
    }
    assert_ne!(from_wav, id(&d.join("a48.wav"), "5.osp"), "rate");
    assert_ne!(from_wav, id(&d.join("amono.wav"), "6.osp"), "channels");
    // the same audio twice in one pack is stored once
    let out = run(bin()
        .args(["pack", "-o"])
        .arg(d.join("7.osp"))
        .arg(d.join("a.wav"))
        .arg(corpus("sweep-st-b1024-l4-r44100.flac")));
    assert!(out.contains("same audio as an earlier input"));
    assert_eq!(entries(&d.join("7.osp")).1.len(), 1);
}

#[test]
fn round_trip_extract_and_flac_test() {
    let d = work("extract");
    let names = [
        "sweep-st-b1024-l4-r44100",
        "noise-mono-b1024-l4-r44100",
        "clipped-st-b4096-l8-r48000",
        "silence-st-b1024-l4-r44100",
        "impulses-mono-b1024-l4-r44100",
        "colored-st-b192-l4-r8000",
        "sweep-mono-b100-l3-r44100",
    ];
    let inputs: Vec<PathBuf> = names.iter().map(|n| corpus(&format!("{n}.flac"))).collect();
    let pk = d.join("p.osp");
    let out = run(bin()
        .args(["pack", "--credit", "synthetic test signals", "-o"])
        .arg(&pk)
        .args(&inputs));
    let ids = ids_of(&out);
    assert_eq!(ids.len(), names.len());
    run(bin().args(["verify", "--full"]).arg(&pk));
    run(bin().arg("verify").arg(&pk));
    let listed = run(bin().arg("list").arg(&pk));
    assert!(listed.contains("synthetic test signals"));
    let flac = have_flac();
    for (id, name) in ids.iter().zip(names) {
        let f = d.join(format!("{name}.flac"));
        run(bin().arg("extract").arg(&pk).arg(&id[..12]).arg("-o").arg(&f));
        // Without the flac tool too: the extracted file packs again to the same sample id.
        let again = ids_of(&run(bin()
            .args(["pack", "-o"])
            .arg(d.join(format!("{name}.osp")))
            .arg(&f)));
        assert_eq!(&again[0], id, "{name}: extract is bit-exact");
        if flac {
            run(Command::new("flac").args(["-t", "-s"]).arg(&f));
            let raw = d.join(format!("{name}.raw"));
            run(Command::new("flac")
                .args([
                    "-d",
                    "-s",
                    "-f",
                    "--force-raw-format",
                    "--endian=little",
                    "--sign=signed",
                    "-o",
                ])
                .arg(&raw)
                .arg(&f));
            assert_eq!(
                std::fs::read(&raw).unwrap(),
                std::fs::read(corpus(&format!("{name}.pcm"))).unwrap(),
                "{name} bit-exact"
            );
        }
    }
    let s = run(bin().arg("locality").arg(&pk).arg(&ids[0][..8]).arg(&ids[2][..8]));
    assert!(s.contains("records: 2") && s.contains("merged"));
}

#[test]
fn copy_combine_remove_compact_keep_record_bytes() {
    let d = work("copy");
    let ins = |names: &[&str]| names.iter().map(|n| corpus(&format!("{n}.flac"))).collect::<Vec<_>>();
    let a = d.join("a.osp");
    let b = d.join("b.osp");
    let ida = ids_of(&run(bin().args(["pack", "--window", "8192", "-o"]).arg(&a).args(ins(
        &[
            "sweep-st-b1024-l4-r44100",
            "noise-st-b1024-l4-r44100",
            "dc-mono-b1024-l4-r44100",
        ],
    ))));
    let idb = ids_of(&run(bin()
        .args(["pack", "--window", "0", "-o"])
        .arg(&b)
        .args(ins(&["colored-st-b1024-l4-r44100", "sweep-st-b1024-l4-r44100"]))));
    let (da, ea) = entries(&a);
    let rec = |data: &[u8], e: &IndexEntry| data[e.offset as usize..(e.offset + e.length) as usize].to_vec();
    let bytes_of = |path: &Path| {
        let (data, es) = entries(path);
        es.iter().map(|e| (e.id, rec(&data, e))).collect::<Vec<_>>()
    };
    // copy a subset, in the order given
    let c = d.join("c.osp");
    run(bin().arg("copy").arg(&a).arg(&c).arg(&ida[2]).arg(&ida[0][..10]));
    let cb = bytes_of(&c);
    assert_eq!(cb.len(), 2);
    assert_eq!(cb[0].1, rec(&da, &ea[2]));
    assert_eq!(cb[1].1, rec(&da, &ea[0]));
    run(bin().args(["verify", "--full"]).arg(&c));
    // combine: a's records then b's new ones; the sweep (in both, with different windows) once
    let m = d.join("m.osp");
    run(bin().arg("combine").arg("-o").arg(&m).arg(&a).arg(&b));
    let mb = bytes_of(&m);
    assert_eq!(mb.len(), 4);
    assert_eq!(ida[0], idb[1], "same audio, same id, whatever the window");
    let ab = bytes_of(&a);
    for (id, bytes) in ab
        .iter()
        .chain(bytes_of(&b).iter().filter(|(i, _)| !ab.iter().any(|(j, _)| j == i)))
    {
        let found = mb.iter().find(|(i, _)| i == id).unwrap();
        assert_eq!(
            &found.1, bytes,
            "record copied byte for byte (the first pack's copy wins)"
        );
    }
    assert_eq!(hex(&mb[0].0), ida[0]);
    run(bin().args(["verify", "--full"]).arg(&m));
    // remove (tombstone) then compact
    let before = std::fs::metadata(&m).unwrap().len();
    run(bin().arg("remove").arg(&m).arg(&ida[1]));
    assert!(std::fs::metadata(&m).unwrap().len() > before);
    run(bin().args(["verify", "--full"]).arg(&m));
    let kept = bytes_of(&m);
    assert_eq!(kept.len(), 3);
    let order = d.join("order.txt");
    std::fs::write(&order, format!("{}\n", hex(&kept[2].0))).unwrap();
    run(bin().arg("compact").arg(&m).arg("--order").arg(&order));
    assert!(std::fs::metadata(&m).unwrap().len() < before);
    let after = bytes_of(&m);
    assert_eq!(after[0], kept[2], "--order puts that record first");
    assert_eq!(&after[1..], &kept[..2]);
    run(bin().args(["verify", "--full"]).arg(&m));
    // removing something absent fails and leaves the pack as it was
    let snapshot = std::fs::read(&m).unwrap();
    fails(bin().arg("remove").arg(&m).arg(&ida[1]));
    assert_eq!(std::fs::read(&m).unwrap(), snapshot);
}

fn hex(id: &[u8]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
#[test]
fn deterministic_output() {
    let d = work("det");
    let ins: Vec<PathBuf> = ["clipped-st-b1024-l4-r44100", "colored-mono-b1024-l4-r44100"]
        .iter()
        .map(|n| corpus(&format!("{n}.flac")))
        .collect();
    run(bin().args(["pack", "-o"]).arg(d.join("1.osp")).args(&ins));
    run(bin().args(["pack", "-o"]).arg(d.join("2.osp")).args(&ins));
    assert_eq!(
        std::fs::read(d.join("1.osp")).unwrap(),
        std::fs::read(d.join("2.osp")).unwrap()
    );
    fails(bin().args(["pack", "-o"]).arg(d.join("1.osp")).args(&ins));
}

/// The encoder is deterministic across targets: CI runs this on every platform against one hash.
#[test]
fn same_pack_on_every_target() {
    let d = work("golden");
    let ins: Vec<PathBuf> = [
        "sweep-st-b1024-l4-r44100",
        "colored-mono-b1024-l4-r44100",
        "correlated-st-b1000-l4-Me-r22050",
        "clipped-st-b4096-l8-r48000",
    ]
    .iter()
    .map(|n| corpus(&format!("{n}.flac")))
    .collect();
    let p = d.join("g.osp");
    run(bin().args(["pack", "--credit", "golden", "-o"]).arg(&p).args(&ins));
    let h = samplepack::hash::Sha256::digest(&std::fs::read(&p).unwrap());
    assert_eq!(hex(&h), GOLDEN);
}
const GOLDEN: &str = "70be89b79f3f0fa724b0711a9657d983b999ec7d8d170bd1db2f6b184d0aca97";

#[test]
fn torn_append_is_cut_back() {
    let d = work("torn");
    let p = d.join("p.osp");
    run(bin()
        .args(["pack", "-o"])
        .arg(&p)
        .arg(corpus("sweep-st-b1024-l4-r44100.flac")));
    let committed = std::fs::read(&p).unwrap();
    run(bin()
        .args(["pack", "--append", "-o"])
        .arg(&p)
        .arg(corpus("noise-st-b1024-l4-r44100.flac")));
    let full = std::fs::read(&p).unwrap();
    // a crash part-way through the second append: the file ends inside the new record
    std::fs::write(&p, &full[..committed.len() + 5000]).unwrap();
    let listed = run(bin().arg("list").arg(&p));
    assert!(listed.contains("1 records") && listed.contains("ignored"));
    fails(bin().arg("verify").arg(&p));
    // the next append cuts the tail back and commits on top of the last good state
    run(bin()
        .args(["pack", "--append", "-o"])
        .arg(&p)
        .arg(corpus("noise-st-b1024-l4-r44100.flac")));
    assert_eq!(std::fs::read(&p).unwrap(), full);
    run(bin().args(["verify", "--full"]).arg(&p));
}

/// Synthetic interleaved 16-bit PCM, `frames` x `channels`, as little-endian bytes.
fn synth(frames: usize, channels: usize) -> Vec<u8> {
    (0..frames * channels)
        .flat_map(|i| {
            let (t, c) = ((i / channels) as i32, (i % channels) as i32);
            ((t * 37 + c * 11) % 2000 - 1000 + (t * t) % 97).to_le_bytes()[..2].to_vec()
        })
        .collect()
}

/// Packs one WAV, verifies the pack, extracts the record and packs the extract again; returns the
/// sample id (equal both times), the pack bytes and the extracted file.
fn round_trip(d: &Path, name: &str, rate: u32, channels: u16, pcm: &[u8], window: &str) -> (String, Vec<u8>, Vec<u8>) {
    let (w, p, f) = (
        d.join(format!("{name}.wav")),
        d.join(format!("{name}.osp")),
        d.join(format!("{name}.flac")),
    );
    wav(&w, rate, channels, pcm);
    let id = ids_of(&run(bin().args(["pack", "--window", window, "-o"]).arg(&p).arg(&w)))[0].clone();
    run(bin().args(["verify", "--full"]).arg(&p));
    run(bin().arg("extract").arg(&p).arg(&id).arg("-o").arg(&f));
    let again = ids_of(&run(bin()
        .args(["pack", "-o"])
        .arg(d.join(format!("{name}.2.osp")))
        .arg(&f)));
    assert_eq!(again[0], id, "{name}: extract packs to the same id");
    if have_flac() {
        run(Command::new("flac").args(["-t", "-s"]).arg(&f));
    }
    (id, std::fs::read(&p).unwrap(), std::fs::read(&f).unwrap())
}

/// Rates inside and outside FLAC's header codes, up to the record maximum: the frame header uses a
/// code that gives the rate exactly, or 0 ("from STREAMINFO") when none does, and the extracted
/// STREAMINFO carries the rate.
#[test]
fn any_rate_round_trips() {
    use samplepack::flac::{self, FrameEntry, Scratch, SAMPLE_RATES};
    let d = work("rates");
    let pcm = synth(3000, 2);
    let mut ids = Vec::new();
    for rate in [
        1143, 8000, 27471, 44100, 70958, 96000, 139636, 192000, 384000, 1_048_575,
    ] {
        let (id, _, file) = round_trip(&d, &format!("r{rate}"), rate, 2, &pcm, "4096");
        let si = &file[8..42];
        assert_eq!(
            (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4),
            rate
        );
        let exact = |code: u8, x: &[u8]| match code {
            1..=11 => SAMPLE_RATES[usize::from(code)] == rate,
            12 => u32::from(x[0]) * 1000 == rate,
            13 => u32::from(u16::from_be_bytes([x[0], x[1]])) == rate,
            14 => u32::from(u16::from_be_bytes([x[0], x[1]])) * 10 == rate,
            _ => false,
        };
        let any_exact = SAMPLE_RATES.contains(&rate)
            || (rate % 1000 == 0 && rate / 1000 <= 255)
            || rate <= 65535
            || (rate % 10 == 0 && rate / 10 <= 65535);
        let frames = &file[flac::first_frame(&file).unwrap()..];
        let mut t = vec![FrameEntry::default(); 8];
        let r = flac::scan(frames, flac::FLAG_CHECK_CRC, &mut Box::new(Scratch::new()), &mut t);
        assert_eq!(r.status, Ok(()));
        for e in &t[..r.frames] {
            let h = &frames[e.offset as usize..];
            let code = h[2] & 15;
            // after sync, codes and the 1- or 2-byte frame number (frames 0 to 2) and an explicit block size
            let extra = &h[5 + if h[2] >> 4 == 7 { 2 } else { 0 }..];
            assert!(
                if code == 0 { !any_exact } else { exact(code, extra) },
                "rate {rate}: code {code}"
            );
        }
        ids.push(id);
    }
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "the same PCM at another rate is another sample");
}

/// Samples shorter than one block, shorter than the smallest block size and one over a block:
/// pack, verify, extract, and decode block by block; windows larger than the sample hold it whole.
#[test]
fn short_samples_round_trip() {
    use samplepack::flac::Scratch;
    let d = work("short");
    for frames in [1usize, 15, 16, 255, 256, 257, 1025] {
        let pcm = synth(frames, 1);
        let want: Vec<i16> = pcm.chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        for window in ["4096", "1024"] {
            let (_, pack, _) = round_trip(&d, &format!("n{frames}w{window}"), 44100, 1, &pcm, window);
            let p = Pack::open(&pack).unwrap();
            let r = p.record(&p.entries().next().unwrap()).unwrap();
            let i = r.info();
            let expect_window = if frames <= 1024 || window == "4096" {
                frames
            } else {
                1024
            };
            assert_eq!((i.frames as usize, i.window_frames as usize), (frames, expect_window));
            assert_eq!(r.window(), &pcm[..expect_window * 2]);
            let mut s = Box::new(Scratch::new());
            let mut out = vec![0i16; 2048];
            let mut got = Vec::new();
            for b in 0..i.blocks {
                let n = r.decode(b, 1, 0, &mut s, &mut out, &mut []).unwrap();
                got.extend_from_slice(&out[..n]);
            }
            assert_eq!(got, want, "{frames} frames block by block");
            assert_eq!(r.decode(0, i.blocks, 0, &mut s, &mut out, &mut []), Ok(frames));
            assert_eq!(&out[..frames], &want[..]);
        }
    }
}
