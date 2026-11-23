//! Inventory installed RPF7 names, including nested TOCs, without reading assets.
//! Usage: gta5_name_export GAME_DIR MAGIC_DAT OUTPUT
//! Every filesystem read <=1MiB; each combined TOC <=64MiB. No production changes.
use rage_crypto::{GtaVKeys, decode_codewalker_magic, scan_executable};
use std::{
    collections::HashSet,
    error::Error,
    fs::{self, File},
    io::{self, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const CHUNK: usize = 1 << 20;
const CAP: u64 = 64 << 20;
fn bad(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn word(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u24(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], 0])
}
fn read_at(file: &mut File, offset: u64, size: usize) -> Result<Vec<u8>> {
    if size as u64 > CAP
        || offset.checked_add(size as u64).is_none_or(|e| e > file.metadata().map(|m| m.len()).unwrap_or(0))
    {
        return Err(bad("bounded read exceeds file or cap").into());
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; size];
    for chunk in bytes.chunks_mut(CHUNK) {
        file.read_exact(chunk)?;
    }
    Ok(bytes)
}
fn keys(game: &Path, magic: &Path) -> Result<GtaVKeys> {
    let mut exe = File::open(game.join("GTA5_Enhanced.exe"))?;
    let size = exe.metadata()?.len();
    // 1024-byte overlap also preserves all windows considered by the scanner.
    // The step is 8-byte aligned, as are its fingerprint search positions.
    let mut at = 0;
    let aes = loop {
        if at >= size {
            return Err(bad("AES fingerprint absent from executable").into());
        }
        let bytes = read_at(&mut exe, at, (size - at).min(CHUNK as u64) as usize)?;
        let scan = scan_executable(&bytes);
        if let Some(aes) = scan.aes_key {
            break aes;
        }
        at += (CHUNK - 1024) as u64;
        if at % (8 * (CHUNK - 1024) as u64) == 0 {
            eprintln!("AES scan: {} MiB", at >> 20);
        }
    };
    eprintln!("AES fingerprint verified; key bytes are not logged");
    let mut file = File::open(magic)?;
    let size = file.metadata()?.len();
    if size == 0 || size > CHUNK as u64 {
        return Err(bad("magic.dat exceeds 1MiB cap").into());
    }
    let blob = read_at(&mut file, 0, size as usize)?;
    let material = decode_codewalker_magic(&blob, &aes)?;
    GtaVKeys::from_magic(aes, &material).ok_or_else(|| bad("invalid key blob layout").into())
}

struct Job {
    base: u64,
    size: u64,
    name: String,
    qualified: String,
    depth: u8,
}
#[derive(Default)]
struct Stats {
    archives: u64,
    entries: u64,
    errors: u64,
    metadata: u64,
}
fn toc(file: &mut File, job: &Job, keys: &GtaVKeys, output: &mut impl Write, stats: &mut Stats) -> Result<Vec<Job>> {
    if job.size < 16 || job.depth > 16 {
        return Err(bad("invalid nested archive window").into());
    }
    let h = read_at(file, job.base, 16)?;
    if word(&h, 0) != 0x52504637 {
        return Err(bad("not RPF7").into());
    }
    let n = word(&h, 4) as usize;
    let names_len = word(&h, 8) as usize;
    let len = 16 + (n as u64) * 16 + names_len as u64;
    if n == 0 || n > 1 << 20 || names_len as u64 > CAP || len > CAP || len > job.size {
        return Err(bad("TOC exceeds count/metadata/archive bounds").into());
    }
    let mut entries = read_at(file, job.base + 16, n * 16)?;
    let mut names = read_at(file, job.base + 16 + (n as u64) * 16, names_len)?;
    match word(&h, 12) {
        0 | 0x4e45504f => {}
        0x0ffffff9 => {
            keys.decrypt_aes(&mut entries);
            keys.decrypt_aes(&mut names);
        }
        _ => {
            keys.decrypt_ng(&mut entries, &job.name, job.size as u32);
            keys.decrypt_ng(&mut names, &job.name, job.size as u32);
        }
    }
    stats.metadata += len;
    if word(&entries, 4) != 0x7fffff00 {
        return Err(bad("decrypted root is not directory").into());
    }
    let mut seen = vec![false; n];
    seen[0] = true;
    let mut pending = vec![(0usize, String::new(), 0u16)];
    let mut children = Vec::new();
    while let Some((dir, parent, depth)) = pending.pop() {
        if depth > 256 {
            return Err(bad("directory depth cap").into());
        }
        let r = &entries[dir * 16..dir * 16 + 16];
        let first = word(r, 8) as usize;
        let count = word(r, 12) as usize;
        let end = first.checked_add(count).filter(|end| *end <= n).ok_or_else(|| bad("directory range"))?;
        if count > 0 && first == 0 {
            return Err(bad("root claimed as child").into());
        }
        for (i, claimed) in seen.iter_mut().enumerate().take(end).skip(first) {
            if *claimed {
                return Err(bad("shared/cyclic directory entry").into());
            }
            *claimed = true;
            let r = &entries[i * 16..i * 16 + 16];
            let second = word(r, 4);
            let directory = second == 0x7fffff00;
            let no = if directory { word(r, 0) as usize } else { u16::from_le_bytes([r[0], r[1]]) as usize };
            let tail = names.get(no..).ok_or_else(|| bad("name offset"))?;
            let end = tail
                .iter()
                .position(|b| *b == 0)
                .filter(|end| *end <= 4096)
                .ok_or_else(|| bad("unterminated/long name"))?;
            let name = std::str::from_utf8(&tail[..end])?;
            if name.is_empty() || name.chars().any(char::is_control) || name.contains(['/', '\\']) {
                return Err(bad("invalid path component").into());
            }
            let path = if parent.is_empty() { name.to_owned() } else { format!("{parent}/{name}") };
            if path.len() > 4096 {
                return Err(bad("path length cap").into());
            }
            if directory {
                pending.push((i, path, depth + 1));
                continue;
            }
            stats.entries += 1;
            writeln!(output, "{path}")?;
            writeln!(output, "{name}")?;
            writeln!(output, "{}/{path}", job.qualified)?;
            if !name.to_ascii_lowercase().ends_with(".rpf") {
                continue;
            }
            if second & 0x80000000 != 0 || u24(r, 2) != 0 || word(r, 12) != 0 {
                return Err(bad("nested RPF is compressed/encrypted/resource; payload reads forbidden").into());
            }
            let offset = u64::from(u24(r, 5)) * 512;
            let size = u64::from(word(r, 8));
            if offset < len || size < 16 || offset.checked_add(size).is_none_or(|end| end > job.size) {
                return Err(bad("nested RPF outside parent/data window").into());
            }
            children.push(Job {
                base: job.base + offset,
                size,
                name: name.into(),
                qualified: format!("{}/{path}", job.qualified),
                depth: job.depth + 1,
            });
        }
    }
    if seen.iter().any(|v| !*v) {
        return Err(bad("unreachable TOC entries").into());
    }
    stats.archives += 1;
    Ok(children)
}
fn roots(dir: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for item in fs::read_dir(dir)? {
        let item = item?;
        let kind = item.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            roots(&item.path(), output)?;
        } else if item.path().extension().is_some_and(|s| s.eq_ignore_ascii_case("rpf")) {
            output.push(item.path());
        }
    }
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err(bad("usage: gta5_name_export GAME_DIR MAGIC_DAT OUTPUT").into());
    }
    let game = Path::new(&args[0]);
    let dest = Path::new(&args[2]);
    let keys = keys(game, Path::new(&args[1]))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let spool = dest.with_extension("unsorted.tmp");
    let mut output = BufWriter::new(File::create(&spool)?);
    let mut errors = BufWriter::new(File::create(dest.with_extension("errors.log"))?);
    let mut files = Vec::new();
    roots(game, &mut files)?;
    files.sort();
    let mut stats = Stats::default();
    for path in files {
        let mut file = File::open(&path)?;
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let qualified = path.strip_prefix(game)?.to_string_lossy().replace('\\', "/");
        let mut pending = vec![Job { base: 0, size: file.metadata()?.len(), name, qualified, depth: 0 }];
        let mut seen = HashSet::new();
        while let Some(job) = pending.pop() {
            if !seen.insert((job.base, job.size)) {
                continue;
            }
            match toc(&mut file, &job, &keys, &mut output, &mut stats) {
                Ok(children) => pending.extend(children),
                Err(e) => {
                    stats.errors += 1;
                    writeln!(errors, "{}: {e}", job.qualified)?;
                }
            }
            if stats.archives > 0 && stats.archives % 100 == 0 {
                eprintln!("{} archives, {} files, {} errors", stats.archives, stats.entries, stats.errors);
            }
        }
    }
    output.flush()?;
    errors.flush()?;
    drop(output);
    let status =
        Command::new("sort").env("LC_ALL", "C").args(["-u", "-S", "64M", "-o"]).arg(dest).arg(&spool).status()?;
    if !status.success() {
        return Err(bad("external bounded-memory dictionary sort failed").into());
    }
    fs::remove_file(spool)?;
    eprintln!(
        "DONE archives={} files={} errors={} metadata_bytes={}",
        stats.archives, stats.entries, stats.errors, stats.metadata
    );
    if stats.errors != 0 {
        return Err(bad("partial inventory: see errors.log").into());
    }
    Ok(())
}
