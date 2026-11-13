//! Texture dictionaries (`.ytd`).
//!
//! Root structure (`rage::pgDictionary<grcTexture>`, 64 bytes, both layouts):
//! `0x20` name-hash list (pointer, count), `0x30` texture pointer list (pointer, count).
//!
//! Legacy textures (`grcTexturePC`, resource version 13) keep D3D9 format codes:
//! `0x28` name, `0x40` usage, `0x50` width, height, depth, stride, `0x58` format,
//! `0x5D` mip levels, `0x70` data pointer.
//!
//! Enhanced textures (`rage::sga::Texture`, resource version 5) use DXGI formats:
//! `0x18` width, height, depth, `0x1E` dimension, `0x1F` format, `0x22` mip levels,
//! `0x28` name, `0x38` data pointer, `0x40` usage.
//!
//! Red Dead Redemption 2 (RSC8, resource version 2) uses the same structure,
//! 0xB0 bytes long, with `0x08` block count and `0x0C` bytes per block. Its
//! pixel data is padded: each mip level's grid of blocks (pixels for
//! uncompressed formats) is rounded up to a multiple of 8 per axis (16 when a
//! block is one byte), after rounding the size up to a power of two when the
//! texture has mips; arrays, cube maps and volumes with mips reserve a power
//! of two of slices. Single-level one-byte textures pad their rows to
//! multiples of 8 and their width irregularly (as do a few render targets);
//! their row pitch follows from the stored size.
//! Reading repacks the data tightly, so the rest of OpenRAGE sees the same
//! layout as for GTA V. (Based on texfury by Hancapo, MIT, and checked
//! against the block counts of every texture in the game.)
//!
//! GTA IV (`.wtd`, RSC5 type 8) uses 32-bit pointers. Root: `0x10` name-hash
//! list (pointer, u16 count), `0x18` texture pointer list (pointer, u16 count).
//! Textures (`grcTexturePC`, 0x50 bytes): `0x14` name (`pack:/<name>.dds`),
//! `0x1C` width, height, `0x20` D3D9 format, `0x24` stride, `0x27` mip
//! levels, `0x48` data pointer (`0x44` in a few dictionaries of older builds).

use rage_resource::{Resource, ResourceError};

use crate::texture::{Dimension, Format, Texture};

/// Resource versions of texture dictionaries.
pub const VERSION_LEGACY: u32 = 13;
pub const VERSION_ENHANCED: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// GTA V Legacy (`grcTexturePC`, 144-byte textures, D3D9 formats).
    Legacy,
    /// GTA V Enhanced (`rage::sga::Texture`, 128-byte textures, DXGI formats).
    Enhanced,
    /// Red Dead Redemption 2 (`rage::sga::Texture`, 176-byte textures, padded mips).
    Rdr2,
    /// GTA IV (`grcTexturePC`, 80-byte textures, D3D9 formats, 32-bit pointers).
    Gta4,
}

impl Layout {
    pub fn texture_size(self) -> usize {
        match self {
            Self::Legacy => 144,
            Self::Enhanced => 128,
            Self::Rdr2 => 0xB0,
            Self::Gta4 => 0x50,
        }
    }
}

/// Where a texture came from inside the resource; needed to write the dictionary back.
#[derive(Debug, Clone)]
pub struct Stored {
    /// The texture structure as read (keeps fields OpenRAGE does not interpret).
    pub template: Vec<u8>,
    /// Address of the pixel data.
    pub data_ptr: u64,
}

#[derive(Debug, Clone)]
pub struct TextureDictionary {
    pub textures: Vec<Texture>,
    /// Textures that could not be read (name or index, reason).
    pub errors: Vec<String>,
    pub layout: Layout,
    pub version: u32,
    /// The 64-byte root structure as read.
    pub root: Vec<u8>,
    /// Parallel to `textures`.
    pub stored: Vec<Stored>,
}

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

/// Reads a texture dictionary.
pub fn parse(res: &Resource) -> Result<TextureDictionary, ResourceError> {
    match res.version {
        VERSION_ENHANCED => parse_layout(res, true),
        VERSION_LEGACY => parse_layout(res, false),
        // Rare older dictionaries: use whichever layout reads cleanly.
        _ => {
            let legacy = parse_layout(res, false);
            match legacy {
                Ok(d) if d.errors.is_empty() && !d.textures.is_empty() => Ok(d),
                _ => match parse_layout(res, true) {
                    Ok(d) if d.errors.is_empty() => Ok(d),
                    _ => Err(bad(format!("unsupported texture dictionary version {}", res.version))),
                },
            }
        }
    }
}

