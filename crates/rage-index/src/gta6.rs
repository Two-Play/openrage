//! Names of GTA VI's files from those of others.
//!
//! The files of one thing lie in different archives: its model in one, what
//! rays hit of it in another. An archive only tells the hashes of its files'
//! names; with the names of all archives together, more of them are known.

use std::collections::{HashMap, HashSet};

use rage_core::hash::joaat;
use rage_rpf::rpf9_names::hash64;
use rayon::prelude::*;
use rusqlite::params;

use crate::{Index, IndexError, update::stem};

/// What files that belong to one called otherwise have at the end.
const ENDINGS: [&str; 14] =
    ["", "_children", "_lod", "_slod", "_lod1", "_l1", "_l2", "_l3", "_m", "_a", "_b", "_d", "_int", "_ext"];

/// Where unnamed files are, and how their names start: `entries/000012_<hash>.<extension>`.
const UNNAMED: &str = "entries/";

/// The hash in the stand-in for an unknown name.
fn unnamed_hash(name: &str) -> Option<u64> {
    let (slot, rest) = name.split_once('_')?;
    let hash = rest.get(..16)?;
    let is_stand_in = slot.len() >= 6 && slot.bytes().all(|b| b.is_ascii_digit()) && rest[16..].starts_with('.');
    is_stand_in.then(|| u64::from_str_radix(hash, 16).ok()).flatten()
}

/// A file whose name is only known by its hash.
struct Unnamed {
    id: i64,
    vpath: String,
    name: String,
}

type UnnamedByHash = HashMap<u64, Vec<Unnamed>>;

impl Index {
    /// Looks for the names of files without one among those of the other
    /// files and the words of the texts, names the files in the index, and
    /// gives the names that were found.
    pub fn learn_gta6_names(&mut self) -> Result<Vec<String>, IndexError> {
        let Some((mut unnamed, stems)) = self.collect_unnamed_and_stems()? else {
            return Ok(Vec::new());
        };
        let found = find_names(&stems, &unnamed);
        let tx = self.conn.transaction()?;
        rename_files(&tx, &found, &mut unnamed)?;
        tx.commit()?;
        Ok(found.into_values().collect())
    }

    /// The unnamed files by hash, and every stem and word that may name them; `None` if no file lacks a name.
    fn collect_unnamed_and_stems(&self) -> Result<Option<(UnnamedByHash, HashSet<String>)>, IndexError> {
        let mut unnamed: UnnamedByHash = HashMap::new();
        let mut stems: HashSet<String> = HashSet::new();
        // Archives keep theirs, as the files in them are found by it.
        let mut stmt = self.conn.prepare("SELECT id, vpath, name FROM entries WHERE kind >= 2")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            sort_entry(r, &mut unnamed, &mut stems)?;
        }
        if unnamed.is_empty() {
            return Ok(None);
        }
        self.add_dictionary_words(&mut stems)?;
        Ok(Some((unnamed, stems)))
    }

    fn add_dictionary_words(&self, stems: &mut HashSet<String>) -> Result<(), IndexError> {
        let mut stmt = self.conn.prepare("SELECT string FROM hashes")?;
        let words = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for word in words {
            stems.insert(word?);
        }
        Ok(())
    }
}

/// Files the row of an entry under its hash if it is a stand-in for an unknown name, else under its stem.
fn sort_entry(r: &rusqlite::Row, unnamed: &mut UnnamedByHash, stems: &mut HashSet<String>) -> rusqlite::Result<()> {
    let name: String = r.get(2)?;
    match unnamed_hash(&name) {
        Some(hash) => unnamed.entry(hash).or_default().push(Unnamed { id: r.get(0)?, vpath: r.get(1)?, name }),
        None => {
            stems.insert(stem(&name).to_owned());
        }
    }
    Ok(())
}

/// The candidate names (stems with and without endings) whose hash is one of an unnamed file.
fn find_names(stems: &HashSet<String>, unnamed: &UnnamedByHash) -> HashMap<u64, String> {
    stems
        .par_iter()
        .flat_map_iter(|stem| {
            let shorter = stem.rmatch_indices('_').take(2).map(|(at, _)| stem[..at].to_owned());
            let longer = ENDINGS.iter().map(move |end| format!("{stem}{end}"));
            longer.chain(shorter).filter_map(|name| {
                let hash = hash64(name.as_bytes());
                unnamed.contains_key(&hash).then_some((hash, name))
            })
        })
        .collect()
}

/// Gives the files their found names, unless a file of that name is already in the same source.
fn rename_files(
    tx: &rusqlite::Transaction,
    found: &HashMap<u64, String>,
    unnamed: &mut UnnamedByHash,
) -> Result<(), IndexError> {
    let mut taken = tx.prepare("SELECT 1 FROM entries WHERE source = (SELECT source FROM entries WHERE id = ?1) AND name_hash = ?2 AND vpath = ?3")?;
    let mut rename =
        tx.prepare("UPDATE entries SET vpath = ?2, name = ?3, name_hash = ?4, stem_hash = ?5 WHERE id = ?1")?;
    for (hash, name) in found {
        for file in unnamed.remove(hash).unwrap_or_default() {
            let Some(folder) = file.vpath.strip_suffix(&file.name).and_then(|v| v.strip_suffix(UNNAMED)) else {
                continue;
            };
            let ending = file.name.split_once('.').map_or("", |(_, ending)| ending);
            let name = format!("{name}.{ending}");
            let vpath = format!("{folder}{name}");
            let hash = i64::from(joaat(&name));
            if !taken.exists(params![file.id, hash, vpath])? {
                rename.execute(params![file.id, vpath, name, hash, i64::from(joaat(stem(&name)))])?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stand_ins_tell_the_hash() {
        assert_eq!(unnamed_hash("000012_00ff00000000abcd.#bdr"), Some(0x00FF_0000_0000_ABCD));
        assert_eq!(unnamed_hash("prop_000012_00ff00000000abcd.#dr"), None);
        assert_eq!(unnamed_hash("000012_00ff.#dr"), None);
    }

    #[test]
    fn files_are_named_after_those_of_other_archives() {
        let dir = std::env::temp_dir().join(format!("openrage-gta6-names-{}", std::process::id()));
        let mut index = Index::open(&dir.join("index.sqlite")).unwrap();
        let house = hash64(b"House_01");
        let add = |index: &Index, vpath: &str, source: i64| {
            let name = vpath.rsplit('/').next().unwrap();
            index
                .conn
                .execute(
                    "INSERT INTO entries(source, vpath, name, kind, size, name_hash, stem_hash) VALUES(?1, ?2, ?3, 3, 9, ?4, 0)",
                    params![source, vpath, name, i64::from(joaat(name))],
                )
                .unwrap();
        };
        add(&index, "levels.rpf/House_01.#dr", 1);
        add(&index, &format!("bvh.rpf/entries/000003_{house:016x}.#bdr"), 2);
        add(&index, "bvh.rpf/entries/000004_0000000000000001.#bdr", 2);
        assert_eq!(index.learn_gta6_names().unwrap(), ["House_01"]);
        let count = |vpath: &str| -> i64 {
            index.conn.query_row("SELECT count(*) FROM entries WHERE vpath = ?1", [vpath], |r| r.get(0)).unwrap()
        };
        assert_eq!(count("bvh.rpf/House_01.#bdr"), 1);
        assert_eq!(count("bvh.rpf/entries/000004_0000000000000001.#bdr"), 1);
        drop(index);
        let _ = std::fs::remove_dir_all(dir);
    }
}
