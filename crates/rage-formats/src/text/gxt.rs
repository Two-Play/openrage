//! GTA IV text tables (`.gxt`).
//!
//! Layout (little endian): `u16` version (4), `u16` bits per character (16),
//! then `TABL`, its size and 12-byte records (8-byte table name, offset).
//! Each table — after its 8-byte name, except for `MAIN` — has a `TKEY`
//! block of (offset, key hash) pairs and a `TDAT` block of NUL-terminated
//! UTF-16 strings the offsets point into.
//!
//! The text form lists the tables in order, `[NAME]` followed by one
//! `0xHASH = text` line per string (labels by name where known), with the
//! escapes of [`crate::gxt2`].

use crate::gxt2::{escape, unescape};
use rage_core::bytes::LeRead;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GxtError {
    #[error("{0}")]
    Invalid(String),
}

fn err(msg: impl Into<String>) -> GxtError {
    GxtError::Invalid(msg.into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    /// (key hash, text) in the order of the file.
    pub entries: Vec<(u32, String)>,
}

fn u32_at(d: &[u8], o: usize) -> Result<u32, GxtError> {
    d.u32_at(o).ok_or_else(|| err(format!("truncated at {o:#x}")))
}

fn block(d: &[u8], o: usize, magic: &[u8; 4]) -> Result<(usize, usize), GxtError> {
    if d.get(o..o + 4) != Some(magic) {
        return Err(err(format!("expected {} at {o:#x}", String::from_utf8_lossy(magic))));
    }
    let size = u32_at(d, o + 4)? as usize;
    let start = o + 8;
    if start + size > d.len() {
        return Err(err(format!("{} block runs past the end", String::from_utf8_lossy(magic))));
    }
    Ok((start, size))
}

fn utf16_at(d: &[u8], o: usize) -> Result<String, GxtError> {
    let mut units = Vec::new();
    let mut p = o;
    loop {
        let b = d.get(p..p + 2).ok_or_else(|| err(format!("unterminated string at {o:#x}")))?;
        let u = u16::from_le_bytes([b[0], b[1]]);
        if u == 0 {
            break;
        }
        units.push(u);
        p += 2;
    }
    Ok(String::from_utf16_lossy(&units))
}

/// Whether `data` starts like a GTA IV text table.
pub fn is_gxt(data: &[u8]) -> bool {
    data.len() >= 12 && data[0..4] == [4, 0, 16, 0] && &data[4..8] == b"TABL"
}

pub fn parse(d: &[u8]) -> Result<Vec<Table>, GxtError> {
    if !is_gxt(d) {
        return Err(err("not a GTA IV text table (version 4, 16-bit characters)"));
    }
    let (tabl, tabl_size) = block(d, 4, b"TABL")?;
    let mut tables = Vec::with_capacity(tabl_size / 12);
    for k in 0..tabl_size / 12 {
        let rec = tabl + k * 12;
        let name = String::from_utf8_lossy(d[rec..rec + 8].split(|b| *b == 0).next().unwrap_or_default()).into_owned();
        let mut o = u32_at(d, rec + 8)? as usize;
        if name != "MAIN" {
            o += 8;
        }
        let (keys, keys_size) = block(d, o, b"TKEY")?;
        let (data, _) = block(d, keys + keys_size, b"TDAT")?;
        let mut entries = Vec::with_capacity(keys_size / 8);
        for i in 0..keys_size / 8 {
            let offset = u32_at(d, keys + i * 8)? as usize;
            let hash = u32_at(d, keys + i * 8 + 4)?;
            entries.push((hash, utf16_at(d, data + offset)?));
        }
        tables.push(Table { name, entries });
    }
    Ok(tables)
}

/// Writes tables back: `MAIN` and the others in the given order, strings in
/// the order of their keys.
pub fn write(tables: &[Table]) -> Vec<u8> {
    let mut out = vec![4, 0, 16, 0];
    out.extend_from_slice(b"TABL");
    out.extend_from_slice(&(tables.len() as u32 * 12).to_le_bytes());
    let records = out.len();
    out.resize(records + tables.len() * 12, 0);
    for (k, t) in tables.iter().enumerate() {
        let rec = records + k * 12;
        let name = t.name.as_bytes();
        out[rec..rec + name.len().min(8)].copy_from_slice(&name[..name.len().min(8)]);
        let at = out.len() as u32;
        out[rec + 8..rec + 12].copy_from_slice(&at.to_le_bytes());
        if t.name != "MAIN" {
            let mut n = [0u8; 8];
            n[..name.len().min(8)].copy_from_slice(&name[..name.len().min(8)]);
            out.extend_from_slice(&n);
        }
        let mut dat: Vec<u8> = Vec::new();
        let mut keys: Vec<u8> = Vec::with_capacity(t.entries.len() * 8);
        for (hash, text) in &t.entries {
            keys.extend_from_slice(&(dat.len() as u32).to_le_bytes());
            keys.extend_from_slice(&hash.to_le_bytes());
            for u in text.encode_utf16() {
                dat.extend_from_slice(&u.to_le_bytes());
            }
            dat.extend_from_slice(&[0, 0]);
        }
        out.extend_from_slice(b"TKEY");
        out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        out.extend_from_slice(&keys);
        out.extend_from_slice(b"TDAT");
        out.extend_from_slice(&(dat.len() as u32).to_le_bytes());
        out.extend_from_slice(&dat);
    }
    out
}

/// The text form; `name` gives a label's name when it is known.
pub fn to_text(tables: &[Table], name: impl Fn(u32) -> Option<String>) -> String {
    let mut out = String::new();
    for t in tables {
        out.push_str(&format!("[{}]\n", t.name));
        for (hash, text) in &t.entries {
            let label = name(*hash).unwrap_or_else(|| format!("0x{hash:08X}"));
            out.push_str(&format!("\t{label} = {}\n", escape(text)));
        }
    }
    out
}

/// Reads the text form; labels are `0x…` hashes or names (hashed with joaat).
pub fn from_text(text: &str) -> Result<Vec<Table>, GxtError> {
    let mut tables: Vec<Table> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            if name.is_empty() || name.len() > 7 {
                return Err(err(format!("line {}: table names have 1 to 7 characters", n + 1)));
            }
            tables.push(Table { name: name.to_owned(), entries: Vec::new() });
            continue;
        }
        let table = tables.last_mut().ok_or_else(|| err(format!("line {}: text before the first [TABLE]", n + 1)))?;
        let (label, value) = line
            .split_once(" = ")
            .or_else(|| line.split_once('='))
            .ok_or_else(|| err(format!("line {}: expected `label = text`", n + 1)))?;
        let label = label.trim();
        let hash = match label.strip_prefix("0x").or_else(|| label.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| err(format!("line {}: bad hash {label}", n + 1)))?,
            None => rage_core::hash::joaat(label),
        };
        table.entries.push((hash, unescape(value)));
    }
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Table> {
        vec![
            Table {
                name: "MAIN".into(),
                entries: vec![(0x843AFC84, "Error reading the DVD".into()), (1, "Zwei\nZeilen".into())],
            },
            Table { name: "AUTOERT".into(), entries: vec![(0x22197, "HOME".into())] },
        ]
    }

    #[test]
    fn round_trips_through_the_file_and_the_text() {
        let file = write(&sample());
        assert!(is_gxt(&file));
        let tables = parse(&file).unwrap();
        assert_eq!(tables, sample());
        let text = to_text(&tables, |_| None);
        assert!(text.starts_with("[MAIN]\n\t0x843AFC84 = Error reading the DVD\n"));
        assert_eq!(from_text(&text).unwrap(), sample());
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse(b"RSC\x05....").is_err());
    }
}
