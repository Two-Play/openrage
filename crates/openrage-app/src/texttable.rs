//! Text tables (`.gxt2`) and audio name tables (`.nametable`) as editable text.
//!
//! `.gxt2` becomes OpenIV's `.oxt` form (`0xHASH = text`, or the label's name
//! when it is known); `.nametable` one name per line. Both convert back exactly.

use anyhow::{Result, bail};
use rage_core::hash::joaat;
use rage_formats::gxt2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `.gxt2`, shown as `.oxt`.
    Gxt2,
    /// `.nametable`: NUL-separated names.
    NameTable,
}

impl Kind {
    pub fn of(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".gxt2") {
            Some(Self::Gxt2)
        } else if lower.ends_with(".nametable") {
            Some(Self::NameTable)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Gxt2 => "GXT2 text table",
            Self::NameTable => "name table",
        }
    }

    /// The text form's file extension, e.g. for exports.
    pub fn text_extension(self) -> &'static str {
        match self {
            Self::Gxt2 => "oxt",
            Self::NameTable => "txt",
        }
    }
}

pub fn is_text_table(name: &str) -> bool {
    Kind::of(name).is_some()
}

/// A label's name, if the name is known and really hashes to it.
fn label_name(hash: u32) -> Option<String> {
    let name = rage_formats::names::value(hash);
    (!name.starts_with("hash_") && joaat(&name) == hash).then_some(name)
}

/// The file as text, plus how many strings or names it has.
pub fn to_text(kind: Kind, data: &[u8]) -> Result<(String, usize)> {
    Ok(match kind {
        Kind::Gxt2 => {
            let entries = gxt2::parse(data)?;
            (gxt2::to_oxt(&entries, label_name), entries.len())
        }
        Kind::NameTable if data.is_empty() => (String::new(), 0),
        Kind::NameTable => {
            let names: Vec<String> = data
                .strip_suffix(&[0])
                .unwrap_or(data)
                .split(|&b| b == 0)
                .map(|s| gxt2::escape(&String::from_utf8_lossy(s)))
                .collect();
            let n = names.len();
            (names.join("\n") + "\n", n)
        }
    })
}

/// Converts edited text back to the file format.
pub fn from_text(kind: Kind, text: &str) -> Result<Vec<u8>> {
    Ok(match kind {
        Kind::Gxt2 => gxt2::write(&gxt2::from_oxt(text)?),
        Kind::NameTable => {
            let mut out = Vec::new();
            for line in text.lines() {
                if line.contains('\0') {
                    bail!("names cannot contain NUL characters");
                }
                out.extend(gxt2::unescape(line).as_bytes());
                out.push(0);
            }
            out
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_table_round_trip() {
        let data = b"CBR2_Train\0xm4_cbr6_agg\0".to_vec();
        let (text, n) = to_text(Kind::NameTable, &data).unwrap();
        assert_eq!(n, 2);
        assert_eq!(text, "CBR2_Train\nxm4_cbr6_agg\n");
        assert_eq!(from_text(Kind::NameTable, &text).unwrap(), data);
    }

    #[test]
    fn gxt2_round_trip() {
        let data = gxt2::write(&[gxt2::Entry { hash: 7, text: "Hello".into() }]);
        let (text, n) = to_text(Kind::Gxt2, &data).unwrap();
        assert_eq!(n, 1);
        assert_eq!(from_text(Kind::Gxt2, &text).unwrap(), data);
    }
}
