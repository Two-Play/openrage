//! Querying the index: names, hashes and full text.

use std::collections::HashSet;

use rage_core::hash::joaat;
use rusqlite::params;

use crate::content::hashed_names;
use crate::{EntryTag, Index, IndexError};

/// Why an entry matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitReason {
    /// The name contains the query.
    Name,
    /// joaat(name) or joaat(name without extension) equals the queried hash.
    NameHash,
    /// The file's text contains the query terms.
    Content,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub vpath: String,
    pub name: String,
    pub kind: EntryTag,
    pub size: u64,
    pub reason: HitReason,
}

#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    /// joaat of the query text, shown so users can look up names quickly.
    pub query_hash: u32,
    /// The query, if it was a hash (`0x…` or a decimal, possibly negative).
    pub hash: Option<u32>,
    /// Strings from the dictionary that hash to `hash`.
    pub dictionary: Vec<String>,
    pub hits: Vec<Hit>,
    /// More hits exist than were returned.
    pub limited: bool,
}

/// Parses `0xB779A091`, `B779A091` (8 hex digits), `3078201489` or `-1216766607`.
pub fn parse_hash(query: &str) -> Option<u32> {
    let q = query.trim();
    if let Some(hex) = q.strip_prefix("0x").or_else(|| q.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).ok();
    }
    if q.len() == 8 && q.chars().all(|c| c.is_ascii_hexdigit()) && q.chars().any(|c| c.is_ascii_digit()) {
        return u32::from_str_radix(q, 16).ok();
    }
    if q.len() >= 5 && q.trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) {
        let v: i64 = q.parse().ok()?;
        return match v {
            0..=0xFFFF_FFFF => Some(v as u32),
            -0x8000_0000..0 => Some(v as i32 as u32),
            _ => None,
        };
    }
    None
}

/// Text languages of the games, as used in the names of its `*.rpf` language archives.
/// `brazilian` and `chinesetrad` are GTA VI's.
pub const LANGUAGES: [&str; 15] = [
    "american",
    "brazilian",
    "chinese",
    "chinesesimp",
    "chinesetrad",
    "french",
    "german",
    "italian",
    "japanese",
    "korean",
    "mexican",
    "polish",
    "portuguese",
    "russian",
    "spanish",
];

/// Language of a localized file, derived from its path (`…/germandlc.rpf/…`, `…/german_rel.rpf/…`,
/// `…/german_final.rpf/…`, `…/lang/german/…`).
pub fn language_of(vpath: &str) -> Option<&'static str> {
    vpath.split('/').find_map(|seg| {
        let seg = seg.to_ascii_lowercase();
        // GTA IV keeps its text tables loose: `common/text/german.gxt`.
        let base = seg.strip_suffix(".rpf").or_else(|| seg.strip_suffix(".gxt")).unwrap_or(&seg);
        let base = ["dlc", "_rel", "_final"].iter().find_map(|end| base.strip_suffix(end)).unwrap_or(base);
        LANGUAGES.iter().find(|&&l| l == base).copied()
    })
}

/// FTS5 query: every whitespace-separated term must appear (prefix match).
/// Other characters end a word in the text too, so `vehicles.meta` means its
/// words next to each other. A single name may also appear as its hash.
fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty()).collect::<Vec<_>>())
        .filter(|words| !words.is_empty())
        .map(|words| format!("\"{}\"*", words.join(" ")))
        .collect();
    let terms = (!terms.is_empty()).then(|| terms.join(" "))?;
    Some(match hashed_names(query.trim()) {
        Some([exact, lower]) => format!("{terms} OR \"{exact}\" OR \"{lower}\""),
        None => terms,
    })
}

fn like_pattern(query: &str) -> String {
    let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// Collects hits in order, skipping repeated paths, until `limit` is reached.
struct Collector<'a> {
    limit: usize,
    /// How many rows to ask the database for: one more than `limit`.
    fetch: i64,
    language: Option<&'a str>,
    seen: HashSet<String>,
    results: SearchResults,
}

impl Collector<'_> {
    fn wanted_language(&self, vpath: &str) -> bool {
        match (self.language, language_of(vpath)) {
            (Some(want), Some(lang)) => want.eq_ignore_ascii_case(lang),
            _ => true,
        }
    }

    fn push(&mut self, hits: Vec<Hit>) {
        for h in hits {
            if self.results.hits.len() >= self.limit {
                self.results.limited = true;
                return;
            }
            if self.seen.insert(h.vpath.clone()) {
                self.results.hits.push(h);
            }
        }
    }
}

