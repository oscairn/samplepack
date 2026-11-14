// SPDX-License-Identifier: MIT
//! `samplepack`: make, inspect and rearrange Oscairn Sample Packs (.osp). See `usage()`.
// A panic here ends the tool with a message; the pack parsing itself is the library's, which
// cannot panic, and writes follow the append protocol, so a panic cannot leave a pack corrupt.
#![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]

mod audio;
mod encode;
mod packio;

use packio::{err, hex, Item};
use samplepack::id::{sample_id, SampleId};
use samplepack::pack::{self, find, IndexEntry, Pack, RecordParams, VerifyScratch, DEFAULT_WINDOW};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const BLOCK: usize = 1024;

/// An encoded input: its sample id and record bytes.
type Encoded = Result<(SampleId, Vec<u8>), String>;

fn usage() -> &'static str {
    "usage:
  samplepack pack -o OUT.osp [--append] [--window FRAMES] [--credit TEXT] INPUT.(wav|flac)...
  samplepack list PACK.osp
  samplepack verify [--full] PACK.osp
  samplepack extract PACK.osp ID [-o OUT.flac]
  samplepack copy SRC.osp DST.osp ID...
  samplepack combine -o DST.osp SRC.osp...
  samplepack compact PACK.osp [--order IDS.txt]
  samplepack remove PACK.osp ID...
  samplepack locality PACK.osp [ID...]
IDs are 64 hex digits or a unique prefix of at least 8. New records are written in the order given,
which keeps one sound's records adjacent; --order lists IDs (one per line) to put first."
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("pack") => cmd_pack(&args[1..]),
        Some("list") => cmd_list(&args[1..]),
        Some("verify") => cmd_verify(&args[1..]),
        Some("extract") => cmd_extract(&args[1..]),
        Some("copy") => cmd_copy(&args[1..]),
        Some("combine") => cmd_combine(&args[1..]),
        Some("compact") => cmd_compact(&args[1..]),
        Some("remove") => cmd_remove(&args[1..]),
        Some("locality") => cmd_locality(&args[1..]),
        _ => Err(usage().to_string()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("samplepack: {e}");
            ExitCode::FAILURE
        }
    }
}

type Parsed<'a> = (Vec<(&'a str, &'a str)>, Vec<&'a str>);

/// Splits `args` into options (with their values, for the names in `valued`) and positionals.
fn parse<'a>(args: &'a [String], valued: &[&str]) -> Result<Parsed<'a>, String> {
    let (mut opts, mut pos) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a.starts_with('-') && a.len() > 1 {
            if valued.contains(&a.as_str()) {
                let v = it.next().ok_or(format!("{a} needs a value"))?;
                opts.push((a.as_str(), v.as_str()));
            } else {
                opts.push((a.as_str(), ""));
            }
        } else {
            pos.push(a.as_str());
        }
    }
    Ok((opts, pos))
}

fn opt<'a>(opts: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    opts.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

fn known(opts: &[(&str, &str)], allowed: &[&str]) -> Result<(), String> {
    match opts.iter().find(|(n, _)| !allowed.contains(n)) {
        Some((n, _)) => Err(format!("unknown option {n}\n{}", usage())),
        None => Ok(()),
    }
}

/// Resolves an id argument (full or a unique prefix of at least 8 hex digits) against `index`.
fn resolve(index: &[IndexEntry], arg: &str) -> Result<IndexEntry, String> {
    let a = arg.to_ascii_lowercase();
    if a.len() < 8 || a.len() > 64 || !a.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{arg}: not a sample id"));
    }
    let hits: Vec<&IndexEntry> = index.iter().filter(|e| hex(&e.id).starts_with(&a)).collect();
    match hits.as_slice() {
        [e] => Ok(**e),
        [] => Err(format!("{arg}: no such sample in the pack")),
        _ => Err(format!("{arg}: prefix matches {} samples", hits.len())),
    }
}

fn sorted_index(p: &Pack) -> Result<Vec<IndexEntry>, String> {
    let mut v = vec![IndexEntry::default(); p.records()];
    p.index(&mut v).map_err(err)?;
    Ok(v)
}

