//! GTA IV compiled scripts (`.sco`): the header only.
//!
//! A 24-byte header — magic (`SCR\x0D` or `scr\x0E`), code size, number of
//! static variables, number of global variables, flags, signature — followed
//! by the code and the variables. Disassembly is not supported yet.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoHeader {
    /// `SCR\x0D` or `scr\x0E`.
    pub magic: [u8; 4],
    pub code_size: u32,
    pub statics: u32,
    pub globals: u32,
    pub flags: u32,
    pub signature: u32,
}

impl ScoHeader {
    /// The magic as text, e.g. `scr 0x0E`.
    pub fn format_name(&self) -> String {
        format!("{} 0x{:02X}", String::from_utf8_lossy(&self.magic[..3]), self.magic[3])
    }
}

/// Reads the header of a script; `None` if `data` is not one.
pub fn header(data: &[u8]) -> Option<ScoHeader> {
    let magic: [u8; 4] = data.get(0..4)?.try_into().ok()?;
    if !matches!(&magic, b"SCR\x0D" | b"scr\x0E") || data.len() < 24 {
        return None;
    }
    let w = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    Some(ScoHeader { magic, code_size: w(4), statics: w(8), globals: w(12), flags: w(16), signature: w(20) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_header() {
        let mut d = b"scr\x0E".to_vec();
        for w in [14u32, 7, 0, 7, 0x7E20_4400] {
            d.extend_from_slice(&w.to_le_bytes());
        }
        let h = header(&d).unwrap();
        assert_eq!((h.code_size, h.statics, h.globals, h.signature), (14, 7, 0, 0x7E20_4400));
        assert_eq!(h.format_name(), "scr 0x0E");
        assert!(header(b"RSC\x05aaaaaaaaaaaaaaaaaaaaaaaa").is_none());
    }
}
