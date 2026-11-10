//! Querying the index: names, hashes and full text.

use std::collections::HashSet;

use rage_core::hash::joaat;
use rusqlite::params;

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

/// Text languages of GTA V, as used in the names of its `*.rpf` language archives.
pub const LANGUAGES: [&str; 13] = [
    "american",
    "chinese",
    "chinesesimp",
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

/// Language of a localized file, derived from its path (`…/germandlc.rpf/…`, `…/german_rel.rpf/…`, `…/lang/german/…`).
pub fn language_of(vpath: &str) -> Option<&'static str> {
    vpath.split('/').find_map(|seg| {
        let seg = seg.to_ascii_lowercase();
        let base = seg.strip_suffix(".rpf").unwrap_or(&seg);
        let base = base.strip_suffix("dlc").or_else(|| base.strip_suffix("_rel")).unwrap_or(base);
        LANGUAGES.iter().find(|&&l| l == base).copied()
    })
}

/// FTS5 query: every whitespace-separated term must appear (prefix match).
fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect::<String>())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn like_pattern(query: &str) -> String {
    let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

impl Index {
    /// Searches names, hashes and text. Name and hash hits come first.
    ///
    /// With `language`, text hits in localized files of other languages are skipped.
    pub fn search(&self, query: &str, limit: usize, language: Option<&str>) -> Result<SearchResults, IndexError> {
        let query = query.trim();
        let mut results = SearchResults { query_hash: joaat(query), hash: parse_hash(query), ..Default::default() };
        if query.is_empty() {
            return Ok(results);
        }
        let mut seen = HashSet::new();
        let wanted_language = |vpath: &str| match (language, language_of(vpath)) {
            (Some(want), Some(lang)) => want.eq_ignore_ascii_case(lang),
            _ => true,
        };
        let fetch = limit as i64 + 1;

        let row_to_hit = |reason: HitReason| {
            move |r: &rusqlite::Row<'_>| -> rusqlite::Result<Hit> {
                Ok(Hit {
                    vpath: r.get(0)?,
                    name: r.get(1)?,
                    kind: EntryTag::from_i64(r.get(2)?),
                    size: r.get::<_, i64>(3)? as u64,
                    reason: reason.clone(),
                })
            }
        };
        let mut push = |hits: Vec<Hit>, results: &mut SearchResults| {
            for h in hits {
                if results.hits.len() >= limit {
                    results.limited = true;
                    return;
                }
                if seen.insert(h.vpath.clone()) {
                    results.hits.push(h);
                }
            }
        };

        if let Some(hash) = results.hash {
            let mut stmt =
                self.conn.prepare_cached("SELECT string FROM hashes WHERE hash = ?1 ORDER BY string LIMIT 50")?;
            results.dictionary = stmt.query_map([hash as i64], |r| r.get(0))?.collect::<Result<_, _>>()?;
            let mut stmt = self.conn.prepare_cached(
                "SELECT vpath, name, kind, size FROM entries WHERE name_hash = ?1 OR stem_hash = ?1 ORDER BY vpath LIMIT ?2",
            )?;
            let hits = stmt
                .query_map(params![hash as i64, fetch], row_to_hit(HitReason::NameHash))?
                .collect::<Result<_, _>>()?;
            push(hits, &mut results);
            // GXT2 labels are indexed as `0xHASH text` lines.
            let mut stmt = self.conn.prepare_cached(
                "SELECT e.vpath, e.name, e.kind, e.size FROM content JOIN entries e ON e.id = content.rowid
                 WHERE content MATCH ?1 ORDER BY rank LIMIT ?2",
            )?;
            let hits: Vec<Hit> = stmt
                .query_map(params![format!("\"0x{hash:08x}\""), fetch * 20], row_to_hit(HitReason::Content))?
                .collect::<Result<_, _>>()?;
            push(hits.into_iter().filter(|h| wanted_language(&h.vpath)).collect(), &mut results);
            return Ok(results);
        }

        let mut stmt = self.conn.prepare_cached(
            "SELECT vpath, name, kind, size FROM entries WHERE name LIKE ?1 ESCAPE '\\'
             ORDER BY CASE
                 WHEN name_hash = ?3 OR stem_hash = ?3 THEN 0   -- exact name or stem
                 WHEN name LIKE ?4 ESCAPE '\\' THEN 1          -- prefix
                 ELSE 2 END,
             length(name), vpath LIMIT ?2",
        )?;
        let prefix = format!("{}%", like_pattern(query).trim_start_matches('%'));
        let hits = stmt
            .query_map(params![like_pattern(query), fetch, joaat(query) as i64, prefix], row_to_hit(HitReason::Name))?
            .collect::<Result<_, _>>()?;
        push(hits, &mut results);

        if let Some(fts) = fts_query(query) {
            let mut stmt = self.conn.prepare_cached(
                "SELECT e.vpath, e.name, e.kind, e.size FROM content JOIN entries e ON e.id = content.rowid
                 WHERE content MATCH ?1 ORDER BY rank LIMIT ?2",
            )?;
            let hits: Vec<Hit> =
                stmt.query_map(params![fts, fetch * 20], row_to_hit(HitReason::Content))?.collect::<Result<_, _>>()?;
            push(hits.into_iter().filter(|h| wanted_language(&h.vpath)).collect(), &mut results);
        }
        Ok(results)
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
        assert_eq!(language_of("x64e.rpf/levels/gta5/vehicles.rpf/adder.yft"), None);
    }

    #[test]
    fn builds_fts_queries() {
        assert_eq!(fts_query("weapon_pistol").as_deref(), Some("\"weapon_pistol\"*"));
        assert_eq!(fts_query("a \"b\" -c").as_deref(), Some("\"a\"* \"b\"* \"c\"*"));
        assert_eq!(fts_query("  ").as_deref(), None);
    }
}