fn cmd_pack(args: &[String]) -> Result<(), String> {
    let (opts, inputs) = parse(args, &["-o", "--window", "--credit"])?;
    known(&opts, &["-o", "--window", "--credit", "--append"])?;
    let out = opt(&opts, "-o").ok_or("pack needs -o OUT.osp")?;
    let window: u32 = match opt(&opts, "--window") {
        Some(w) => w.parse().map_err(|_| format!("--window {w}: not a number"))?,
        None => DEFAULT_WINDOW,
    };
    let credit = opt(&opts, "--credit").unwrap_or("");
    if inputs.is_empty() {
        return Err("pack needs at least one input".into());
    }
    // Inputs are encoded in parallel; results are used in input order, so output does not depend
    // on the thread count.
    let encode_one = |input: &str, vs: &mut VerifyScratch| -> Encoded {
        let a = audio::read_audio(Path::new(input))?;
        let id = sample_id(a.rate, a.channels as u8, &a.pcm);
        let frames = (a.pcm.len() / a.channels) as u32;
        let (data, table) = encode::encode(&a.pcm, a.channels, a.rate, BLOCK);
        let p = RecordParams {
            rate: a.rate,
            channels: a.channels as u8,
            frames,
            block_size: BLOCK as u32,
            window,
            credit,
            table: &table,
            frame_data: &data,
        };
        let mut rec = vec![0u8; pack::record_len(&p).map_err(|e| format!("{input}: {}", err(e)))?];
        let info = pack::write_record(&p, vs, &mut rec).map_err(|e| format!("{input}: {}", err(e)))?;
        // The record's id is computed from its decoded frames; equality with the id of the input
        // samples shows the stored audio decodes bit-exactly to the input.
        if info.id != id {
            return Err(format!("{input}: encoded audio does not decode to the input"));
        }
        Ok((id, rec))
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(inputs.len());
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<(usize, Encoded)> = std::thread::scope(|sc| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                sc.spawn(|| {
                    let mut vs = Box::new(VerifyScratch::new());
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(input) = inputs.get(i) else { break done };
                        done.push((i, encode_one(input, &mut vs)));
                    }
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap_or_default()).collect()
    });
    let mut ordered: Vec<Option<Encoded>> = (0..inputs.len()).map(|_| None).collect();
    for (i, r) in results {
        ordered[i] = Some(r);
    }
    let mut records: Vec<Vec<u8>> = Vec::new();
    let mut ids: Vec<SampleId> = Vec::new();
    for (input, r) in inputs.iter().zip(ordered) {
        let (id, rec) = r.ok_or_else(|| format!("{input}: encoder thread failed"))??;
        if ids.contains(&id) {
            eprintln!("{input}: same audio as an earlier input ({}), skipped", hex(&id));
            continue;
        }
        println!("{}  {input}", hex(&id));
        ids.push(id);
        records.push(rec);
    }
    let items: Vec<Item> = records.iter().map(|r| Item::Record(r)).collect();
    let path = Path::new(out);
    let len = if opt(&opts, "--append").is_some() && path.exists() {
        packio::append(path, &items)?
    } else if path.exists() {
        return Err(format!("{out} exists (use --append to add to it)"));
    } else {
        packio::create(path, &items)?
    };
    println!("{out}: {} records written, {len} bytes", records.len());
    Ok(())
}

fn cmd_list(args: &[String]) -> Result<(), String> {
    let (opts, pos) = parse(args, &[])?;
    known(&opts, &[])?;
    let [path] = pos.as_slice() else {
        return Err(usage().into());
    };
    let l = packio::load(Path::new(path))?;
    let p = l.pack()?;
    println!("# id offset length preload rate channels frames window_frames credit");
    for e in p.entries() {
        let r = p.record(&e).map_err(err)?;
        let i = r.info();
        println!(
            "{} {} {} {} {} {} {} {} {}",
            hex(&e.id),
            e.offset,
            e.length,
            e.preload,
            i.rate,
            i.channels,
            i.frames,
            i.window_frames,
            r.credit()
        );
    }
    println!("# {} records, {} bytes", p.records(), p.data().len());
    Ok(())
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let (opts, pos) = parse(args, &[])?;
    known(&opts, &["--full"])?;
    let [path] = pos.as_slice() else {
        return Err(usage().into());
    };
    let l = packio::load(Path::new(path))?;
    let p = l.pack()?;
    let entries = p.scan().count();
    let mut idx = vec![IndexEntry::default(); entries.max(1)];
    if opt(&opts, "--full").is_some() {
        let mut vs = Box::new(VerifyScratch::new());
        let n = pack::verify_pack(&p, &mut vs, &mut idx).map_err(err)?;
        println!("{path}: full verification passed, {n} records");
    } else {
        let n = p.rebuild_index(&mut idx).map_err(|(e, _)| err(e))?;
        let live = &idx[..n];
        if n != p.records() || p.entries().any(|e| find(live, &e.id) != Some(&e)) {
            return Err(err(pack::Error::TrailerMismatch));
        }
        println!("{path}: structure passed, {n} records (frames not decoded; use --full)");
    }
    if l.dropped > 0 {
        return Err(format!("{path}: has an uncommitted tail of {} bytes", l.dropped));
    }
    Ok(())
}

