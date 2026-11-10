//! Text / hex rendering of file contents for the preview pane.

use std::fmt::Write;

const MAX_TEXT_BYTES: usize = 192 * 1024;
const MAX_TEXT_LINES: usize = 4000;
const MAX_HEX_BYTES: usize = 32 * 1024;

/// Renders `data` as text if it looks like text, otherwise as a hex dump.
pub fn render(data: &[u8]) -> String {
    match rage_core::text::decode_text(data, MAX_TEXT_BYTES) {
        Some(text) => truncate_text(text),
        None => hex_dump(data),
    }
}

fn truncate_text(text: String) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT_BYTES));
    for (i, line) in text.lines().enumerate() {
        if i == MAX_TEXT_LINES {
            out.push_str("\n… (preview truncated)");
            return out;
        }
        out.push_str(&line.replace('\t', "    "));
        out.push('\n');
    }
    if text.len() >= MAX_TEXT_BYTES {
        out.push_str("… (preview truncated)");
    }
    out
}

/// Classic 16-bytes-per-line hex dump with ASCII column.
pub fn hex_dump(data: &[u8]) -> String {
    let shown = &data[..data.len().min(MAX_HEX_BYTES)];
    let mut out = String::with_capacity(shown.len() * 4 + 64);
    for (line, chunk) in shown.chunks(16).enumerate() {
        let _ = write!(out, "{:08x}  ", line * 16);
        for i in 0..16 {
            match chunk.get(i) {
                Some(b) => {
                    let _ = write!(out, "{b:02x} ");
                }
                None => out.push_str("   "),
            }
            if i == 7 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        out.extend(chunk.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }));
        out.push_str("|\n");
    }
    if data.len() > shown.len() {
        let _ = write!(out, "… ({} more bytes)", data.len() - shown.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_text_and_binary() {
        assert!(render(b"<?xml version=\"1.0\"?>\n<a/>").starts_with("<?xml"));
        assert!(render(&[0u8, 1, 2, 3]).starts_with("00000000  00 01 02 03"));
        let utf16: Vec<u8> =
            [0xFF, 0xFE].into_iter().chain("hi".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert!(render(&utf16).starts_with("hi"));
    }
}
