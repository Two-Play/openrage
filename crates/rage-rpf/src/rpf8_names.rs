//! Names for RDR2 archive entries.
//!
//! RPF8 entries carry only a hash of their path, and the name tables of the
//! encrypted archives cannot be read. OpenRAGE finds names in the game's own
//! files instead: every path-like string in every file (text, metadata and
//! resources alike) is tried, in a few forms, against the hashes of all
//! entries. Archives at the top level hash paths (`data/ai/…/x.meta`, after a
//! mount prefix such as `common:/`); files inside nested archives hash their
//! bare name without extension (`p_chair01x`).
//!
//! The hits are kept in a small cache, so the search (minutes over the whole
//! game) runs once.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_core::hash::joaat;

use crate::rpf8::{Entry, Rdr2Keys, Rpf8, extension};

const CACHE_HEADER: &str = "openrage-rdr2-names 8";
/// File count recorded for names from an imported list: always trusted.
const IMPORTED: u32 = u32::MAX;

/// Entry names by hash. The name is what the hash was computed from: a full
/// path with extension when the entry stores no extension id, otherwise the
/// path or bare name without extension.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameIndex {
    pub names: HashMap<u32, String>,
    /// In how many of the game's files each name was found ([`IMPORTED`] for listed names).
    pub files: HashMap<u32, u32>,
    /// Names judged to be chance matches (see [`NameIndex::judge`]).
    untrusted: HashSet<u32>,
}