fn cmd_extract(args: &[String]) -> Result<(), String> {
    let (opts, pos) = parse(args, &["-o"])?;
    known(&opts, &["-o"])?;
    let [path, id] = pos.as_slice() else {
        return Err(usage().into());
    };
    let l = packio::load(Path::new(path))?;
    let p = l.pack()?;
    let e = resolve(&sorted_index(&p)?, id)?;
    let r = p.record(&e).map_err(err)?;
    let mut vs = Box::new(VerifyScratch::new());
    pack::verify_record(&r, &mut vs).map_err(err)?;
    let out = opt(&opts, "-o")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{}.flac", hex(&e.id))));
    std::fs::write(&out, audio::flac_file(&r)).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("{}", out.display());
    Ok(())
}

/// Writes a new pack at `dst` (refusing an existing file unless `replace`) holding the listed
/// records of each source, copied byte for byte; an id already copied is skipped.
fn copy_out(dst: &str, srcs: &[(Pack, Vec<IndexEntry>)], replace: bool) -> Result<(), String> {
    let mut items = Vec::new();
    let mut seen: Vec<SampleId> = Vec::new();
    for (p, list) in srcs {
        for e in list {
            if seen.contains(&e.id) {
                eprintln!("{} is already included, skipped", hex(&e.id));
                continue;
            }
            seen.push(e.id);
            items.push(Item::Record(p.record(e).map_err(err)?.bytes()));
        }
    }
    let path = Path::new(dst);
    if !replace && path.exists() {
        return Err(format!("{dst} exists"));
    }
    let len = packio::create(path, &items)?;
    println!("{dst}: {} records, {len} bytes", items.len());
    Ok(())
}

fn cmd_copy(args: &[String]) -> Result<(), String> {
    let (opts, pos) = parse(args, &[])?;
    known(&opts, &[])?;
    let [src, dst, ids @ ..] = pos.as_slice() else {
        return Err(usage().into());
    };
    if ids.is_empty() {
        return Err("copy needs at least one ID".into());
    }
    let l = packio::load(Path::new(src))?;
    let p = l.pack()?;
    let idx = sorted_index(&p)?;
    let list = ids.iter().map(|a| resolve(&idx, a)).collect::<Result<Vec<_>, _>>()?;
    copy_out(dst, &[(p, list)], false)
}

fn cmd_combine(args: &[String]) -> Result<(), String> {
    let (opts, srcs) = parse(args, &["-o"])?;
    known(&opts, &["-o"])?;
    let dst = opt(&opts, "-o").ok_or("combine needs -o DST.osp")?;
    let loaded = srcs
        .iter()
        .map(|s| packio::load(Path::new(s)))
        .collect::<Result<Vec<_>, _>>()?;
    let parts = loaded
        .iter()
        .map(|l| l.pack().map(|p| (p, p.entries().collect())))
        .collect::<Result<Vec<_>, _>>()?;
    copy_out(dst, &parts, false)
}

fn cmd_compact(args: &[String]) -> Result<(), String> {
    let (opts, pos) = parse(args, &["--order"])?;
    known(&opts, &["--order"])?;
    let [path] = pos.as_slice() else {
        return Err(usage().into());
    };
    let l = packio::load(Path::new(path))?;
    let p = l.pack()?;
    let idx = sorted_index(&p)?;
    let mut list: Vec<IndexEntry> = Vec::new();
    if let Some(f) = opt(&opts, "--order") {
        let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
        for line in text
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
        {
            let e = resolve(&idx, line)?;
            if !list.contains(&e) {
                list.push(e);
            }
        }
    }
    for e in p.entries() {
        if !list.contains(&e) {
            list.push(e);
        }
    }
    // The pack is in memory; create replaces the file through a synced temporary and a rename.
    copy_out(path, &[(p, list)], true)?;
    println!("{path}: compacted from {} bytes", l.data.len() + l.dropped);
    Ok(())
}

