//! Heuristic text detection and decoding for game data files.

/// Decodes `data` (up to `max_bytes`) as text if it looks like text:
/// UTF-16LE with BOM, or UTF-8 / ASCII (optional BOM) without NUL bytes and
/// with few control characters. Returns `None` for binary data.
pub fn decode_text(data: &[u8], max_bytes: usize) -> Option<String> {
    if let Some(rest) = data.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> =
            rest[..rest.len().min(max_bytes)].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return Some(String::from_utf16_lossy(&units));
    }
    let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    let sample = &data[..data.len().min(8192)];
    if sample.is_empty() || sample.contains(&0) {
        return None;
    }
    let control = sample.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r')).count();
    if control * 100 > sample.len() {
        return None;
    }
    let text = String::from_utf8_lossy(&data[..data.len().min(max_bytes)]);
    let replaced = text.chars().take(8192).filter(|&c| c == char::REPLACEMENT_CHARACTER).count();
    (replaced * 50 <= sample.len()).then(|| text.into_owned())
}

/// Identifier-like tokens (`[A-Za-z_][A-Za-z0-9_]{2,63}`), the kind of names
/// RAGE stores as hashes elsewhere.
pub fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| (3..=64).contains(&t.len()) && !t.as_bytes()[0].is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_text() {
        assert_eq!(decode_text(b"<a/>", 100).as_deref(), Some("<a/>"));
        assert_eq!(decode_text(&[0, 1, 2], 100), None);
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi", 100).as_deref(), Some("hi"));
    }

    #[test]
    fn extracts_identifiers() {
        let ids: Vec<&str> = identifiers("<Item>WEAPON_PISTOL</Item> 0x12 ab 3d_x prop_bench_01").collect();
        assert_eq!(ids, ["Item", "WEAPON_PISTOL", "Item", "prop_bench_01"]);
    }
}