fn parse_layout(res: &Resource, enhanced: bool) -> Result<TextureDictionary, ResourceError> {
    parse_at(res, Resource::ROOT, enhanced)
}

/// Resource version of Red Dead Redemption 2 texture dictionaries.
pub const VERSION_RDR2: u32 = 2;

/// Reads a texture dictionary from Red Dead Redemption 2's archives: version 2
/// in RDR2's own layout; a few old ones there use GTA V's layouts.
pub fn parse_rdr2(res: &Resource) -> Result<TextureDictionary, ResourceError> {
    if res.version == VERSION_RDR2 { parse_rdr2_at(res, Resource::ROOT) } else { parse(res) }
}

/// Reads a Red Dead Redemption 2 texture dictionary located at `root`.
pub fn parse_rdr2_at(res: &Resource, root: u64) -> Result<TextureDictionary, ResourceError> {
    let list = res.u64(root + 0x30)?;
    let count = res.u16(root + 0x38)? as u64;
    if count > 4096 {
        return Err(bad(format!("implausible texture count {count}")));
    }
    let mut dict = TextureDictionary {
        textures: Vec::with_capacity(count as usize),
        errors: Vec::new(),
        layout: Layout::Rdr2,
        version: res.version,
        root: res.bytes(root, 64)?.to_vec(),
        stored: Vec::with_capacity(count as usize),
    };
    for i in 0..count {
        let result = res.u64(list + i * 8).and_then(|ptr| {
            let (tex, data_ptr) = read_rdr2(res, ptr)?;
            Ok((tex, Stored { template: res.bytes(ptr, Layout::Rdr2.texture_size())?.to_vec(), data_ptr }))
        });
        match result {
            Ok((t, s)) => {
                dict.textures.push(t);
                dict.stored.push(s);
            }
            Err(e) => dict.errors.push(format!("texture #{i}: {e}")),
        }
    }
    Ok(dict)
}

/// Reads a GTA IV texture dictionary (`.wtd`).
pub fn parse_gta4(res: &Resource) -> Result<TextureDictionary, ResourceError> {
    parse_gta4_at(res, Resource::ROOT)
}

/// Reads a GTA IV texture dictionary located at `root` (e.g. one embedded in a drawable).
pub fn parse_gta4_at(res: &Resource, root: u64) -> Result<TextureDictionary, ResourceError> {
    let list = res.u32(root + 0x18)? as u64;
    let count = res.u16(root + 0x1C)? as u64;
    if count > 4096 {
        return Err(bad(format!("implausible texture count {count}")));
    }
    let mut dict = TextureDictionary {
        textures: Vec::with_capacity(count as usize),
        errors: Vec::new(),
        layout: Layout::Gta4,
        version: res.version,
        root: res.bytes(root, 32)?.to_vec(),
        stored: Vec::with_capacity(count as usize),
    };
    for i in 0..count {
        let result = res.u32(list + i * 4).and_then(|ptr| {
            let ptr = ptr as u64;
            let (tex, data_ptr) = read_gta4(res, ptr)?;
            Ok((tex, Stored { template: res.bytes(ptr, Layout::Gta4.texture_size())?.to_vec(), data_ptr }))
        });
        match result {
            Ok((t, s)) => {
                dict.textures.push(t);
                dict.stored.push(s);
            }
            Err(e) => dict.errors.push(format!("texture #{i}: {e}")),
        }
    }
    Ok(dict)
}

/// The texture name without GTA IV's `pack:/` prefix and `.dds` suffix.
pub fn gta4_texture_name(stored: &str) -> &str {
    let name = stored.strip_prefix("pack:/").unwrap_or(stored);
    let len = name.len();
    if len > 4 && name[len - 4..].eq_ignore_ascii_case(".dds") { &name[..len - 4] } else { name }
}

