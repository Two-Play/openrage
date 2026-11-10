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
}

impl Layout {
    pub fn texture_size(self) -> usize {
        match self {
            Self::Legacy => 144,
            Self::Enhanced => 128,
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
    let format = Format::from_d3d9(code).ok_or_else(|| bad(format!("{name}: unknown D3D9 format {code:#x}")))?;
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
