//! DDS export, matching what OpenIV and CodeWalker write: classic FourCC /
//! bit-mask pixel formats where Direct3D 9 can express them, a DX10 header
//! otherwise (BC6H, BC7, sRGB and other DXGI-only formats).

use crate::texture::{Dimension, Format, Texture};

/// Why a DDS file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DdsError {
    #[error("not a DDS file")]
    NotDds,
    #[error("truncated DX10 header")]
    TruncatedHeader,
    #[error("unsupported DDS FourCC {0:?}")]
    UnsupportedFourCc(String),
    #[error("unsupported DDS pixel format (flags {flags:#x}, {bits} bits)")]
    UnsupportedPixelFormat { flags: u32, bits: u32 },
    #[error("unsupported format {0}")]
    UnsupportedFormat(String),
    #[error("DDS data too short: {have} bytes, {need} expected")]
    TooShort { have: usize, need: usize },
}

const DDSD_CAPS: u32 = 0x1;
const DDSD_HEIGHT: u32 = 0x2;
const DDSD_WIDTH: u32 = 0x4;
const DDSD_PITCH: u32 = 0x8;
const DDSD_PIXELFORMAT: u32 = 0x1000;
const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
const DDSD_LINEARSIZE: u32 = 0x8_0000;
const DDSD_DEPTH: u32 = 0x80_0000;

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_ALPHA: u32 = 0x2;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x2_0000;

const DDSCAPS_COMPLEX: u32 = 0x8;
const DDSCAPS_TEXTURE: u32 = 0x1000;
const DDSCAPS_MIPMAP: u32 = 0x40_0000;
const DDSCAPS2_CUBEMAP_ALL: u32 = 0xFE00;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;

fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*s)
}

/// (flags, fourcc, bit count, r, g, b, a masks), or `None` → DX10 header.
fn legacy_pixel_format(f: Format) -> Option<(u32, u32, u32, [u32; 4])> {
    Some(match f {
        Format::BC1_UNORM => (DDPF_FOURCC, fourcc(b"DXT1"), 0, [0; 4]),
        Format::BC2_UNORM => (DDPF_FOURCC, fourcc(b"DXT3"), 0, [0; 4]),
        Format::BC3_UNORM => (DDPF_FOURCC, fourcc(b"DXT5"), 0, [0; 4]),
        Format::BC4_UNORM => (DDPF_FOURCC, fourcc(b"ATI1"), 0, [0; 4]),
        Format::BC5_UNORM => (DDPF_FOURCC, fourcc(b"ATI2"), 0, [0; 4]),
        Format::B8G8R8A8_UNORM => {
            (DDPF_RGB | DDPF_ALPHAPIXELS, 0, 32, [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000])
        }
        Format::B8G8R8X8_UNORM => (DDPF_RGB, 0, 32, [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0]),
        Format::R8G8B8A8_UNORM => {
            (DDPF_RGB | DDPF_ALPHAPIXELS, 0, 32, [0x0000_00FF, 0x0000_FF00, 0x00FF_0000, 0xFF00_0000])
        }
        Format::B5G5R5A1_UNORM => (DDPF_RGB | DDPF_ALPHAPIXELS, 0, 16, [0x7C00, 0x03E0, 0x001F, 0x8000]),
        Format::B5G6R5_UNORM => (DDPF_RGB, 0, 16, [0xF800, 0x07E0, 0x001F, 0]),
        Format::A8_UNORM => (DDPF_ALPHA, 0, 8, [0, 0, 0, 0xFF]),
        Format::R8_UNORM => (DDPF_LUMINANCE, 0, 8, [0xFF, 0, 0, 0]),
        _ => return None,
    })
}