fn row_to_hit(reason: HitReason) -> impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<Hit> {
    move |r| {
        Ok(Hit {
            vpath: r.get(0)?,
            name: r.get(1)?,
            kind: EntryTag::from_i64(r.get(2)?),
            size: r.get::<_, i64>(3)? as u64,
            reason: reason.clone(),
        })
    }
}

impl Index {
    /// Searches names, hashes and text. Name and hash hits come first.
    ///
    /// With `language`, text hits in localized files of other languages are skipped.
    pub fn search(&self, query: &str, limit: usize, language: Option<&str>) -> Result<SearchResults, IndexError> {
        let query = query.trim();
        let results = SearchResults { query_hash: joaat(query), hash: parse_hash(query), ..Default::default() };
        if query.is_empty() {
            return Ok(results);
        }
        let hash = results.hash;
        let mut collector = Collector { limit, fetch: limit as i64 + 1, language, seen: HashSet::new(), results };
        match hash {
            Some(hash) => self.search_hash(hash, &mut collector)?,
            None => self.search_words(query, &mut collector)?,
        }
        Ok(collector.results)
    }

    /// Hits for a query that is a hash: dictionary strings, names with that hash, and texts that contain it.
    fn search_hash(&self, hash: u32, c: &mut Collector) -> Result<(), IndexError> {
        let fetch = c.fetch;
        let mut stmt =
            self.conn.prepare_cached("SELECT string FROM hashes WHERE hash = ?1 ORDER BY string LIMIT 50")?;
        c.results.dictionary = stmt.query_map([hash as i64], |r| r.get(0))?.collect::<Result<_, _>>()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT vpath, name, kind, size FROM entries WHERE name_hash = ?1 OR stem_hash = ?1 ORDER BY vpath LIMIT ?2",
        )?;
        let hits =
            stmt.query_map(params![hash as i64, fetch], row_to_hit(HitReason::NameHash))?.collect::<Result<_, _>>()?;
        c.push(hits);
        // GXT2 labels are indexed as `0xHASH text` lines, unknown names in
        // metadata as `hash_HASH`.
        let mut stmt = self.conn.prepare_cached(
            "SELECT e.vpath, e.name, e.kind, e.size FROM content JOIN entries e ON e.id = content.rowid
                 WHERE content MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let hits: Vec<Hit> = stmt
            .query_map(
                params![format!("\"0x{hash:08x}\" OR \"hash_{hash:08x}\""), fetch * 20],
                row_to_hit(HitReason::Content),
            )?
            .collect::<Result<_, _>>()?;
        c.push(hits.into_iter().filter(|h| c.wanted_language(&h.vpath)).collect());
        Ok(())
    }

    /// Hits for a query that is a name, a path or words of a text.
    fn search_words(&self, query: &str, c: &mut Collector) -> Result<(), IndexError> {
        let mut names = self.search_names(query, c.fetch)?;
        let texts = self.search_texts(query, c)?;
        // Names come first, but leave room for some text hits: a common word
        // is in more names than are shown.
        let limit = c.limit;
        let named: HashSet<&str> = names.iter().map(|h| h.vpath.as_str()).collect();
        let room = texts.iter().filter(|h| !named.contains(h.vpath.as_str())).take(limit / 4).count();
        if names.len() > limit - room {
            names.truncate(limit - room);
            c.results.limited = true;
        }
        c.push(names);
        c.push(texts);
        Ok(())
    }

