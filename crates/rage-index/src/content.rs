//! Turns indexable files into searchable text.

use rage_core::io::u32_le;
use rage_core::text::{decode_text, identifiers};

const MAX_TEXT: usize = 4 << 20;

/// Extensions of plain-text formats worth indexing.
const TEXT_EXTENSIONS: &[&str] =
    &["xml", "meta", "dat", "txt", "ide", "ipl", "ugc", "json", "ini", "cfg", "csv", "lua", "log"];

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Whether a binary file with this name contributes text to the index.
pub fn is_indexable(name: &str) -> bool {
    let ext = extension(name);
    ext == "gxt2" || ext == "gxt" || ext == "nametable" || TEXT_EXTENSIONS.contains(&ext.as_str())
}

/// Searchable text for a file, plus names for the hash dictionary.
///
/// * text formats: the text itself; dictionary = identifiers in it
/// * `.nametable`: one name per line; dictionary = all names
/// * `.gxt2`, GTA IV `.gxt`: one `0xLABELHASH text` line per string; no
///   dictionary entries
pub fn content_for(name: &str, data: &[u8]) -> Option<(String, Vec<String>)> {
    match extension(name).as_str() {
        "gxt2" => parse_gxt2(data).map(|body| (body, Vec::new())),
        "gxt" => parse_gxt(data).map(|body| (body, Vec::new())),
        "nametable" => {
            let names: Vec<String> = data
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            (!names.is_empty()).then(|| (names.join("\n"), names))
        }
        ext if TEXT_EXTENSIONS.contains(&ext) => {
            let text = decode_text(data, MAX_TEXT)?;
            let mut dict: Vec<String> = identifiers(&text).map(str::to_owned).collect();
            dict.sort_unstable();
            dict.dedup();
            Some((text, dict))
        }
        _ => None,
    }
}

/// GXT2: `"2TXG"`, count, `count × (label hash, absolute offset)`, then NUL-terminated UTF-8 strings.
fn parse_gxt2(data: &[u8]) -> Option<String> {
    if data.len() < 8 || &data[0..4] != b"2TXG" {
        return None;
    }
    let count = u32_le(data, 4) as usize;
    let table_end = 8usize.checked_add(count.checked_mul(8)?)?;
    if table_end > data.len() {
        return None;
    }
    let mut out = String::new();
    for i in 0..count {
        let hash = u32_le(data, 8 + i * 8);
        let offset = u32_le(data, 12 + i * 8) as usize;
        let Some(tail) = data.get(offset..) else { continue };
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        out.push_str(&format!("0x{hash:08X} {}\n", String::from_utf8_lossy(&tail[..end])));
    }
    Some(out)
}

/// GTA IV GXT: `u16` 4, `u16` 16, `TABL` (12-byte records: name, offset);
/// each table (after its 8-byte name, except `MAIN`) has a `TKEY` block of
/// (offset, hash) pairs and a `TDAT` block of NUL-terminated UTF-16 strings.
fn parse_gxt(data: &[u8]) -> Option<String> {
    if data.len() < 12 || data[0..4] != [4, 0, 16, 0] || &data[4..8] != b"TABL" {
        return None;
    }
    let block = |at: usize, magic: &[u8; 4]| -> Option<(usize, usize)> {
        (data.get(at..at + 4)? == magic).then(|| (at + 8, u32_le(data, at + 4) as usize))
    };
    let (tabl, tabl_len) = block(4, b"TABL")?;
    let mut out = String::new();
    for k in 0..tabl_len / 12 {
        let rec = tabl + k * 12;
        let name = data.get(rec..rec + 8)?;
        let mut at = u32_le(data, rec + 8) as usize;
        if !name.starts_with(b"MAIN\0") {
            at += 8;
        }
        let (keys, keys_len) = block(at, b"TKEY")?;
        let (strings, _) = block(keys + keys_len, b"TDAT")?;
        for i in 0..keys_len / 8 {
            let offset = u32_le(data, keys + i * 8) as usize;
            let hash = u32_le(data, keys + i * 8 + 4);
            let start = strings + offset;
            let units: Vec<u16> = data
                .get(start..)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&u| u != 0)
                .collect();
            out.push_str(&format!("0x{hash:08X} {}\n", String::from_utf16_lossy(&units)));
        }
    }
    Some(out)
}

/// First line of `text` containing any of the query terms (case-insensitive), shortened.
pub fn snippet(text: &str, query: &str) -> Option<String> {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.trim_matches('"').to_lowercase()).collect();
    let line = text.lines().find(|l| {
        let lower = l.to_lowercase();
        terms.iter().any(|t| !t.is_empty() && lower.contains(t.as_str()))
    })?;
    let line = line.trim();
    Some(if line.chars().count() > 200 {
        format!("{}…", line.chars().take(200).collect::<String>())
    } else {
        line.into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gxt2(entries: &[(u32, &str)]) -> Vec<u8> {
        let mut out = b"2TXG".to_vec();
        out.extend((entries.len() as u32).to_le_bytes());
        let mut strings = Vec::new();
        let base = 8 + entries.len() * 8 + 8;
        for (hash, text) in entries {
            out.extend(hash.to_le_bytes());
            out.extend(((base + strings.len()) as u32).to_le_bytes());
            strings.extend(text.as_bytes());
            strings.push(0);
        }
        out.extend(b"2TXG");
        out.extend(((base + strings.len()) as u32).to_le_bytes());
        out.extend(strings);
        out
    }

    #[test]
    fn parses_gta4_gxt() {
        // One MAIN table with one string.
        let mut d = vec![4, 0, 16, 0];
        d.extend(b"TABL");
        d.extend(12u32.to_le_bytes());
        d.extend(b"MAIN\0\0\0\0");
        d.extend(24u32.to_le_bytes());
        d.extend(b"TKEY");
        d.extend(8u32.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.extend(0x22197u32.to_le_bytes());
        d.extend(b"TDAT");
        d.extend(10u32.to_le_bytes());
        for u in "HOME".encode_utf16().chain([0]) {
            d.extend(u.to_le_bytes());
        }
        let (body, dict) = content_for("american.gxt", &d).unwrap();
        assert_eq!(body, "0x00022197 HOME\n");
        assert!(dict.is_empty());
    }

    #[test]
    fn parses_gxt2() {
        let (body, dict) = content_for("global.gxt2", &gxt2(&[(0xB779_A091, "Adder"), (1, "Hello")])).unwrap();
        assert_eq!(body, "0xB779A091 Adder\n0x00000001 Hello\n");
        assert!(dict.is_empty());
    }

    #[test]
    fn nametable_and_text() {
        let (body, dict) = content_for("a.nametable", b"one\0two\0").unwrap();
        assert_eq!(body, "one\ntwo");
        assert_eq!(dict, ["one", "two"]);
        let (_, dict) = content_for("x.meta", b"<Item>WEAPON_PISTOL</Item>").unwrap();
        assert_eq!(dict, ["Item", "WEAPON_PISTOL"]);
        assert!(content_for("x.ydr", b"abc").is_none());
    }

    #[test]
    fn finds_snippet() {
        assert_eq!(snippet("a\n  <b>Pistol</b>\nc", "pistol").as_deref(), Some("<b>Pistol</b>"));
    }
}
