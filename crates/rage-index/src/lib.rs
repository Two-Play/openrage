//! Search index over RAGE game archives.
//!
//! One SQLite database per game installation holds:
//! * every entry of every archive (including nested archives) and loose file,
//!   searchable by name and by the joaat hash of its name;
//! * a hash dictionary of identifiers found in the game's text data, so a bare
//!   hash like `0xB779A091` can be turned back into a name;
//! * a full-text index (FTS5, contentless) over text files, audio name tables
//!   and GXT2 string tables.
//!
//! The index is updated incrementally: only top-level archives whose size or
//! modification time changed are re-read.

mod content;
mod schema;
mod search;
mod update;

use std::path::{Path, PathBuf};

use rusqlite::Connection;

pub use content::{content_for, snippet};
pub use search::{Hit, HitReason, LANGUAGES, SearchResults, language_of, parse_hash};
pub use update::{Progress, UpdateStats};

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("index database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Kind of an indexed entry (stored as an integer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryTag {
    Dir = 0,
    Archive = 1,
    File = 2,
    Resource = 3,
}

impl EntryTag {
    fn from_i64(v: i64) -> Self {
        match v {
            0 => Self::Dir,
            1 => Self::Archive,
            3 => Self::Resource,
            _ => Self::File,
        }
    }

    pub fn is_container(self) -> bool {
        matches!(self, Self::Dir | Self::Archive)
    }
}

/// Size and freshness information about an index.
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub archives: u64,
    pub entries: u64,
    pub text_documents: u64,
    pub dictionary: u64,
    pub file_bytes: u64,
    /// Unix time of the last completed update, if any.
    pub updated_at: Option<i64>,
}

pub struct Index {
    conn: Connection,
    path: PathBuf,
}

impl Index {
    /// Opens (or creates) the index database at `path`.
    pub fn open(path: &Path) -> Result<Self, IndexError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        schema::prepare(&conn)?;
        Ok(Self { conn, path: path.to_owned() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every file whose name ends with one of `extensions` (lowercase, with
    /// the dot): (name, virtual path).
    pub fn files_with_extensions(&self, extensions: &[&str]) -> Result<Vec<(String, String)>, IndexError> {
        let mut out = Vec::new();
        let mut stmt = self.conn.prepare("SELECT name, vpath FROM entries")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (name, vpath) = row?;
            let lower = name.to_ascii_lowercase();
            if extensions.iter().any(|e| lower.ends_with(e)) {
                out.push((lower, vpath));
            }
        }
        Ok(out)
    }

    /// Every name the index knows: file names without their extension and the
    /// strings of the hash dictionary; used to turn hashes back into names.
    pub fn known_names(&self) -> Result<Vec<String>, IndexError> {
        let mut out = Vec::new();
        let mut stmt = self.conn.prepare("SELECT DISTINCT name FROM entries")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for name in rows {
            let name = name?;
            let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
            out.push(stem.to_owned());
        }
        let mut stmt = self.conn.prepare("SELECT string FROM hashes")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for s in rows {
            out.push(s?);
        }
        Ok(out)
    }

    pub fn stats(&self) -> Result<IndexStats, IndexError> {
        let count =
            |sql: &str| -> Result<u64, IndexError> { Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64) };
        Ok(IndexStats {
            archives: count("SELECT count(*) FROM sources WHERE path <> ''")?,
            entries: count("SELECT count(*) FROM entries")?,
            text_documents: count("SELECT count(*) FROM entries WHERE has_text = 1")?,
            dictionary: count("SELECT count(*) FROM hashes")?,
            file_bytes: std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0),
            updated_at: schema::get_meta(&self.conn, "updated_at")?.and_then(|v| v.parse().ok()),
        })
    }
}