fn cmd_remove(args: &[String]) -> Result<(), String> {
    let (opts, pos) = parse(args, &[])?;
    known(&opts, &[])?;
    let [path, ids @ ..] = pos.as_slice() else {
        return Err(usage().into());
    };
    let l = packio::load(Path::new(path))?;
    let idx = sorted_index(&l.pack()?)?;
    let items = ids
        .iter()
        .map(|a| resolve(&idx, a).map(|e| Item::Tombstone(e.id)))
        .collect::<Result<Vec<_>, _>>()?;
    drop(l);
    let len = packio::append(Path::new(path), &items)?;
    println!(
        "{path}: {} removed, {len} bytes (space is reclaimed by compact)",
        items.len()
    );
    Ok(())
}

/// Device model for the locality report (measured on the target's storage).
const SEQ_BYTES_PER_S: f64 = 167e6;
const RANDOM_READ_S: f64 = 0.59e-3;
const RANDOM_READ_BYTES: f64 = 16384.0;

fn cmd_locality(args: &[String]) -> Result<(), String> {
    let (opts, pos) = parse(args, &[])?;
    known(&opts, &[])?;
    let [path, ids @ ..] = pos.as_slice() else {
        return Err(usage().into());
    };
    let l = packio::load(Path::new(path))?;
    let p = l.pack()?;
    let mut set: Vec<IndexEntry> = if ids.is_empty() {
        p.entries().collect()
    } else {
        let idx = sorted_index(&p)?;
        ids.iter().map(|a| resolve(&idx, a)).collect::<Result<Vec<_>, _>>()?
    };
    set.sort_by_key(|e| e.offset);
    set.dedup();
    let (Some(first), Some(last)) = (set.first(), set.last()) else {
        return Err("no records".into());
    };
    // Two reads are merged when the gap between them costs less to read through than a seek.
    let merge_gap = RANDOM_READ_S * SEQ_BYTES_PER_S;
    let mut gaps = Vec::new();
    let (mut reads, mut read_bytes) = (1usize, first.preload as f64);
    for w in set.windows(2) {
        let gap = (w[1].offset - (w[0].offset + w[0].preload)) as f64;
        gaps.push(gap);
        if gap <= merge_gap {
            read_bytes += gap + w[1].preload as f64;
        } else {
            reads += 1;
            read_bytes += w[1].preload as f64;
        }
    }
    let preload: f64 = set.iter().map(|e| e.preload as f64).sum();
    let span = (last.offset + last.preload - first.offset) as f64;
    let rec_span = (last.offset + last.length - first.offset) as f64;
    let records: f64 = set.iter().map(|e| e.length as f64).sum();
    let gap_total: f64 = gaps.iter().sum();
    let big = gaps.iter().filter(|&&g| g > merge_gap).count();
    let cost = |n: usize, bytes: f64| {
        n as f64 * RANDOM_READ_S + (bytes - n as f64 * RANDOM_READ_BYTES).max(0.0) / SEQ_BYTES_PER_S
    };
    let mib = |b: f64| b / 1048576.0;
    println!("records: {}", set.len());
    println!(
        "preload spans: {:.2} MiB in total, file span {:.2} MiB ({:.1}% of it preload)",
        mib(preload),
        mib(span),
        100.0 * preload / span.max(1.0)
    );
    println!(
        "gaps between preload spans: {} ({} over {:.0} KB), {:.2} MiB in total",
        gaps.len(),
        big,
        merge_gap / 1000.0,
        mib(gap_total)
    );
    println!(
        "whole records: {:.2} MiB, file span {:.2} MiB ({} gaps between records)",
        mib(records),
        mib(rec_span),
        set.windows(2)
            .filter(|w| w[1].offset != w[0].offset + w[0].length)
            .count()
    );
    println!(
        "cold preload estimate (model: {:.2} ms per random read, {:.0} MB/s sequential):",
        RANDOM_READ_S * 1e3,
        SEQ_BYTES_PER_S / 1e6
    );
    println!(
        "  one read per record:             {:4} reads, {:.3} s",
        set.len(),
        cost(set.len(), preload)
    );
    println!(
        "  file order, gaps <= {:3.0} KB merged: {:4} reads, {:.3} s",
        merge_gap / 1000.0,
        reads,
        cost(reads, read_bytes)
    );
    println!("  one sequential read of the span:    1 read,  {:.3} s", cost(1, span));
    Ok(())
}