fn read_gta4(res: &Resource, p: u64) -> Result<(Texture, u64), ResourceError> {
    let name = gta4_texture_name(&res.string(res.u32(p + 0x14)? as u64)?).to_owned();
    let code = res.u32(p + 0x20)?;
    let format = Format::from_d3d9(code).ok_or_else(|| bad(format!("{name}: unknown D3D9 format {code:#x}")))?;
    let mut tex = Texture {
        width: res.u16(p + 0x1C)? as u32,
        height: res.u16(p + 0x1E)? as u32,
        depth: 1,
        levels: res.u8(p + 0x27)?.max(1) as u32,
        format,
        dimension: Dimension::Texture2D,
        usage: 0,
        data: Vec::new(),
        name,
    };
    // A few dictionaries from older builds keep the data pointer 4 bytes earlier.
    let data_ptr = [0x48, 0x44]
        .into_iter()
        .map(|o| res.u32(p + o).map(u64::from))
        .find(|v| v.as_ref().map_or(true, |&v| v >= 0x6000_0000))
        .unwrap_or(Ok(0))?;
    read_data(res, &mut tex, data_ptr)?;
    Ok((tex, data_ptr))
}

/// Columns and rows of one mip level as stored in RDR2: blocks (pixels for
/// uncompressed formats), the real grid and the padded one.
pub fn rdr2_level_grid(format: Format, width: u32, height: u32, levels: u32, level: u32) -> Option<[usize; 4]> {
    let block = format.block_or_pixel_bytes() as usize;
    let align = if block == 1 { 16 } else { 8 };
    let (mut w, mut h) = (width.max(1), height.max(1));
    if levels > 1 {
        w = w.next_power_of_two();
        h = h.next_power_of_two();
    }
    let (lw, lh) = ((w >> level).max(1) as usize, (h >> level).max(1) as usize);
    let (rw, rh) = ((width >> level).max(1) as usize, (height >> level).max(1) as usize);
    let (cols, rows, real_cols, real_rows) = if format.block_bytes().is_some() {
        (lw.div_ceil(4), lh.div_ceil(4), rw.div_ceil(4), rh.div_ceil(4))
    } else {
        format.bits_per_pixel()?;
        (lw, lh, rw, rh)
    };
    Some([real_cols, real_rows, cols.next_multiple_of(align), rows.next_multiple_of(align)])
}

/// Blocks RDR2 stores for a texture (all levels of one slice).
pub fn rdr2_block_count(format: Format, width: u32, height: u32, levels: u32) -> Option<usize> {
    (0..levels).map(|l| rdr2_level_grid(format, width, height, levels, l).map(|[_, _, c, r]| c * r)).sum()
}

