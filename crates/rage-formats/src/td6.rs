//! GTA VI texture dictionaries (`.#td`).
//!
//! Layout of the system part (pointers into it start at `0x5000_0000`, into
//! the graphics part at `0x6000_0000`): name hashes at 32 and texture
//! pointers at 48, each a pointer and a 16-bit count. A texture has, from its
//! start: at 8 the size of its data in elements (pixels, or 4×4 blocks), at 24 the pointer
//! to it, at 48 8 bytes before its name, at 56 width, height and slices (16 bits each), then
//! a byte each for the shape (1: 2D, 2: cube map, 3: volume) and the pixel
//! format (DXGI), at 66 the number of mip levels (7 bits).
//!
//! The data is tiled like that of the texture pools ([`crate::ttpf`]), which
//! tile size is told by its length. The slices of an array follow each other.

use rage_core::bytes::LeRead;

use crate::texture::{Dimension, Format, Texture};
use crate::ttpf::Tiling;
use crate::ytd::{Layout, TextureDictionary};

const SYSTEM: u64 = 0x5000_0000;
const GRAPHICS: u64 = 0x6000_0000;
const PART: u64 = 0x1000_0000;

#[derive(Debug, thiserror::Error)]
#[error("not a GTA VI texture dictionary: {0}")]
pub struct TdError(&'static str);

fn system_pointer(system: &[u8], at: usize) -> Option<usize> {
    let p = system.u64_at(at)?;
    (SYSTEM..SYSTEM + PART).contains(&p).then(|| (p - SYSTEM) as usize)
}

/// Reads the textures of a dictionary: the largest level of each, of every
/// slice of an array and face of a cube map. Those that cannot be read are
/// listed as errors.
pub fn parse(system: &[u8], graphics: &[u8]) -> Result<TextureDictionary, TdError> {
    if system.len() < 64 || system.u64_at(0) != Some(0) || system_pointer(system, 8).is_none() {
        return Err(TdError("no resource header"));
    }
    let count = usize::from(system.u16_at(56).unwrap_or(0));
    let list = match system_pointer(system, 48) {
        Some(list) => list,
        None if count == 0 => 0,
        None => return Err(TdError("no list of textures")),
    };
    if list + count * 8 > system.len() {
        return Err(TdError("the list of textures is cut off"));
    }
    let mut dict = TextureDictionary {
        textures: Vec::with_capacity(count),
        errors: Vec::new(),
        layout: Layout::Enhanced,
        version: 0,
        root: Vec::new(),
        stored: Vec::new(),
    };
    for i in 0..count {
        let at = system_pointer(system, list + i * 8);
        let name = at.and_then(|at| name(system, at)).unwrap_or_else(|| format!("texture_{i}"));
        match at.map(|at| texture(system, graphics, at, name.clone())) {
            Some(Ok(texture)) => dict.textures.push(texture),
            Some(Err(reason)) => dict.errors.push(format!("{name}: {reason}")),
            None => dict.errors.push(format!("{name}: no such texture")),
        }
    }
    Ok(dict)
}

fn name(system: &[u8], at: usize) -> Option<String> {
    let bytes = system.get(system_pointer(system, at + 48)? + 8..)?;
    let end = bytes.iter().take(256).position(|b| *b == 0)?;
    let name = std::str::from_utf8(&bytes[..end]).ok()?;
    (!name.is_empty() && !name.chars().any(char::is_control)).then(|| name.to_owned())
}

fn texture(system: &[u8], graphics: &[u8], at: usize, name: String) -> Result<Texture, String> {
    let record = system.get(at..at + 80).ok_or("its record is cut off")?;
    let u16_at = |at: usize| u32::from(record.u16_at(at).unwrap_or(0));
    let (width, height, slices) = (u16_at(56), u16_at(58), u16_at(60));
    let (shape, format, mips) = (record[62], u32::from(record[63]), u32::from(record[66] & 0x7f));
    let element = crate::ttpf::element_bytes(format).ok_or_else(|| format!("unknown pixel format {format}"))?;
    let size = record.u32_at(8).unwrap_or(0) as usize * element;
    if width == 0 || height == 0 || !(1..=4096).contains(&slices) {
        return Err("impossible size".into());
    }
    let dimension = match shape {
        1 => Dimension::Texture2D,
        2 => Dimension::Cube,
        _ => return Err("a volume, which is not read yet".into()),
    };
    let offset = record.u64_at(24).and_then(|p| p.checked_sub(GRAPHICS)).filter(|o| *o < PART);
    let offset = offset.ok_or("its data is not in this file")? as usize;
    let data = graphics.get(offset..).and_then(|d| d.get(..size)).ok_or("its data is cut off")?;
    let tiling = Tiling::by_size(width, height, mips, format, slices, size).ok_or("its layout is not understood")?;
    let mut rows = Vec::new();
    for slice in 0..slices as usize {
        rows.extend(tiling.largest(data, slice).ok_or("its data is cut off")?);
    }
    Ok(Texture {
        name,
        width,
        height,
        depth: slices,
        levels: 1,
        format: Format(format),
        dimension,
        usage: 0,
        data: rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary(shape: u8) -> (Vec<u8>, Vec<u8>) {
        let mut system = vec![0u8; 0x100];
        let put = |s: &mut Vec<u8>, at: usize, v: u64| s[at..at + 8].copy_from_slice(&v.to_le_bytes());
        put(&mut system, 8, SYSTEM + 0xf0);
        put(&mut system, 48, SYSTEM + 0xe0);
        system[56] = 1;
        put(&mut system, 0xe0, SYSTEM + 0x40);
        // 64×64 pixels of 4 bytes: 4 tiles of 4 KiB.
        put(&mut system, 0x48, 0x1000);
        put(&mut system, 0x58, GRAPHICS);
        put(&mut system, 0x70, SYSTEM + 0xa0);
        system[0x78..0x82].copy_from_slice(&[64, 0, 64, 0, 1, 0, shape, 28, 5, 0]);
        system[0x82] = 1;
        system[0xa8..0xac].copy_from_slice(b"tile");
        let mut graphics = vec![0u8; 0x4000];
        // The first pixel of the second tile is the 33rd of the first row.
        graphics[0x1000..0x1004].copy_from_slice(&[1, 2, 3, 4]);
        (system, graphics)
    }

    #[test]
    fn reads_a_tiled_texture() {
        let (system, graphics) = dictionary(1);
        let dict = parse(&system, &graphics).unwrap();
        let t = &dict.textures[0];
        assert_eq!((t.name.as_str(), t.width, t.height, t.data.len()), ("tile", 64, 64, 0x4000));
        assert_eq!(t.data[32 * 4..33 * 4], [1, 2, 3, 4]);
    }

    #[test]
    fn damaged_dictionaries_are_errors() {
        let (system, graphics) = dictionary(3);
        assert_eq!(parse(&system, &graphics).unwrap().errors.len(), 1);
        let (system, graphics) = dictionary(1);
        assert_eq!(parse(&system, &graphics[..100]).unwrap().errors.len(), 1);
        assert!(parse(&system[..40], &graphics).is_err());
        assert!(parse(&system[..0xe4], &graphics).is_err());
    }
}