impl NameIndex {
    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = self
            .names
            .iter()
            .map(|(h, n)| format!("{h:08x}\t{n}\t{}", self.files.get(h).copied().unwrap_or(1)))
            .collect();
        lines.sort_by(|a, b| a[9..].cmp(&b[9..]));
        format!("{CACHE_HEADER}\n{}\n", lines.join("\n"))
    }

    pub fn from_text(text: &str) -> Option<NameIndex> {
        let mut lines = text.lines();
        if lines.next()? != CACHE_HEADER {
            return None;
        }
        let mut index = NameIndex::default();
        for l in lines {
            let mut f = l.split('\t');
            let (Some(h), Some(n)) = (f.next(), f.next()) else { continue };
            let Ok(h) = u32::from_str_radix(h, 16) else { continue };
            index.names.insert(h, n.to_owned());
            index.files.insert(h, f.next().and_then(|c| c.parse().ok()).unwrap_or(1));
        }
        index.judge();
        Some(index)
    }

    pub fn load(path: &Path) -> std::io::Result<Option<NameIndex>> {
        match std::fs::read_to_string(path) {
            Ok(t) => Ok(NameIndex::from_text(&t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_text())
    }

    /// Adds names from a plain list (one path or name per line), keeping those
    /// that match one of `wanted`. Returns how many were new.
    pub fn add_list(&mut self, text: &str, wanted: &HashSet<u32>) -> usize {
        let before = self.names.len();
        for line in text.lines() {
            for c in candidates(line.trim()) {
                let h = joaat(&c);
                if wanted.contains(&h) {
                    self.names.entry(h).or_insert(c);
                    self.files.insert(h, IMPORTED);
                }
            }
        }
        self.judge();
        self.names.len() - before
    }

    /// Whether the name for a hash is trusted (see [`NameIndex::judge`]).
    pub fn trusted(&self, hash: u32) -> bool {
        self.names.contains_key(&hash) && !self.untrusted.contains(&hash)
    }

    /// Sorts out chance matches. With ~60 million strings tried against
    /// ~350,000 hashes, a few thousand random letter runs match by chance.
    /// Paths (`/`) and names with `_` are reliable; a bare name without `_`
    /// must read like the reliable ones: a letter-bigram model trained on
    /// them scores its letter segments (3+ letters), and names that score
    /// below −3.6 per bigram (`9xetp`, `zzwwuwwwzz`) or have no such segment
    /// are dropped. Imported names are always kept.
    pub fn judge(&mut self) {
        let reliable = |n: &str| n.contains('/') || n.contains('_');
        let mut bigrams: HashMap<(u8, u8), f64> = HashMap::new();
        let mut firsts: HashMap<u8, f64> = HashMap::new();
        let segments = |n: &str| -> Vec<Vec<u8>> {
            n.split(|c: char| !c.is_ascii_lowercase()).filter(|s| s.len() >= 3).map(|s| s.bytes().collect()).collect()
        };
        for n in self.names.values().filter(|n| reliable(n)) {
            for seg in n.split(|c: char| !c.is_ascii_lowercase()).filter(|s| !s.is_empty()) {
                let w: Vec<u8> = std::iter::once(b'^').chain(seg.bytes()).chain(std::iter::once(b'$')).collect();
                for p in w.windows(2) {
                    *bigrams.entry((p[0], p[1])).or_default() += 1.0;
                    *firsts.entry(p[0]).or_default() += 1.0;
                }
            }
        }
        let score = |n: &str| -> f64 {
            let segs = segments(n);
            if segs.is_empty() {
                return f64::NEG_INFINITY;
            }
            let (mut total, mut count) = (0.0, 0.0);
            for seg in segs {
                let w: Vec<u8> = std::iter::once(b'^').chain(seg).chain(std::iter::once(b'$')).collect();
                for p in w.windows(2) {
                    let pair = bigrams.get(&(p[0], p[1])).copied().unwrap_or(0.0);
                    let first = firsts.get(&p[0]).copied().unwrap_or(0.0);
                    total += ((pair + 0.1) / (first + 2.7)).ln();
                    count += 1.0;
                }
            }
            total / count
        };
        self.untrusted = self
            .names
            .iter()
            .filter(|(h, n)| {
                self.files.get(h) != Some(&IMPORTED)
                    && (!plausible(n.split_once(":/").map_or(n, |x| x.1)) || (!reliable(n) && score(n) < -3.6))
            })
            .map(|(h, _)| *h)
            .collect();
    }

    /// How many trusted names there are.
    pub fn trusted_count(&self) -> usize {
        self.names.keys().filter(|h| self.trusted(**h)).count()
    }

    /// The display name of an entry, if its hash is known.
    pub fn name_for(&self, e: &Entry, platform: char) -> Option<String> {
        if !self.trusted(e.hash) {
            return None;
        }
        let n = self.names.get(&e.hash)?;
        Some(match extension(e.ext_id, platform) {
            Some(ext) if e.ext_id != 0xFF => format!("{n}.{ext}"),
            _ => n.clone(),
        })
    }
}

/// Whether a candidate looks like a real name rather than a run of binary
/// data that happens to hash to an entry: with ~60 million strings tried
/// against ~350,000 32-bit hashes, a few thousand chance matches are expected.
/// Every path part must use only `a-z 0-9 _ . -`, contain a vowel or digit,
/// and have no run of more than five consonants.
fn plausible(name: &str) -> bool {
    name.split('/').all(|part| {
        let ok_chars = !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
            && part.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_');
        let vowel_or_digit = part.bytes().any(|b| b"aeiouy".contains(&b) || b.is_ascii_digit());
        let mut run = 0;
        let mut max_run = 0;
        for b in part.bytes() {
            if b.is_ascii_lowercase() && !b"aeiouy".contains(&b) {
                run += 1;
                max_run = max_run.max(run);
            } else {
                run = 0;
            }
        }
        ok_chars && vowel_or_digit && max_run <= 5
    })
}

/// The forms of a string that entry hashes are made from.
fn candidates(s: &str) -> Vec<String> {
    let s = s.to_ascii_lowercase().replace('\\', "/");
    let mut out = Vec::with_capacity(6);
    let after = s.split_once(":/").map_or(s.as_str(), |x| x.1);
    let no_x64 = after.strip_prefix("x64/").unwrap_or(after);
    let stem = |x: &str| x.rsplit_once('.').map_or(x, |y| y.0).to_owned();
    let base = no_x64.rsplit('/').next().unwrap_or(no_x64);
    for c in [s.clone(), after.to_owned(), no_x64.to_owned(), stem(no_x64), base.to_owned(), stem(base)] {
        // The full string keeps its mount prefix (`common:/…`); everything else must look like a name.
        let check = c.split_once(":/").map_or(c.as_str(), |x| x.1);
        if !c.is_empty() && !out.contains(&c) && plausible(check) {
            out.push(c);
        }
    }
    out
}

/// Calls `f` with every path-like string in `data` (letters, digits and `_/\\.:-`, 3–199 bytes).
pub fn strings_in(data: &[u8], mut f: impl FnMut(&str)) {
    let mut start = None;
    for (i, &b) in data.iter().chain(std::iter::once(&0u8)).enumerate() {
        let ok = b.is_ascii_alphanumeric() || b"_/\\.:-".contains(&b);
        match (ok, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if (3..200).contains(&(i - s))
                    && let Ok(text) = std::str::from_utf8(&data[s..i])
                {
                    f(text);
                }
                start = None;
            }
            _ => {}
        }
    }
}

/// Every entry hash in the archives and those nested in them.
pub fn entry_hashes(files: &[PathBuf], keys: &Arc<Rdr2Keys>) -> HashSet<u32> {
    use rayon::prelude::*;
    fn walk(r: &Rpf8, out: &mut HashSet<u32>) {
        let Some(es) = r.entries() else { return };
        for e in es {
            out.insert(e.hash);
            if e.ext_id == 0
                && let Ok(c) = r.open_child(e)
            {
                walk(&c, out);
            }
        }
    }
    files
        .par_iter()
        .map(|p| {
            let mut out = HashSet::new();
            if let Ok(r) = Rpf8::open_with_keys(p, Some(keys.clone())) {
                walk(&r, &mut out);
            }
            out
        })
        .reduce(HashSet::new, |mut a, b| {
            a.extend(b);
            a
        })
}