fn read_rdr2(res: &Resource, p: u64) -> Result<(Texture, u64), ResourceError> {
    let name = res.string(res.u64(p + 0x28)?)?;
    let depth = res.u16(p + 0x1C)? as u32;
    let dimension = match res.u8(p + 0x1E)? {
        2 => Dimension::Cube,
        3 => Dimension::Texture3D,
        _ => Dimension::Texture2D,
    };
    let mut tex = Texture {
        width: res.u16(p + 0x18)? as u32,
        height: res.u16(p + 0x1A)? as u32,
        depth: if dimension == Dimension::Cube { depth.max(6) } else { depth.max(1) },
        levels: res.u8(p + 0x22)?.max(1) as u32,
        format: Format(res.u8(p + 0x1F)? as u32),
        dimension,
        usage: (res.u32(p + 0x40)? & 0x1F) as u8,
        data: Vec::new(),
        name,
    };
    let data_ptr = res.u64(p + 0x38)?;
    let stride = res.u32(p + 0x0C)? as usize;
    let stored_blocks = res.u32(p + 0x08)? as usize;
    let unsupported = || bad(format!("{}: unsupported format {}", tex.name, tex.format.name()));
    let block = tex.format.block_or_pixel_bytes() as usize;
    if tex.format.block_bytes().is_none() && tex.format.bits_per_pixel().is_none() {
        return Err(unsupported());
    }
    let slices = tex.depth.max(1) as usize;
    let reserved_slices = if tex.levels > 1 { slices.next_power_of_two() } else { slices };
    let mut grids: Vec<[usize; 4]> = (0..tex.levels)
        .map(|l| rdr2_level_grid(tex.format, tex.width, tex.height, tex.levels, l))
        .collect::<Option<_>>()
        .ok_or_else(unsupported)?;
    let per_slice: usize = grids.iter().map(|[_, _, c, r]| c * r).sum();
    // Some single-level textures (one-byte masks, render targets) pad their
    // rows to 8 and their width irregularly: the pitch is what the size says.
    if per_slice * reserved_slices != stored_blocks && tex.levels == 1 {
        let [cols, rows, _, _] = grids[0];
        let padded_rows = rows.next_multiple_of(8);
        let per = stored_blocks / reserved_slices.max(1);
        if per.is_multiple_of(padded_rows) && per / padded_rows >= cols {
            grids[0] = [cols, rows, per / padded_rows, padded_rows];
        }
    }
    let per_slice: usize = grids.iter().map(|[_, _, c, r]| c * r).sum();
    if stride != block || per_slice * reserved_slices != stored_blocks {
        return Err(bad(format!(
            "{}: stored size {stored_blocks}×{stride} does not match {}×{} {} with {} levels and {slices} slices",
            tex.name,
            tex.width,
            tex.height,
            tex.format.name(),
            tex.levels
        )));
    }
    let src = res.bytes(data_ptr, stored_blocks * stride)?;
    let mut data = Vec::with_capacity(tex.expected_size().unwrap_or(0));
    let mut at = 0;
    // Slices one after another, each with its mip levels.
    for _ in 0..slices {
        for &[cols, rows, pcols, prows] in &grids {
            for r in 0..rows {
                let start = at + r * pcols * block;
                data.extend_from_slice(&src[start..start + cols * block]);
            }
            at += pcols * prows * block;
        }
    }
    tex.data = data;
    Ok((tex, data_ptr))
}

/// Reads a texture dictionary located at `root` (e.g. one embedded in a drawable).
pub fn parse_at(res: &Resource, root: u64, enhanced: bool) -> Result<TextureDictionary, ResourceError> {
    let list = res.u64(root + 0x30)?;
    let count = res.u16(root + 0x38)? as u64;
    if count > 4096 {
        return Err(bad(format!("implausible texture count {count}")));
    }
    let layout = if enhanced { Layout::Enhanced } else { Layout::Legacy };
    let mut dict = TextureDictionary {
        textures: Vec::with_capacity(count as usize),
        errors: Vec::new(),
        layout,
        version: res.version,
        root: res.bytes(root, 64)?.to_vec(),
        stored: Vec::with_capacity(count as usize),
    };
    for i in 0..count {
        let result = res.u64(list + i * 8).and_then(|ptr| {
            let (tex, data_ptr) = if enhanced { read_enhanced(res, ptr)? } else { read_legacy(res, ptr)? };
            Ok((tex, Stored { template: res.bytes(ptr, layout.texture_size())?.to_vec(), data_ptr }))
        });
        match result {
            Ok((t, s)) => {
                dict.textures.push(t);
                dict.stored.push(s);
            }
            Err(e) => dict.errors.push(format!("texture #{i}: {e}")),
        }
    }
    Ok(dict)
}

fn read_data(res: &Resource, tex: &mut Texture, ptr: u64) -> Result<(), ResourceError> {
    let size =
        tex.expected_size().ok_or_else(|| bad(format!("{}: unsupported format {}", tex.name, tex.format.name())))?;
    tex.data = res.bytes(ptr, size)?.to_vec();
    Ok(())
}

fn read_legacy(res: &Resource, p: u64) -> Result<(Texture, u64), ResourceError> {
    let name = res.string(res.u64(p + 0x28)?)?;
    let code = res.u32(p + 0x58)?;
    // A few old dictionaries in RDR2 use this layout with a DXGI code in the low byte.
    let format = Format::from_d3d9(code)
        .or_else(|| {
            let dxgi = Format(code & 0xFF);
            (code > 0xFF && (dxgi.block_bytes().is_some() || dxgi.bits_per_pixel().is_some())).then_some(dxgi)
        })
        .ok_or_else(|| bad(format!("{name}: unknown D3D9 format {code:#x}")))?;
    let depth = res.u16(p + 0x54)? as u32;
    let mut tex = Texture {
        width: res.u16(p + 0x50)? as u32,
        height: res.u16(p + 0x52)? as u32,
        depth: depth.max(1),
        levels: res.u8(p + 0x5D)?.max(1) as u32,
        format,
        dimension: Dimension::Texture2D,
        usage: (res.u32(p + 0x40)? & 0x1F) as u8,
        data: Vec::new(),
        name,
    };
    let data_ptr = res.u64(p + 0x70)?;
    read_data(res, &mut tex, data_ptr)?;
    Ok((tex, data_ptr))
}

