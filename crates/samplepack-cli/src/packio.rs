// SPDX-License-Identifier: MIT
//! Pack files on disk: reading with torn-tail recovery, creating, and the append protocol
//! (`docs/format.md`, "Appending"): entries, sync, trailer, sync.

use samplepack::id::SampleId;
use samplepack::pack::{self, Error, IndexEntry, Pack, MAX_PACK, PAGE};
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub fn err(e: Error) -> String {
    format!("{e:?} (code {})", e.code())
}

/// A pack file read into memory, cut to its committed length.
pub struct Loaded {
    pub data: Vec<u8>,
    /// Bytes past the committed length (an uncommitted or torn tail), ignored.
    pub dropped: usize,
}

impl Loaded {
    /// The pack (open succeeded on these bytes in `load`).
    pub fn pack(&self) -> Result<Pack<'_>, String> {
        Pack::open(&self.data).map_err(err)
    }
}

pub fn load(path: &Path) -> Result<Loaded, String> {
    let what = path.display();
    let mut data = std::fs::read(path).map_err(|e| format!("{what}: {e}"))?;
    let full = data.len();
    match Pack::open(&data) {
        Ok(_) => {}
        Err(Error::NoFooter) => {
            let keep = pack::recover(&data).map_err(|e| format!("{what}: {}", err(e)))?;
            data.truncate(keep);
            Pack::open(&data).map_err(|e| format!("{what}: {}", err(e)))?;
        }
        Err(e) => return Err(format!("{what}: {}", err(e))),
    }
    let dropped = full - data.len();
    if dropped > 0 {
        eprintln!("{what}: {dropped} bytes after the last commit are ignored (uncommitted or torn append)");
    }
    Ok(Loaded { data, dropped })
}

/// One entry to write: a record (its bytes and the sample id) or a tombstone.
pub enum Item<'a> {
    Record(&'a [u8]),
    Tombstone(SampleId),
}

fn record_entry(bytes: &[u8], offset: u64) -> Result<IndexEntry, String> {
    let r = pack::Record::parse(bytes, offset).map_err(err)?;
    Ok(r.entry())
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(dir)?.sync_all()
}
#[cfg(not(unix))]
fn sync_dir(_: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Writes items at `start` and then the trailer for `live` plus the new records; syncs after
/// the entries and again after the trailer. Returns the new pack length.
fn write_items(f: &mut File, start: u64, mut live: Vec<IndexEntry>, items: &[Item]) -> Result<u64, String> {
    let io = |e: std::io::Error| e.to_string();
    let mut buf = Vec::new();
    let mut pos = start;
    for it in items {
        match it {
            Item::Tombstone(id) => {
                let before = live.len();
                live.retain(|e| &e.id != id);
                if live.len() == before {
                    return Err(format!("{} is not in the pack", hex(id)));
                }
                let mut t = vec![0u8; PAGE];
                pack::write_tombstone(id, &mut t).map_err(err)?;
                buf.extend(t);
                pos += PAGE as u64;
            }
            Item::Record(bytes) => {
                let e = record_entry(bytes, pos)?;
                if live.iter().any(|l| l.id == e.id) {
                    return Err(format!("{} is already in the pack", hex(&e.id)));
                }
                live.push(e);
                buf.extend_from_slice(bytes);
                pos += bytes.len() as u64;
            }
        }
    }
    live.sort_by_key(|e| e.offset);
    let tlen = pack::trailer_len(live.len()).map_err(err)?;
    if pos + tlen as u64 > MAX_PACK as u64 {
        return Err(format!("the pack would exceed {} bytes; start another pack", MAX_PACK));
    }
    let mut t = vec![0u8; tlen];
    pack::write_trailer(&live, pos, &mut t).map_err(err)?;
    f.seek(SeekFrom::Start(start)).map_err(io)?;
    f.write_all(&buf).map_err(io)?;
    f.sync_data().map_err(io)?;
    f.write_all(&t).map_err(io)?;
    f.sync_data().map_err(io)?;
    Ok(pos + tlen as u64)
}

/// Creates a new pack at `path` holding `items`, via a temporary file renamed into place.
pub fn create(path: &Path, items: &[Item]) -> Result<u64, String> {
    let what = path.display();
    let mut tmp = PathBuf::from(path);
    tmp.as_mut_os_string().push(".tmp");
    let mut f = File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut h = vec![0u8; PAGE];
    pack::write_pack_header(&mut h).map_err(err)?;
    f.write_all(&h).map_err(|e| format!("{what}: {e}"))?;
    let r = write_items(&mut f, PAGE as u64, Vec::new(), items).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{what}: {e}")
    })?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{what}: {e}"))?;
    sync_dir(path).map_err(|e| format!("{what}: {e}"))?;
    Ok(r)
}

/// Appends `items` to the pack at `path` with one trailer. A torn tail is cut back first.
pub fn append(path: &Path, items: &[Item]) -> Result<u64, String> {
    let what = path.display();
    let l = load(path)?;
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("{what}: {e}"))?;
    if l.dropped > 0 {
        f.set_len(l.data.len() as u64).map_err(|e| format!("{what}: {e}"))?;
        f.sync_data().map_err(|e| format!("{what}: {e}"))?;
        eprintln!("{what}: cut back to the last commit ({} bytes)", l.data.len());
    }
    let live: Vec<IndexEntry> = l.pack()?.entries().collect();
    write_items(&mut f, l.data.len() as u64, live, items).map_err(|e| format!("{what}: {e}"))
}

pub fn hex(id: &[u8]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}