/// Searches every file below 16 MiB for names of the `wanted` hashes.
/// `progress` is called after each archive file with (done, total).
///
/// Resources are searched in their virtual pages only.
pub fn build(
    files: &[PathBuf],
    keys: &Arc<Rdr2Keys>,
    wanted: &HashSet<u32>,
    progress: impl Fn(usize, usize) + Sync,
) -> NameIndex {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    /// Hits by hash: the name and in how many files it was found.
    type Hits = HashMap<u32, (String, u32)>;
    fn walk(r: &Rpf8, wanted: &HashSet<u32>, out: &mut Hits) {
        let Some(es) = r.entries() else { return };
        for e in es {
            if e.ext_id == 0 {
                if let Ok(c) = r.open_child(e) {
                    walk(&c, wanted, out);
                }
            } else if e.size() < 16 << 20
                && let Ok(data) = r.read(e)
            {
                // A resource's physical pages hold pixel and vertex data, whose
                // random letter runs only produce chance matches; names live in
                // the virtual pages (the structures).
                let virtual_len = if e.is_resource { (e.size_or_flags & 0xFFFF_FFF0) as usize } else { data.len() };
                let mut in_file: HashMap<u32, String> = HashMap::new();
                strings_in(&data[..virtual_len.min(data.len())], |s| {
                    for c in candidates(s) {
                        let h = joaat(&c);
                        if wanted.contains(&h) {
                            in_file.entry(h).or_insert(c);
                        }
                    }
                });
                for (h, name) in in_file {
                    out.entry(h).or_insert((name, 0)).1 += 1;
                }
            }
        }
    }
    let done = AtomicUsize::new(0);
    let hits: Hits = files
        .par_iter()
        .map(|p| {
            let mut out = Hits::new();
            if let Ok(r) = Rpf8::open_with_keys(p, Some(keys.clone())) {
                walk(&r, wanted, &mut out);
            }
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, files.len());
            out
        })
        .reduce(Hits::new, |mut a, b| {
            for (h, (n, count)) in b {
                a.entry(h).or_insert((n, 0)).1 += count;
            }
            a
        });
    let mut index = NameIndex::default();
    for (h, (name, count)) in hits {
        if plausible(name.split_once(":/").map_or(&name, |x| x.1)) {
            index.names.insert(h, name);
            index.files.insert(h, count);
        }
    }
    index.judge();
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_binary_noise() {
        assert!(plausible("p_chair01x"));
        assert!(plausible("data/levels/rdr3/fallen_snow/texturegridinfo.meta"));
        assert!(plausible("ambientscanner"));
        assert!(!plausible("uyrzvysxvpuwkaowxcrtvfsstlputkmyyjl"));
        assert!(!plausible("wvsxwn"));
        assert!(!plausible(":-v_"));
        assert!(!plausible("data//x"));
        // The bigram judge: trained on the reliable names, it keeps word-like bare names.
        let mut idx = NameIndex::default();
        for (i, n) in
            ["data/ai/horse_walk", "p_chair_cow", "wagon_supply_army", "duck_hunting_camp", "cow", "zzwwuwwwzz", "9x"]
                .into_iter()
                .enumerate()
        {
            idx.names.insert(i as u32, n.to_owned());
        }
        idx.judge();
        assert!(idx.trusted(0) && idx.trusted(1) && idx.trusted(4));
        assert!(!idx.trusted(5) && !idx.trusted(6));
    }

    #[test]
    fn candidate_forms() {
        let c = candidates("common:/data/AI/Missions/mud5.meta");
        assert!(c.contains(&"common:/data/ai/missions/mud5.meta".to_owned()));
        assert!(c.contains(&"data/ai/missions/mud5.meta".to_owned()));
        assert!(c.contains(&"data/ai/missions/mud5".to_owned()));
        assert!(c.contains(&"mud5".to_owned()));
        assert_eq!(candidates("x64\\levels\\p_chair01x.ydr").last().map(String::as_str), Some("p_chair01x"));
    }

    #[test]
    fn strings_and_cache() {
        let mut found = Vec::new();
        strings_in(b"\x00p_chair01x\x00ab\x00<platform:/levels/rdr3>\x07elk", |s| found.push(s.to_owned()));
        assert_eq!(found, ["p_chair01x", "platform:/levels/rdr3", "elk"]);

        let wanted: HashSet<u32> = [joaat("p_chair01x"), joaat("data/x.meta")].into();
        let mut idx = NameIndex::default();
        assert_eq!(idx.add_list("P_Chair01x.ydr\ncommon:/data/x.meta\nnope\n", &wanted), 2);
        let back = NameIndex::from_text(&idx.to_text()).unwrap();
        assert_eq!(back, idx);
        let e = Entry {
            hash: joaat("p_chair01x"),
            ext_id: 2,
            key_id: 0xFF,
            encryption_config: 0,
            is_resource: true,
            signed: false,
            offset: 0,
            size_on_disk: 0,
            compression: crate::rpf8::Compression::None,
            size_or_flags: 0,
            path: None,
        };
        assert_eq!(idx.name_for(&e, 'y').as_deref(), Some("p_chair01x.ydr"));
    }
}
