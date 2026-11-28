//! GTA V text tables (`.gxt2`) and OpenIV's text form of them (`.oxt`).
//!
//! Layout (little-endian): `"2TXG"`, entry count, `count × (label hash,
//! absolute offset)` sorted by hash, `"2TXG"` again, the file size, then the
//! NUL-terminated UTF-8 strings.
//!
//! The `.oxt` form is one line per entry inside `Version 2 30 { … }`:
//! `0x1234ABCD = text` (or `LABEL_NAME = text` when the name is known). The few
//! strings with real line breaks keep them as `\n` (and `\r`, `\\` for a
//! backslash), so converting back gives the identical file.

use rage_core::bytes::LeRead;
use rage_core::hash::joaat;

pub const MAGIC: &[u8; 4] = b"2TXG";

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Gxt2Error {
    #[error("invalid text table: {0}")]
    Invalid(String),
}

/// One string: the hash of its label and the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub hash: u32,
    pub text: String,
}

pub fn is_gxt2(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, Gxt2Error> {
    d.u32_at(at).ok_or_else(|| Gxt2Error::Invalid(format!("offset {at:#x} beyond the end")))
}

pub fn parse(data: &[u8]) -> Result<Vec<Entry>, Gxt2Error> {
    if !is_gxt2(data) {
        return Err(Gxt2Error::Invalid("missing 2TXG header".into()));
    }
    let count = u32_at(data, 4)? as usize;
    if 8 + count.saturating_mul(8) > data.len() {
        return Err(Gxt2Error::Invalid(format!("{count} entries do not fit")));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let hash = u32_at(data, 8 + i * 8)?;
        let offset = u32_at(data, 12 + i * 8)? as usize;
        let tail = data
            .get(offset..)
            .ok_or_else(|| Gxt2Error::Invalid(format!("string offset {offset:#x} beyond the end")))?;
        let end = tail.iter().position(|&b| b == 0).ok_or_else(|| Gxt2Error::Invalid("unterminated string".into()))?;
        out.push(Entry { hash, text: String::from_utf8_lossy(&tail[..end]).into_owned() });
    }
    Ok(out)
}

/// Writes a text table; entries are sorted by hash as the game expects.
pub fn write(entries: &[Entry]) -> Vec<u8> {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by_key(|e| e.hash);
    let strings_start = 8 + sorted.len() * 8 + 8;
    let mut strings = Vec::new();
    let mut table = Vec::with_capacity(sorted.len() * 8);
    for e in &sorted {
        table.extend(e.hash.to_le_bytes());
        table.extend(((strings_start + strings.len()) as u32).to_le_bytes());
        strings.extend(e.text.as_bytes());
        strings.push(0);
    }
    let mut out = Vec::with_capacity(strings_start + strings.len());
    out.extend(MAGIC);
    out.extend((sorted.len() as u32).to_le_bytes());
    out.extend(table);
    out.extend(MAGIC);
    out.extend(((strings_start + strings.len()) as u32).to_le_bytes());
    out.extend(strings);
    out
}

/// The OpenIV text form. `name` returns a label's name when it is known.
pub fn to_oxt(entries: &[Entry], name: impl Fn(u32) -> Option<String>) -> String {
    let mut out = String::from("Version 2 30\n{\n");
    for e in entries {
        let label = name(e.hash).unwrap_or_else(|| format!("0x{:08X}", e.hash));
        out.push_str(&format!("\t{label} = {}\n", escape(&e.text)));
    }
    out.push_str("}\n");
    out
}

/// Reads the OpenIV text form; labels are `0x…` hashes or names (hashed with joaat).
pub fn from_oxt(text: &str) -> Result<Vec<Entry>, Gxt2Error> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "{" || trimmed == "}" || trimmed.starts_with("Version ") {
            continue;
        }
        let (label, value) = line
            .split_once(" = ")
            .or_else(|| line.split_once('='))
            .ok_or_else(|| Gxt2Error::Invalid(format!("line {}: expected `label = text`", n + 1)))?;
        let label = label.trim();
        let hash = match label.strip_prefix("0x").or_else(|| label.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16)
                .map_err(|_| Gxt2Error::Invalid(format!("line {}: bad hash {label}", n + 1)))?,
            None if !label.is_empty() => joaat(label),
            None => return Err(Gxt2Error::Invalid(format!("line {}: empty label", n + 1))),
        };
        out.push(Entry { hash, text: unescape(value) });
    }
    Ok(out)
}

/// Makes a string fit on one line: `\` → `\\`, line feed → `\n`, carriage return → `\r`.
pub fn escape(text: &str) -> String {
    if !text.contains(['\\', '\n', '\r']) {
        return text.to_owned();
    }
    text.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r")
}

/// Reverses [`escape`]; other backslashes stay as they are.
pub fn unescape(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n') => {
                    chars.next();
                    out.push('\n');
                    continue;
                }
                Some('r') => {
                    chars.next();
                    out.push('\r');
                    continue;
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let entries = vec![
            Entry { hash: 0x48455968, text: "Defend your property".into() },
            Entry { hash: 0x401DCCA0, text: "Property defense failed.".into() },
            Entry { hash: 0xE2BD3506, text: "Übung ~r~gemacht~s~".into() },
        ];
        let data = write(&entries);
        assert_eq!(&data[..4], MAGIC);
        let back = parse(&data).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].hash, 0x401DCCA0, "sorted by hash");
        assert_eq!(write(&back), data);

        let oxt = to_oxt(&back, |h| (h == 0x48455968).then(|| "MY_LABEL".to_owned()));
        assert!(oxt.starts_with("Version 2 30\n{\n\t0x401DCCA0 = Property defense failed.\n"), "{oxt}");
        let again = from_oxt(&oxt).unwrap();
        assert_eq!(again[1].hash, joaat("MY_LABEL"));
        // The test label's hash differs from the original; keep the original otherwise.
        assert_eq!(again[0], back[0]);
        assert_eq!(again[2], back[2]);
    }

    #[test]
    fn escapes_line_breaks() {
        for s in ["a\nb", "c:\\path\\n", "x\r\ny", "plain", "\\"] {
            let e = escape(s);
            assert!(!e.contains('\n'));
            assert_eq!(unescape(&e), s);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"nope").is_err());
        assert!(from_oxt("Version 2 30\n{\n\tno equals sign\n}").is_err());
    }
}