fn read_enhanced(res: &Resource, p: u64) -> Result<(Texture, u64), ResourceError> {
    let name = res.string(res.u64(p + 0x28)?)?;
    let depth = res.u16(p + 0x1C)? as u32;
    let dimension = match res.u8(p + 0x1E)? {
        2 => Dimension::Texture3D,
        3 => Dimension::Cube,
        _ => Dimension::Texture2D,
    };
    let mut tex = Texture {
        width: res.u16(p + 0x18)? as u32,
        height: res.u16(p + 0x1A)? as u32,
        depth: if dimension == Dimension::Cube { depth.max(6) } else { depth.max(1) },
        levels: res.u8(p + 0x22)?.max(1) as u32,
        format: Format(res.u8(p + 0x1F)? as u32),
        dimension,
        usage: (res.u32(p + 0x40)? & 0x1F) as u8,
        data: Vec::new(),
        name,
    };
    let data_ptr = res.u64(p + 0x38)?;
    read_data(res, &mut tex, data_ptr)?;
    Ok((tex, data_ptr))
}

#[cfg(test)]
mod rdr2_tests {
    use super::*;

    /// Block counts the game stores, from real RDR2 textures.
    #[test]
    fn rdr2_padding_matches_the_game() {
        let cases = [
            (Format::BC1_UNORM, 256, 256, 7, 5632),
            (Format::BC3_UNORM, 128, 128, 6, 1536),
            (Format::BC3_UNORM_SRGB, 1024, 256, 7, 22016),
            (Format::BC7_UNORM, 1920, 1080, 1, 130_560),
            (Format::BC3_UNORM, 4, 4, 1, 64),
            (Format::A8_UNORM, 4, 4, 1, 256),
            (Format::B8G8R8A8_UNORM, 128, 128, 4, 0x5500),
        ];
        for (format, w, h, levels, blocks) in cases {
            assert_eq!(rdr2_block_count(format, w, h, levels), Some(blocks), "{} {w}×{h} {levels}", format.name());
        }
    }
}

#[cfg(test)]
mod gta4_tests {
    use super::*;

    /// A one-texture GTA IV dictionary: 4×4 DXT1, one level.
    fn dictionary(data_offset: usize) -> Resource {
        let mut sys = vec![0u8; 0x100];
        let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut sys, 0x18, 0x5000_0040); // texture list
        sys[0x1C] = 1; // count
        put(&mut sys, 0x40, 0x5000_0080); // list[0]
        let t = 0x80;
        put(&mut sys, t + 0x14, 0x5000_00E0); // name
        sys[t + 0x1C] = 4; // width
        sys[t + 0x1E] = 4; // height
        put(&mut sys, t + 0x20, 0x3154_5844); // DXT1
        sys[t + 0x27] = 1; // levels
        put(&mut sys, t + data_offset, 0x6000_0000);
        sys[0xE0..0xE0 + 15].copy_from_slice(b"pack:/stone.dds");
        Resource { version: 8, system: sys, graphics: vec![0xAB; 8] }
    }

    #[test]
    fn reads_gta4_textures() {
        for offset in [0x48, 0x44] {
            let dict = parse_gta4(&dictionary(offset)).unwrap();
            assert!(dict.errors.is_empty(), "{:?}", dict.errors);
            let t = &dict.textures[0];
            assert_eq!((t.name.as_str(), t.width, t.height, t.levels), ("stone", 4, 4, 1));
            assert_eq!(t.format, Format::BC1_UNORM);
            assert_eq!(t.data, vec![0xAB; 8]);
        }
    }

    #[test]
    fn strips_pack_prefix_and_extension() {
        assert_eq!(gta4_texture_name("pack:/a_b.DDS"), "a_b");
        assert_eq!(gta4_texture_name("plain"), "plain");
    }
}
