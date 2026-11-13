//! Example plugin: applies an OpenIV text file (`.oxt`) to a GTA V text
//! table (`.gxt2`) and writes the result into the mods folder.
//!
//! `.oxt` lines are `LABEL = text` or `0x1234ABCD = text` (inside an
//! optional `Version 2 30 { … }`); `\n`, `\r` and `\\` are escapes. Entries
//! of the patch replace those with the same label or are added; all other
//! entries stay as they are.

use openrage_plugin_sdk::prelude::*;

struct Gxt2Patch;

impl Plugin for Gxt2Patch {
    fn preview(_vpath: String, _data: Vec<u8>) -> Result<Preview, String> {
        Err("this plugin has no preview".into())
    }

    fn run_action(action: String, inputs: Vec<Input>) -> Result<Vec<OutputFile>, String> {
        if action != "apply" {
            return Err(format!("unknown action {action:?}"));
        }
        let mut patch = Vec::new();
        for p in inputs.iter().filter(|i| i.picked) {
            let text = String::from_utf8_lossy(&p.data);
            patch.extend(parse_oxt(&text).map_err(|e| format!("{}: {e}", p.vpath))?);
        }
        if patch.is_empty() {
            return Err("the patch has no entries".into());
        }
        for table in inputs.iter().filter(|i| !i.picked) {
            let mut entries = parse_gxt2(&table.data).map_err(|e| format!("{}: {e}", table.vpath))?;
            let (mut changed, mut added) = (0, 0);
            for (hash, text) in &patch {
                match entries.iter_mut().find(|(h, _)| h == hash) {
                    Some((_, old)) if old == text => {}
                    Some((_, old)) => {
                        *old = text.clone();
                        changed += 1;
                    }
                    None => {
                        entries.push((*hash, text.clone()));
                        added += 1;
                    }
                }
            }
            log(Level::Info, format!("{}: {changed} changed, {added} added", table.vpath));
            if changed + added > 0 {
                write_mod_file(&table.vpath, &write_gxt2(&entries))?;
            }
        }
        Ok(Vec::new())
    }
}

openrage_plugin_sdk::export_plugin!(Gxt2Patch);

const MAGIC: &[u8; 4] = b"2TXG";

fn u32_at(d: &[u8], at: usize) -> Result<u32, String> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "file is cut off".into())
}

/// `"2TXG"`, count, `count × (hash, absolute offset)`, `"2TXG"`, file size,
/// NUL-terminated UTF-8 strings.
fn parse_gxt2(data: &[u8]) -> Result<Vec<(u32, String)>, String> {
    if !data.starts_with(MAGIC) {
        return Err("not a GXT2 text table".into());
    }
    let count = u32_at(data, 4)? as usize;
    let mut out = Vec::with_capacity(count.min(1 << 20));
    for i in 0..count {
        let hash = u32_at(data, 8 + i * 8)?;
        let offset = u32_at(data, 12 + i * 8)? as usize;
        let tail = data.get(offset..).ok_or("string beyond the end")?;
        let end = tail.iter().position(|&b| b == 0).ok_or("unterminated string")?;
        out.push((hash, String::from_utf8_lossy(&tail[..end]).into_owned()));
    }
    Ok(out)
}

fn write_gxt2(entries: &[(u32, String)]) -> Vec<u8> {
    let mut sorted: Vec<&(u32, String)> = entries.iter().collect();
    sorted.sort_by_key(|(h, _)| *h);
    let start = 8 + sorted.len() * 8 + 8;
    let (mut table, mut strings) = (Vec::new(), Vec::new());
    for (hash, text) in sorted {
        table.extend(hash.to_le_bytes());
        table.extend(((start + strings.len()) as u32).to_le_bytes());
        strings.extend(text.as_bytes());
        strings.push(0);
    }
    let mut out = Vec::with_capacity(start + strings.len());
    out.extend(MAGIC);
    out.extend((table.len() as u32 / 8).to_le_bytes());
    out.extend(table);
    out.extend(MAGIC);
    out.extend(((start + strings.len()) as u32).to_le_bytes());
    out.extend(strings);
    out
}

/// Rockstar's label hash (Jenkins one-at-a-time over the lowercase name).
fn joaat(s: &str) -> u32 {
    let mut h: u32 = 0;
    for b in s.bytes() {
        h = h.wrapping_add(u32::from(b.to_ascii_lowercase()));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

fn parse_oxt(text: &str) -> Result<Vec<(u32, String)>, String> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}');
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed == "{"
            || trimmed == "}"
            || trimmed.starts_with("Version ")
            || trimmed.starts_with('#')
            || trimmed.starts_with("//")
        {
            continue;
        }
        let (label, value) = line
            .split_once(" = ")
            .or_else(|| line.split_once('='))
            .ok_or_else(|| format!("line {}: expected `label = text`", n + 1))?;
        let label = label.trim();
        let hash = match label.strip_prefix("0x").or_else(|| label.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| format!("line {}: bad hash {label}", n + 1))?,
            None if !label.is_empty() => joaat(label),
            None => return Err(format!("line {}: empty label", n + 1)),
        };
        out.push((hash, unescape(value)));
    }
    Ok(out)
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some('n')) => {
                chars.next();
                out.push('\n');
            }
            ('\\', Some('r')) => {
                chars.next();
                out.push('\r');
            }
            ('\\', Some('\\')) => {
                chars.next();
                out.push('\\');
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_patches() {
        let table = write_gxt2(&[(joaat("HELLO"), "Hello".into()), (0x10, "Ten".into())]);
        assert_eq!(parse_gxt2(&table).unwrap(), [(0x10, "Ten".into()), (joaat("hello"), "Hello".into())]);
        let patch = parse_oxt("Version 2 30\n{\n\tHELLO = Hi\\nthere\n\t0x00000020 = New\n}\n").unwrap();
        assert_eq!(patch, [(joaat("HELLO"), "Hi\nthere".into()), (0x20, "New".into())]);
        assert!(parse_oxt("no equals sign").is_err());
        assert_eq!(joaat("adder"), 0xB779A091);
    }
}
