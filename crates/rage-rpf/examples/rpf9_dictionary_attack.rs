//! CPU-only extension dictionary comparison. No archive access.
//! Usage: rpf9_dictionary_attack EXPORT_DIR CANDIDATE_LIST...
//! Reads bounded lines; sorted target vectors, disk dedupe, one hash worker.
use rage_rpf::rpf9::name_hash;
use std::{
    collections::HashSet,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MIB: usize = 1 << 20;
fn bad(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn lines(path: &Path, mut f: impl FnMut(&str) -> Result<()>) -> Result<()> {
    let mut r = BufReader::with_capacity(64 * 1024, File::open(path)?);
    let mut line = Vec::new();
    loop {
        let b = r.fill_buf()?;
        if b.is_empty() {
            break;
        }
        let n = b.iter().position(|v| *v == b'\n').map_or(b.len(), |i| i + 1);
        if line.len() + n > 65536 {
            return Err(bad("line exceeds 64KiB").into());
        }
        line.extend_from_slice(&b[..n]);
        r.consume(n);
        if line.last() == Some(&b'\n') {
            f(std::str::from_utf8(&line)?.trim_end_matches(['\r', '\n']))?;
            line.clear();
        }
    }
    if !line.is_empty() {
        f(std::str::from_utf8(&line)?)?
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_hash_vectors() {
        assert_eq!(hash32("ADDER"), 0xb779a091);
        assert_eq!(hash32("\"adder\"unused"), 0xb779a091);
        assert_eq!(hash32("a\\b\0ignored"), hash32("A/B"));
        assert_eq!(name_hash("DATA\\graphics/graphics_definitions.meta"), 0x5c33030f6402b9dd);
        assert_eq!(name_hash(&"a".repeat(256)), 0x42bffdb35fe459a8);
    }

    #[test]
    fn nested_paths_and_compound_suffixes() {
        let v = variants("COMMON:/outer.rpf/inside.rpf/x64/audio/example.dat151.rel");
        assert!(v.iter().any(|s| s == "audio/example.dat151.rel"));
        assert!(v.iter().any(|s| s == "audio/example"));
        assert!(v.iter().any(|s| s == "example"));
        assert!(variants("// a comment").is_empty());
    }
}
fn create(p: &Path) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(OpenOptions::new().write(true).create_new(true).open(p)?))
}
fn hash32(s: &str) -> u32 {
    let data = if let Some(s) = s.strip_prefix('"') { s.split('"').next().unwrap_or("") } else { s };
    let mut h = 0u32;
    for b in data.bytes().take_while(|b| *b != 0) {
        let b = if b == b'\\' { b'/' } else { b.to_ascii_lowercase() };
        h = h.wrapping_add(b as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}
fn variants(line: &str) -> Vec<String> {
    let p = line
        .trim()
        .trim_start_matches('\u{feff}')
        .split('=')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase()
        .replace('\\', "/");
    if p.is_empty()
        || p.len() > 1024
        || p.starts_with("//")
        || p.starts_with('#')
        || p.starts_with('[')
        || !p.is_ascii()
        || p.bytes().any(|c| !(c.is_ascii_alphanumeric() || b"_./:-@+ #".contains(&c)))
    {
        return vec![];
    }
    let after = p.split_once(":/").map_or(p.as_str(), |(_, s)| s).trim_start_matches('/');
    let inner = after.rsplit_once(".rpf/").map_or(after, |(_, s)| s);
    let x = inner.strip_prefix("x64/").unwrap_or(inner);
    let base = x.rsplit('/').next().unwrap_or(x);
    let mut v = vec![p.clone(), after.into(), inner.into(), x.into(), base.into()];
    for s in v.clone() {
        if let Some((stem, ext)) = s.rsplit_once('.') {
            if !ext.contains('/') {
                v.push(stem.into());
            }
        }
        // Compound .dat151.rel and friends: also permit replacing the whole suffix.
        if let Some((stem, ext)) = s.rsplit_once(".dat") {
            if ext.ends_with(".rel") {
                v.push(stem.into());
            }
        }
    }
    v.retain(|s| !s.is_empty());
    v.sort();
    v.dedup();
    v
}
struct Targets {
    values: Vec<u64>,
    bloom: Vec<u64>,
}
impl Targets {
    fn positions(x: u64) -> [usize; 2] {
        let y = x.wrapping_mul(0x9e3779b97f4a7c15);
        [(y ^ (y >> 32)) as usize & ((1 << 24) - 1), (y.rotate_left(23) ^ (x >> 19)) as usize & ((1 << 24) - 1)]
    }
    fn load(p: &Path) -> Result<Self> {
        let mut values = Vec::new();
        lines(p, |s| {
            if !s.is_empty() {
                values.push(u64::from_str_radix(s, 16)?);
                if values.len() > 2_000_000 {
                    return Err(bad("target count cap").into());
                }
            }
            Ok(())
        })?;
        values.sort_unstable();
        values.dedup();
        let mut bloom = vec![0u64; 1 << 18];
        for &v in &values {
            for bit in Self::positions(v) {
                bloom[bit / 64] |= 1 << (bit % 64);
            }
        }
        Ok(Self { values, bloom })
    }
    fn has(&self, x: u64) -> bool {
        Self::positions(x).into_iter().all(|b| self.bloom[b / 64] & (1 << (b % 64)) != 0)
            && self.values.binary_search(&x).is_ok()
    }
}
struct Hit {
    h64: u64,
    h32: u32,
    m64: bool,
    m32: bool,
    candidate: String,
    source: usize,
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() < 2 {
        return Err(bad("usage: rpf9_dictionary_attack EXPORT_DIR CANDIDATE_LIST...").into());
    }
    let root = Path::new(&args[0]);
    let sources: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();
    let prefix = root.join("cpu_extensions");
    fs::create_dir(&prefix)?;
    let mut sourceout = create(&prefix.join("sources.tsv"))?;
    writeln!(sourceout, "source_id\tpath")?;
    let spool = prefix.join("variants.unsorted.tsv");
    let sorted = prefix.join("variants.sorted.tsv");
    let mut out = create(&spool)?;
    let mut input_lines = 0u64;
    for (id, path) in sources.iter().enumerate() {
        writeln!(sourceout, "{id}\t{}", path.display())?;
        let mut n = 0;
        lines(path, |s| {
            n += 1;
            for v in variants(s) {
                writeln!(out, "{v}\t{id}")?
            }
            Ok(())
        })?;
        input_lines += n;
        eprintln!("prepared source {id}: {n} lines");
    }
    out.flush()?;
    drop(out);
    sourceout.flush()?;
    if !Command::new("sort")
        .env("LC_ALL", "C")
        .args(["-u", "--parallel=1", "-S", "32M", "-T"])
        .arg(&prefix)
        .arg("-o")
        .arg(&sorted)
        .arg(&spool)
        .status()?
        .success()
    {
        return Err(bad("disk dedupe failed").into());
    }
    fs::remove_file(spool)?;
    let targets64 = Targets::load(&root.join("name_hashes_unique.txt"))?;
    let targets32 = Targets::load(&root.join("name_hashes32_unique.txt"))?;
    // Freeze prior verified entry coverage. Never treat 32-only rows as baseline names.
    let mut baseline = HashSet::new();
    let mut baseline_bytes = 0usize;
    let mut baselines = create(&prefix.join("baseline_sources.txt"))?;
    for item in fs::read_dir(root)? {
        let p = item?.path();
        let n = p.file_name().unwrap().to_string_lossy();
        if !n.ends_with("matches.tsv") {
            continue;
        }
        let mut header: Vec<String> = vec![];
        let mut accepted = false;
        lines(&p, |s| {
            if header.is_empty() {
                header = s.split('\t').map(str::to_owned).collect();
                accepted =
                    header.first().is_some_and(|h| h == "archive") && header.get(1).is_some_and(|h| h == "index");
                return Ok(());
            }
            if !accepted {
                return Ok(());
            }
            let f: Vec<_> = s.split('\t').collect();
            if let Some(i) = header.iter().position(|h| h == "evidence") {
                if f.get(i).is_some_and(|s| s.starts_with("32")) {
                    return Ok(());
                }
            }
            if f.len() > 2 {
                let key = format!("{}\t{}", f[0], f[1]);
                if baseline.insert(key.clone()) {
                    baseline_bytes += key.len() + 64;
                    if baseline_bytes > 16 * MIB {
                        return Err(bad("baseline memory cap").into());
                    }
                }
            }
            Ok(())
        })?;
        if accepted {
            writeln!(baselines, "{}", p.display())?;
        }
    }
    baselines.flush()?;
    let mut extensions:Vec<String>="meta xml dat txt rpf ymt ytd ydr ydd yft ybn ycd ytyp ymap ysc awc rel gfx dat151.rel dat54.rel dat10.rel dat15.rel dat16.rel dat22.rel dat149.rel dat150.rel dat151 dat54 dat10 cut pso psin rbf rbf0 yld ypt yvr yed yfd ymap ytyp ycd yclip ynv ynm yhn ycs ymf ybs ysd ypd ybd ydatb ybdr ybdd ybft yvert dds png json ini cfg bin".split_whitespace().map(|s|format!(".{s}")).collect();
    for pre in ["z", "i", "#"] {
        for suffix in [
            "mf", "dr", "ft", "dd", "td", "bn", "bd", "pd", "bs", "sd", "mt", "sc", "cs", "datb", "bdr", "bdd", "bft",
            "vert",
        ] {
            extensions.push(format!(".{pre}{suffix}"));
        }
    }
    extensions.sort();
    extensions.dedup();
    extensions.insert(0, String::new());
    let mut exts = create(&prefix.join("extensions.txt"))?;
    for e in &extensions {
        writeln!(exts, "{e}")?
    }
    exts.flush()?;
    let mut matches = create(&prefix.join("matches.tsv"))?;
    writeln!(matches, "hash64\thash32\tmatched64\tmatched32\tcandidate\tsource_id")?;
    let mut hits = Vec::new();
    let mut hit_bytes = 0usize;
    let mut trials = 0u64;
    let mut bases = 0u64;
    lines(&sorted, |s| {
        let (base, source) = s.rsplit_once('\t').ok_or_else(|| bad("invalid spool"))?;
        let source: usize = source.parse()?;
        bases += 1;
        let mut candidate = String::with_capacity(base.len() + 24);
        for ext in &extensions {
            candidate.clear();
            candidate.push_str(base);
            candidate.push_str(ext);
            let h64 = name_hash(&candidate);
            let h32 = hash32(&candidate);
            trials += 1;
            let m64 = targets64.has(h64);
            let m32 = targets32.has(u64::from(h32));
            if m64 || m32 {
                hit_bytes += candidate.len() + 64;
                if hit_bytes > 32 * MIB {
                    return Err(bad("match memory cap; partial evidence retained").into());
                }
                writeln!(matches, "{h64:016x}\t{h32:08x}\t{m64}\t{m32}\t{candidate}\t{source}")?;
                hits.push(Hit { h64, h32, m64, m32, candidate: candidate.clone(), source });
            }
        }
        if bases % 100_000 == 0 {
            eprintln!("bases={bases} trials={trials} hits={}", hits.len());
        }
        Ok(())
    })?;
    matches.flush()?;
    // Match records are bounded; sort indexes, not copies of candidate strings.
    let mut by64: Vec<_> = (0..hits.len()).filter(|i| hits[*i].m64).collect();
    by64.sort_unstable_by_key(|i| hits[*i].h64);
    let mut by32: Vec<_> = (0..hits.len()).filter(|i| hits[*i].m32).collect();
    by32.sort_unstable_by_key(|i| hits[*i].h32);
    let mut rows = create(&prefix.join("entry_matches.tsv"))?;
    writeln!(rows, "archive\tindex\thash64\thash32\tevidence\tcandidate\tsource_id\tnew_verified_entry")?;
    let (mut entries64, mut entriesboth, mut only32, mut new64) = (0u64, 0u64, 0u64, 0u64);
    let mut first = true;
    lines(&root.join("entries.tsv"), |s| {
        if first {
            first = false;
            return Ok(());
        }
        let f: Vec<_> = s.split('\t').collect();
        if f.len() < 4 {
            return Err(bad("entry row fields").into());
        }
        let h64 = u64::from_str_radix(f[2], 16).ok();
        let h32 = u32::from_str_radix(f[3], 16).ok();
        let mut found64 = false;
        let mut both = false;
        let mut found32 = false;
        let key = format!("{}\t{}", f[0], f[1]);
        let new = !baseline.contains(&key);
        if let Some(h) = h64 {
            let start = by64.partition_point(|i| hits[*i].h64 < h);
            for &i in by64[start..].iter().take_while(|i| hits[**i].h64 == h) {
                let m = &hits[i];
                let confirmed = h32 == Some(m.h32);
                found64 = true;
                both |= confirmed;
                writeln!(
                    rows,
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{new}",
                    f[0],
                    f[1],
                    f[2],
                    f[3],
                    if confirmed { "64+32" } else { "64" },
                    m.candidate,
                    m.source
                )?;
            }
        }
        if let Some(h) = h32 {
            let start = by32.partition_point(|i| hits[*i].h32 < h);
            for &i in by32[start..].iter().take_while(|i| hits[**i].h32 == h) {
                let m = &hits[i];
                if h64 == Some(m.h64) {
                    continue;
                }
                found32 = true;
                writeln!(
                    rows,
                    "{}\t{}\t{}\t{}\t32-only-unverified\t{}\t{}\tfalse",
                    f[0], f[1], f[2], f[3], m.candidate, m.source
                )?;
            }
        }
        entries64 += u64::from(found64);
        entriesboth += u64::from(both);
        only32 += u64::from(found32 && !found64);
        new64 += u64::from(found64 && new);
        Ok(())
    })?;
    rows.flush()?;
    let unique64 = by64.iter().map(|i| hits[*i].h64).collect::<HashSet<_>>().len();
    let unique32 = by32.iter().map(|i| hits[*i].h32).collect::<HashSet<_>>().len();
    let summary = format!(
        "{{\"input_lines\":{input_lines},\"base_source_pairs\":{bases},\"extensions_including_empty\":{},\"trials\":{trials},\"matched_hashes64\":{unique64},\"matched_hashes32\":{unique32},\"entries64\":{entries64},\"entries_both\":{entriesboth},\"entries32_only_unverified\":{only32},\"new_entries64\":{new64},\"baseline_verified_entries\":{}}}",
        extensions.len(),
        baseline.len()
    );
    writeln!(create(&prefix.join("summary.json"))?, "{summary}")?;
    eprintln!("{summary}");
    Ok(())
}
