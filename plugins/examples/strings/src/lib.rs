//! Example plugin: finds the readable strings in a file (like the Unix
//! `strings` tool), both plain ASCII and UTF-16 (as used by many game files).

use openrage_plugin_sdk::file_name;
use openrage_plugin_sdk::prelude::*;

/// Shortest run of printable characters that counts as a string.
const MIN_LEN: usize = 4;
/// Most strings shown in the preview.
const PREVIEW_LIMIT: usize = 5000;

struct Strings;

impl Plugin for Strings {
    fn preview(_vpath: String, data: Vec<u8>) -> Result<Preview, String> {
        let found = find_strings(&data);
        let shown: Vec<String> =
            found.iter().take(PREVIEW_LIMIT).map(|s| format!("{:08X}  {}", s.offset, s.text)).collect();
        let mut text = shown.join("\n");
        if found.len() > PREVIEW_LIMIT {
            text.push_str(&format!("\n… {} more (export them all from the context menu)", found.len() - PREVIEW_LIMIT));
        }
        Ok(Preview::text(text).with("Strings", found.len()).with("UTF-16", found.iter().filter(|s| s.utf16).count()))
    }

    fn run_action(action: String, inputs: Vec<Input>) -> Result<Vec<OutputFile>, String> {
        if action != "export" {
            return Err(format!("unknown action {action:?}"));
        }
        Ok(inputs
            .iter()
            .map(|input| {
                let mut text = String::new();
                for s in find_strings(&input.data) {
                    text.push_str(&s.text);
                    text.push('\n');
                }
                OutputFile::new(format!("{}.strings.txt", file_name(&input.vpath)), text)
            })
            .collect())
    }
}

openrage_plugin_sdk::export_plugin!(Strings);

#[derive(Debug, PartialEq)]
struct Found {
    offset: usize,
    text: String,
    utf16: bool,
}

fn printable(c: u16) -> bool {
    (0x20..0x7F).contains(&c) || c == u16::from(b'\t')
}

/// ASCII runs and UTF-16LE runs (of ASCII characters), in file order.
fn find_strings(data: &[u8]) -> Vec<Found> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &b) in data.iter().chain(std::iter::once(&0)).enumerate() {
        if !printable(u16::from(b)) {
            if i - start >= MIN_LEN {
                out.push(Found {
                    offset: start,
                    text: String::from_utf8_lossy(&data[start..i]).into_owned(),
                    utf16: false,
                });
            }
            start = i + 1;
        }
    }
    for parity in 0..2 {
        let mut run = String::new();
        let mut run_start = parity;
        let mut i = parity;
        while i + 1 < data.len() + 2 {
            let c = if i + 1 < data.len() { u16::from_le_bytes([data[i], data[i + 1]]) } else { 0 };
            if c < 0x80 && printable(c) && data.get(i + 1) == Some(&0) {
                if run.is_empty() {
                    run_start = i;
                }
                run.push(c as u8 as char);
            } else {
                if run.len() >= MIN_LEN {
                    out.push(Found { offset: run_start, text: std::mem::take(&mut run), utf16: true });
                }
                run.clear();
            }
            i += 2;
        }
    }
    out.sort_by_key(|s| s.offset);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ascii_and_utf16() {
        let mut data = b"\x00\x01hello world\x00ab\x01\x01".to_vec();
        data.extend("Name".encode_utf16().flat_map(u16::to_le_bytes));
        data.extend([0, 0]);
        let found = find_strings(&data);
        let texts: Vec<_> = found.iter().map(|s| (s.text.as_str(), s.utf16)).collect();
        assert_eq!(texts, [("hello world", false), ("Name", true)]);
        assert_eq!(found[0].offset, 2);
        assert_eq!(found[1].offset, 18);
    }
}