    fn search_names(&self, query: &str, fetch: i64) -> Result<Vec<Hit>, IndexError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT vpath, name, kind, size FROM entries
             WHERE name LIKE ?1 ESCAPE '\\' OR ?5 AND vpath LIKE ?1 ESCAPE '\\'
                 OR name_hash = ?3 OR stem_hash = ?3         -- also where only the hash is known
             ORDER BY CASE
                 WHEN name_hash = ?3 OR stem_hash = ?3 THEN 0   -- exact name or stem
                 WHEN name LIKE ?4 ESCAPE '\\' THEN 1          -- prefix
                 ELSE 2 END,
             length(name), vpath LIMIT ?2",
        )?;
        let prefix = format!("{}%", like_pattern(query).trim_start_matches('%'));
        let path = query.contains(['/', '\\']);
        let pattern = like_pattern(&query.replace('\\', "/"));
        let names = stmt
            .query_map(params![pattern, fetch, joaat(query) as i64, prefix, path], row_to_hit(HitReason::Name))?
            .collect::<Result<_, _>>()?;
        Ok(names)
    }

    /// Hits in the text of files, in the wanted language; none if the query has no words.
    fn search_texts(&self, query: &str, c: &Collector) -> Result<Vec<Hit>, IndexError> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT e.vpath, e.name, e.kind, e.size FROM content JOIN entries e ON e.id = content.rowid
                 WHERE content MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let mut texts: Vec<Hit> =
            stmt.query_map(params![fts, c.fetch * 20], row_to_hit(HitReason::Content))?.collect::<Result<_, _>>()?;
        texts.retain(|h| c.wanted_language(&h.vpath));
        Ok(texts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hash_forms() {
        assert_eq!(parse_hash("0xB779A091"), Some(0xB779_A091));
        assert_eq!(parse_hash("b779a091"), Some(0xB779_A091));
        assert_eq!(parse_hash("3078201489"), Some(0xB779_A091));
        assert_eq!(parse_hash("-1216765807"), Some(0xB779_A091));
        assert_eq!(parse_hash("adder"), None);
        assert_eq!(parse_hash("deadbeef"), None); // 8 hex letters without digits read as a word
        assert_eq!(parse_hash("123"), None);
    }

    #[test]
    fn detects_languages() {
        assert_eq!(language_of("x64b.rpf/data/lang/chinese_rel.rpf/global.gxt2"), Some("chinese"));
        assert_eq!(language_of("update/x/dlc.rpf/x64/data/lang/germandlc.rpf/a.gxt2"), Some("german"));
        assert_eq!(language_of("data.rpf/data/lang/chinesetrad_final.rpf/a.#ldbmt"), Some("chinesetrad"));
        assert_eq!(language_of("x64e.rpf/levels/gta5/vehicles.rpf/adder.yft"), None);
    }

    #[test]
    fn finds_paths_and_unnamed_entries_by_the_hash_of_their_name() {
        let dir = std::env::temp_dir().join(format!("openrage-index-search-{}", std::process::id()));
        let index = Index::open(&dir.join("index.sqlite")).unwrap();
        for (vpath, hash) in [("a.rpf/cars/adder.#ft", None), ("a.rpf/entries/000001_00ff.#mt", Some(joaat("adder")))] {
            let name = vpath.rsplit('/').next().unwrap();
            index
                .conn
                .execute(
                    "INSERT INTO entries(source, vpath, name, kind, size, name_hash, stem_hash)
                     VALUES(1, ?1, ?2, 2, 0, ?3, ?3)",
                    params![vpath, name, hash.unwrap_or_else(|| joaat(name)) as i64],
                )
                .unwrap();
        }
        let hits =
            |q: &str| -> Vec<String> { index.search(q, 20, None).unwrap().hits.into_iter().map(|h| h.vpath).collect() };
        assert_eq!(hits("Adder").len(), 2);
        assert_eq!(hits("0xB779A091"), ["a.rpf/entries/000001_00ff.#mt"]);
        assert_eq!(hits("cars/add"), ["a.rpf/cars/adder.#ft"]);
        assert_eq!(hits("cars\\add"), ["a.rpf/cars/adder.#ft"]);
        assert!(hits("cars").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_hits_have_room_beside_names() {
        let dir = std::env::temp_dir().join(format!("openrage-index-room-{}", std::process::id()));
        let index = Index::open(&dir.join("index.sqlite")).unwrap();
        for i in 0..30 {
            let name = if i < 20 { format!("motel_{i}.#dr") } else { format!("{i}.xml") };
            let insert = "INSERT INTO entries(id, source, vpath, name, kind, size, name_hash, stem_hash)
                          VALUES(?1, 1, ?2, ?2, 2, 0, 0, 0)";
            index.conn.execute(insert, params![i, name]).unwrap();
            if i >= 20 {
                index.conn.execute("INSERT INTO content(rowid, body) VALUES(?1, 'a motel')", [i]).unwrap();
            }
        }
        let results = index.search("motel", 12, None).unwrap();
        let texts = results.hits.iter().filter(|h| h.reason == HitReason::Content).count();
        assert_eq!((results.hits.len(), texts, results.limited), (12, 3, true));
        let results = index.search("motel", 30, None).unwrap();
        assert_eq!((results.hits.len(), results.limited), (30, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_fts_queries() {
        let hash = joaat("weapon_pistol");
        let expected = format!("\"weapon_pistol\"* OR \"hash_{hash:08x}\" OR \"hash_{hash:08x}\"");
        assert_eq!(fts_query("weapon_pistol"), Some(expected));
        // The text is split at the dot, too.
        assert_eq!(fts_query("vehicles.meta").as_deref(), Some("\"vehicles meta\"*"));
        assert_eq!(fts_query("a \"b\" -c").as_deref(), Some("\"a\"* \"b\"* \"c\"*"));
        assert_eq!(fts_query("  ").as_deref(), None);
    }
}
