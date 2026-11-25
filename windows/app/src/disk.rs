//! The core's file work (`zcore::screen::DiskOp`), done on this PC: a path is under the app's folder, except
//! `downloads/…` (the Downloads folder) and `manhwa/…` (the chapters' folders inside it).
use crate::net;
use std::path::{Path, PathBuf};
use zcore::screen::{DiskEntry, DiskOp, DiskReply};

/// Where a path the core names is on this PC.
pub fn path(name: &str) -> PathBuf {
    if let Some(rest) = name.strip_prefix("downloads/") {
        net::downloads_dir().join(rest)
    } else if let Some(rest) = name.strip_prefix("manhwa/") {
        net::downloads_dir().join("manhwa").join(rest)
    } else if name == "manhwa" {
        net::downloads_dir().join("manhwa")
    } else {
        net::root().join(name)
    }
}

fn size(p: &Path) -> u64 {
    p.metadata().map(|m| m.len()).unwrap_or(0)
}

fn done(result: std::io::Result<DiskReply>) -> DiskReply {
    result.unwrap_or_else(|e| DiskReply { ok: false, message: e.to_string(), ..Default::default() })
}

/// One of the core's file jobs, done (it blocks: run it off the window's thread).
pub fn run(op: DiskOp) -> DiskReply {
    let ok = DiskReply { ok: true, ..Default::default() };
    match op {
        DiskOp::Read { path: p } => DiskReply { text: std::fs::read_to_string(path(&p)).ok(), ..ok },
        DiskOp::Write { path: p, text } => {
            let to = path(&p);
            if let Some(dir) = to.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            done(net::atomically(&to, &text).map(|()| ok))
        }
        DiskOp::Append { from, to, at } => done((|| {
            use std::io::{Seek, SeekFrom, Write};
            let (from, to) = (path(&from), path(&to));
            let add = std::fs::read(&from)?;
            // (opened to write, not to append: Windows won't change the length of a file opened only to append)
            let mut out = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&to)?;
            out.set_len(at)?;
            out.seek(SeekFrom::End(0))?;
            out.write_all(&add)?;
            out.flush()?;
            drop(out);
            let _ = std::fs::remove_file(&from);
            Ok(DiskReply { bytes: size(&to), ..ok.clone() })
        })()),
        DiskOp::Rename { from, to } => done(std::fs::rename(path(&from), path(&to)).map(|()| ok)),
        DiskOp::Remove { paths } => {
            for p in paths {
                let p = path(&p);
                let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
            ok
        }
        DiskOp::Listing { path: p } => {
            let entries = std::fs::read_dir(path(&p))
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| {
                            let m = e.metadata().ok()?;
                            m.is_file().then(|| DiskEntry { name: e.file_name().to_string_lossy().into_owned(), bytes: m.len() })
                        })
                        .collect()
                })
                .unwrap_or_default();
            DiskReply { entries, ..ok }
        }
        DiskOp::Sizes { paths } => DiskReply { entries: paths.into_iter().map(|p| DiskEntry { bytes: size(&path(&p)), name: p }).collect(), ..ok },
        DiskOp::Picture { path: p } => DiskReply { ok: std::fs::read(path(&p)).ok().and_then(|b| crate::images::dimensions(&b)).is_some(), ..Default::default() },
        DiskOp::Free => DiskReply { bytes: free().unwrap_or(0), ..ok },
        DiskOp::Hash { path: p } => done(sha256_hex(&path(&p)).map(|hex| DiskReply { text: Some(hex), ..ok })),
    }
}

/// A file's SHA-256 as lowercase hex, read a block at a time.
fn sha256_hex(file: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut input = std::fs::File::open(file)?;
    let (mut hash, mut block) = (Sha256::new(), vec![0u8; 256 * 1024]);
    loop {
        let got = input.read(&mut block)?;
        if got == 0 {
            break;
        }
        hash.update(&block[..got]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The bytes free where downloads are kept.
fn free() -> Option<u64> {
    crate::platform::free_bytes(&net::downloads_dir())
}
