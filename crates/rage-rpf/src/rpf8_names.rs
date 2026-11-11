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

const CACHE_HEADER: &str = "openrage-rdr2-names 1";

/// Entry names by hash. The name is what the hash was computed from: a full
/// path with extension when the entry stores no extension id, otherwise the
/// path or bare name without extension.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameIndex {
    pub names: HashMap<u32, String>,
}

impl NameIndex {
    /// Where the names for a game folder are cached.
    pub fn default_path(game: &Path) -> Option<PathBuf> {
        let canonical = game.canonicalize().unwrap_or_else(|_| game.to_path_buf());
        let id = rage_core::hash::jenkins_bytes(canonical.to_string_lossy().as_bytes());
        Some(dirs::cache_dir()?.join("openrage").join(format!("rdr2-names-{id:08x}.tsv")))
    }

    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = self.names.iter().map(|(h, n)| format!("{h:08x}\t{n}")).collect();
        lines.sort_by(|a, b| a[9..].cmp(&b[9..]));
        format!("{CACHE_HEADER}\n{}\n", lines.join("\n"))
    }

    pub fn from_text(text: &str) -> Option<NameIndex> {
        let mut lines = text.lines();
        if lines.next()? != CACHE_HEADER {
            return None;
        }
        let names = lines
            .filter_map(|l| {
                let (h, n) = l.split_once('\t')?;
                Some((u32::from_str_radix(h, 16).ok()?, n.to_owned()))
            })
            .collect();
        Some(NameIndex { names })
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
                }
            }
        }
        self.names.len() - before
    }

    /// The display name of an entry, if its hash is known.
    pub fn name_for(&self, e: &Entry, platform: char) -> Option<String> {
        let n = self.names.get(&e.hash)?;
        Some(match extension(e.ext_id, platform) {
            Some(ext) if e.ext_id != 0xFF => format!("{n}.{ext}"),
            _ => n.clone(),
        })
    }
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
        if !c.is_empty() && !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Calls `f` with every path-like string in `data` (letters, digits and `_/\.:-`, 3–199 bytes).
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
pub fn build(
    files: &[PathBuf],
    keys: &Arc<Rdr2Keys>,
    wanted: &HashSet<u32>,
    progress: impl Fn(usize, usize) + Sync,
) -> NameIndex {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn walk(r: &Rpf8, wanted: &HashSet<u32>, out: &mut HashMap<u32, String>) {
        let Some(es) = r.entries() else { return };
        for e in es {
            if e.ext_id == 0 {
                if let Ok(c) = r.open_child(e) {
                    walk(&c, wanted, out);
                }
            } else if e.size() < 16 << 20
                && let Ok(data) = r.read(e)
            {
                strings_in(&data, |s| {
                    for c in candidates(s) {
                        let h = joaat(&c);
                        if wanted.contains(&h) {
                            out.entry(h).or_insert(c);
                        }
                    }
                });
            }
        }
    }
    let done = AtomicUsize::new(0);
    let names = files
        .par_iter()
        .map(|p| {
            let mut out = HashMap::new();
            if let Ok(r) = Rpf8::open_with_keys(p, Some(keys.clone())) {
                walk(&r, wanted, &mut out);
            }
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, files.len());
            out
        })
        .reduce(HashMap::new, |mut a, b| {
            for (h, n) in b {
                a.entry(h).or_insert(n);
            }
            a
        });
    NameIndex { names }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        strings_in(b"\x00\x01p_chair01x\x00ab\x00platform:/levels/rdr3\xff", |s| found.push(s.to_owned()));
        assert_eq!(found, ["p_chair01x", "platform:/levels/rdr3"]);

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