/// Serializes a texture (all mip levels and slices) as a DDS file.
pub fn to_dds(tex: &Texture) -> Vec<u8> {
    let mut h = [0u32; 31]; // DDS_HEADER, 124 bytes
    let (row, slice) = tex.format.pitch(tex.width, tex.height).unwrap_or((0, 0));
    let compressed = tex.format.block_bytes().is_some();
    h[0] = 124;
    h[1] = DDSD_CAPS
        | DDSD_HEIGHT
        | DDSD_WIDTH
        | DDSD_PIXELFORMAT
        | DDSD_MIPMAPCOUNT
        | if compressed { DDSD_LINEARSIZE } else { DDSD_PITCH }
        | if tex.dimension == Dimension::Texture3D { DDSD_DEPTH } else { 0 };
    h[2] = tex.height;
    h[3] = tex.width;
    h[4] = if compressed { slice as u32 } else { row as u32 };
    h[5] = if tex.dimension == Dimension::Texture3D { tex.depth } else { 0 };
    h[6] = tex.levels.max(1);
    // h[7..18] reserved; pixel format at h[18..26]
    h[18] = 32;
    // Arrays of 2D textures need the DX10 header for their size.
    let array = tex.dimension == Dimension::Texture2D && tex.depth > 1;
    let legacy = legacy_pixel_format(tex.format).filter(|_| !array);
    let dx10 = legacy.is_none();
    match legacy {
        Some((flags, cc, bits, masks)) => {
            h[19] = flags;
            h[20] = cc;
            h[21] = bits;
            h[22..26].copy_from_slice(&masks);
        }
        None => {
            h[19] = DDPF_FOURCC;
            h[20] = fourcc(b"DX10");
        }
    }
    h[26] = DDSCAPS_TEXTURE
        | if tex.levels > 1 { DDSCAPS_MIPMAP | DDSCAPS_COMPLEX } else { 0 }
        | if tex.dimension != Dimension::Texture2D { DDSCAPS_COMPLEX } else { 0 };
    h[27] = match tex.dimension {
        Dimension::Cube => DDSCAPS2_CUBEMAP_ALL,
        Dimension::Texture3D => DDSCAPS2_VOLUME,
        Dimension::Texture2D => 0,
    };

    let mut out = Vec::with_capacity(4 + 124 + 20 + tex.data.len());
    out.extend_from_slice(b"DDS ");
    h.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    if dx10 {
        let resource_dimension = if tex.dimension == Dimension::Texture3D { 4 } else { 3 };
        let misc = if tex.dimension == Dimension::Cube { 0x4 } else { 0 };
        let array_size = if array { tex.depth } else { 1 };
        for v in [tex.format.0, resource_dimension, misc, array_size, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out.extend_from_slice(&tex.data);
    out
}

/// Size of the DDS magic plus `DDS_HEADER`; the pixel data starts here without a DX10 header.
const HEADER_END: usize = 128;
/// Size of the magic, `DDS_HEADER` and `DDS_HEADER_DXT10`.
const DX10_HEADER_END: usize = 148;

/// The little-endian word at byte offset `at`.
fn le32(file: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([file[at], file[at + 1], file[at + 2], file[at + 3]])
}

/// The fields of a `DDS_HEADER_DXT10` that matter here.
struct Dx10 {
    cube: bool,
    array_size: u32,
}

fn dimension_of(caps2: u32, dx10: Option<&Dx10>) -> Dimension {
    if caps2 & DDSCAPS2_CUBEMAP_ALL != 0 || dx10.is_some_and(|d| d.cube) {
        Dimension::Cube
    } else if caps2 & DDSCAPS2_VOLUME != 0 {
        Dimension::Texture3D
    } else {
        Dimension::Texture2D
    }
}

/// The format named by a FourCC, with the DX10 header fields when it is `DX10`.
fn fourcc_format(file: &[u8], cc: u32) -> Result<(Format, Option<Dx10>), DdsError> {
    Ok(match &cc.to_le_bytes() {
        b"DXT1" => (Format::BC1_UNORM, None),
        b"DXT2" | b"DXT3" => (Format::BC2_UNORM, None),
        b"DXT4" | b"DXT5" => (Format::BC3_UNORM, None),
        b"ATI1" | b"BC4U" => (Format::BC4_UNORM, None),
        b"ATI2" | b"BC5U" => (Format::BC5_UNORM, None),
        b"DX10" => {
            if file.len() < DX10_HEADER_END {
                return Err(DdsError::TruncatedHeader);
            }
            let x = |i: usize| le32(file, HEADER_END + i * 4);
            (Format(x(0)), Some(Dx10 { cube: x(2) & 0x4 != 0, array_size: x(3).max(1) }))
        }
        other => return Err(DdsError::UnsupportedFourCc(String::from_utf8_lossy(other).into_owned())),
    })
}

/// The RGB format with the given bit count and first (red) mask, if there is one.
fn rgb_format(pf_flags: u32, bits: u32, red_mask: u32) -> Option<Format> {
    Some(match (bits, red_mask) {
        (32, 0x00FF_0000) if pf_flags & DDPF_ALPHAPIXELS != 0 => Format::B8G8R8A8_UNORM,
        (32, 0x00FF_0000) => Format::B8G8R8X8_UNORM,
        (32, 0xFF) => Format::R8G8B8A8_UNORM,
        (16, 0x7C00) => Format::B5G5R5A1_UNORM,
        (16, 0xF800) => Format::B5G6R5_UNORM,
        _ => return None,
    })
}

/// The format described by a bit-mask pixel format (no FourCC).
fn masked_format(pf_flags: u32, bits: u32, masks: [u32; 4]) -> Result<Format, DdsError> {
    if pf_flags & DDPF_RGB != 0
        && let Some(format) = rgb_format(pf_flags, bits, masks[0])
    {
        return Ok(format);
    }
    match bits {
        8 if pf_flags & DDPF_ALPHA != 0 => Ok(Format::A8_UNORM),
        8 if pf_flags & DDPF_LUMINANCE != 0 => Ok(Format::R8_UNORM),
        _ => Err(DdsError::UnsupportedPixelFormat { flags: pf_flags, bits }),
    }
}

/// The format of the pixel format block of a DDS file, with the DX10 header fields when there is one.
fn pixel_format(file: &[u8]) -> Result<(Format, Option<Dx10>), DdsError> {
    let h = |i: usize| le32(file, 4 + i * 4);
    let (pf_flags, cc, bits) = (h(19), h(20), h(21));
    if pf_flags & DDPF_FOURCC != 0 {
        return fourcc_format(file, cc);
    }
    Ok((masked_format(pf_flags, bits, [h(22), h(23), h(24), h(25)])?, None))
}

/// Reads a DDS file into a texture named `name`.
pub fn from_dds(name: &str, file: &[u8]) -> Result<Texture, DdsError> {
    if file.len() < HEADER_END || &file[0..4] != b"DDS " {
        return Err(DdsError::NotDds);
    }
    let h = |i: usize| le32(file, 4 + i * 4);
    let (height, width) = (h(2), h(3));
    let levels = if h(1) & DDSD_MIPMAPCOUNT != 0 { h(6).max(1) } else { 1 };
    let (format, dx10) = pixel_format(file)?;
    let dimension = dimension_of(h(27), dx10.as_ref());
    let (data_start, array_size) = dx10.map_or((HEADER_END, 1), |d| (DX10_HEADER_END, d.array_size));
    let depth = match dimension {
        Dimension::Cube => 6,
        Dimension::Texture3D => h(5).max(1),
        Dimension::Texture2D => array_size,
    };
    let tex =
        Texture { name: name.to_owned(), width, height, depth, levels, format, dimension, usage: 0, data: Vec::new() };
    let need = tex.expected_size().ok_or_else(|| DdsError::UnsupportedFormat(format.name().to_string()))?;
    let body = file.get(data_start..).unwrap_or(&[]);
    if body.len() < need {
        return Err(DdsError::TooShort { have: body.len(), need });
    }
    Ok(Texture { data: body[..need].to_vec(), ..tex })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_headers() {
        let tex = |format| Texture {
            name: "t".into(),
            width: 8,
            height: 4,
            depth: 1,
            levels: 2,
            format,
            dimension: Dimension::Texture2D,
            usage: 0,
            data: vec![0; 24],
        };
        let dds = to_dds(&tex(Format::BC1_UNORM));
        assert_eq!(&dds[0..4], b"DDS ");
        assert_eq!(&dds[84..88], b"DXT1");
        assert_eq!(dds.len(), 128 + 24);
        let dds = to_dds(&tex(Format::BC7_UNORM));
        assert_eq!(&dds[84..88], b"DX10");
        assert_eq!(u32::from_le_bytes(dds[128..132].try_into().unwrap()), 98);
        assert_eq!(dds.len(), 128 + 20 + 24);
    }

    #[test]
    fn texture_arrays() {
        let t = Texture {
            name: "noise".into(),
            width: 4,
            height: 4,
            depth: 3,
            levels: 1,
            format: Format::R8_UNORM,
            dimension: Dimension::Texture2D,
            usage: 0,
            data: (0..48).collect(),
        };
        let back = from_dds("noise", &to_dds(&t)).unwrap();
        assert_eq!((back.format, back.depth, back.dimension), (Format::R8_UNORM, 3, Dimension::Texture2D));
        assert_eq!(back.data, t.data);
    }

    #[test]
    fn dds_round_trip() {
        for format in [Format::BC1_UNORM, Format::BC7_UNORM, Format::B8G8R8A8_UNORM, Format::R8_UNORM] {
            let t = Texture {
                name: "t".into(),
                width: 8,
                height: 8,
                depth: 1,
                levels: 2,
                format,
                dimension: Dimension::Texture2D,
                usage: 0,
                data: Vec::new(),
            };
            let size = t.expected_size().unwrap();
            let t = Texture { data: (0..size).map(|i| i as u8).collect(), ..t };
            let back = from_dds("t", &to_dds(&t)).unwrap();
            assert_eq!((back.format, back.width, back.height, back.levels), (format, 8, 8, 2));
            assert_eq!(back.data, t.data);
        }
    }
}
