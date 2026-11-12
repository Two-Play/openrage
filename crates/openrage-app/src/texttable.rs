//! Text tables (`.gxt2`) and audio name tables (`.nametable`) as editable text.
//!
//! `.gxt2` becomes OpenIV's `.oxt` form (`0xHASH = text`, or the label's name
//! when it is known); `.nametable` one name per line. Both convert back exactly.
//! Red Dead Redemption 2's text tables (`.yldb`) use the same text form, for
//! reading only.

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

/// Whether a file is a Red Dead Redemption 2 text table (`.yldb`).
pub fn is_rdr2_text_table(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".yldb")
}

/// A Red Dead Redemption 2 text table (`.yldb`) in the `.oxt` text form, plus
/// its number of strings.
pub fn rdr2_to_text(res: &rage_resource::Resource) -> Result<(String, usize)> {
    let entries = rage_formats::ldb::parse(res)?;
    Ok((gxt2::to_oxt(&entries, label_name), entries.len()))
}

/// Reads the Red Dead Redemption 2 text table at entry `i` as text.
pub fn rdr2_entry_to_text(rpf: &rage_rpf::Rpf8, i: usize) -> Result<(String, usize)> {
    let entry = rpf.entries().and_then(|es| es.get(i)).ok_or_else(|| anyhow::anyhow!("no entry {i}"))?;
    rdr2_to_text(&rpf.read_resource(entry)?)
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
    fn rdr2_tables_use_the_oxt_form() {
        let mut system = vec![0u8; 0x80];
        let put = |s: &mut Vec<u8>, at: usize, v: u64| s[at..at + 8].copy_from_slice(&v.to_le_bytes());
        put(&mut system, 0x10, 0x5000_0030); // one bucket
        put(&mut system, 0x18, 1);
        put(&mut system, 0x20, 1);
        put(&mut system, 0x30, 0x5000_0040); // node
        put(&mut system, 0x40, 0xABCD);
        put(&mut system, 0x48, 0x5000_0058); // value
        put(&mut system, 0x68, 0x5000_0078); // string
        put(&mut system, 0x70, 3);
        system[0x78..0x7A].copy_from_slice(b"Hi");
        let res = rage_resource::Resource { version: 1, system, graphics: Vec::new() };
        let (text, n) = rdr2_to_text(&res).unwrap();
        assert_eq!(n, 1);
        assert_eq!(text, "Version 2 30\n{\n\t0x0000ABCD = Hi\n}\n");
        assert!(is_rdr2_text_table("data/lang/global.YLDB"));
    }

    #[test]
    fn gxt2_round_trip() {
        let data = gxt2::write(&[gxt2::Entry { hash: 7, text: "Hello".into() }]);
        let (text, n) = to_text(Kind::Gxt2, &data).unwrap();
        assert_eq!(n, 1);
        assert_eq!(from_text(Kind::Gxt2, &text).unwrap(), data);
    }
}
