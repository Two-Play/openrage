//! Database schema and small helpers.

use rusqlite::{Connection, OptionalExtension, params};

/// Bump when the schema or the indexing rules change; older databases are rebuilt.
const SCHEMA_VERSION: &str = "2";

pub fn prepare(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);")?;
    if get_meta(conn, "schema")?.as_deref() != Some(SCHEMA_VERSION) {
        conn.execute_batch(
            "DROP TABLE IF EXISTS sources;
             DROP TABLE IF EXISTS entries;
             DROP TABLE IF EXISTS hashes;
             DROP TABLE IF EXISTS content;
             DELETE FROM meta;",
        )?;
        conn.execute_batch(
            "CREATE TABLE sources(
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,      -- top-level archive, relative; '' = loose files
                 size INTEGER NOT NULL,
                 mtime INTEGER NOT NULL
             );
             CREATE TABLE entries(
                 id INTEGER PRIMARY KEY,
                 source INTEGER NOT NULL,
                 vpath TEXT NOT NULL,
                 name TEXT NOT NULL,
                 kind INTEGER NOT NULL,
                 size INTEGER NOT NULL,
                 name_hash INTEGER NOT NULL,     -- joaat(name)
                 stem_hash INTEGER NOT NULL,     -- joaat(name without extension)
                 has_text INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX entries_source ON entries(source);
             CREATE INDEX entries_name_hash ON entries(name_hash);
             CREATE INDEX entries_stem_hash ON entries(stem_hash);
             CREATE TABLE hashes(
                 hash INTEGER NOT NULL,
                 string TEXT NOT NULL,
                 PRIMARY KEY(hash, string)
             ) WITHOUT ROWID;
             CREATE VIRTUAL TABLE content USING fts5(
                 body,
                 tokenize = \"unicode61 tokenchars '_'\",
                 content = '',
                 contentless_delete = 1
             );",
        )?;
        set_meta(conn, "schema", SCHEMA_VERSION)?;
    }
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}
