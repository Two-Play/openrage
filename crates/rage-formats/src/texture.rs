//! Textures: pixel formats, mip layout, decoding to RGBA.
//!
//! Formats are expressed as DXGI format codes. GTA V Enhanced stores them
//! directly; the Direct3D 9 codes of GTA V Legacy are mapped on load.

/// A DXGI pixel format code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Format(pub u32);

impl Format {
    pub const R32G32B32A32_FLOAT: Self = Self(2);
    pub const R16G16B16A16_FLOAT: Self = Self(10);
    pub const R16G16B16A16_UNORM: Self = Self(11);
    pub const R10G10B10A2_UNORM: Self = Self(24);
    pub const R11G11B10_FLOAT: Self = Self(26);
    pub const R8G8B8A8_UNORM: Self = Self(28);
    pub const R8G8B8A8_UNORM_SRGB: Self = Self(29);
    pub const R16G16_UNORM: Self = Self(35);
    pub const R32_FLOAT: Self = Self(41);
    pub const R8G8_UNORM: Self = Self(49);
    pub const R16_FLOAT: Self = Self(54);
    pub const R16_UNORM: Self = Self(56);
    pub const R8_UNORM: Self = Self(61);
    pub const A8_UNORM: Self = Self(65);
    pub const BC1_UNORM: Self = Self(71);
    pub const BC1_UNORM_SRGB: Self = Self(72);
    pub const BC2_UNORM: Self = Self(74);
    pub const BC2_UNORM_SRGB: Self = Self(75);
    pub const BC3_UNORM: Self = Self(77);
    pub const BC3_UNORM_SRGB: Self = Self(78);
    pub const BC4_UNORM: Self = Self(80);
    pub const BC4_SNORM: Self = Self(81);
    pub const BC5_UNORM: Self = Self(83);
    pub const BC5_SNORM: Self = Self(84);
    pub const B5G6R5_UNORM: Self = Self(85);
    pub const B5G5R5A1_UNORM: Self = Self(86);
    pub const B8G8R8A8_UNORM: Self = Self(87);
    pub const B8G8R8X8_UNORM: Self = Self(88);
    pub const B8G8R8A8_UNORM_SRGB: Self = Self(91);
    pub const BC6H_UF16: Self = Self(95);
    pub const BC6H_SF16: Self = Self(96);
    pub const BC7_UNORM: Self = Self(98);
    pub const BC7_UNORM_SRGB: Self = Self(99);
    pub const B4G4R4A4_UNORM: Self = Self(115);

    /// Maps a Direct3D 9 format code (GTA V Legacy) to DXGI.
    pub fn from_d3d9(code: u32) -> Option<Self> {
        Some(match code {
            21 => Self::B8G8R8A8_UNORM,     // A8R8G8B8
            22 => Self::B8G8R8X8_UNORM,     // X8R8G8B8
            25 => Self::B5G5R5A1_UNORM,     // A1R5G5B5
            28 => Self::A8_UNORM,           // A8
            32 => Self::R8G8B8A8_UNORM,     // A8B8G8R8
            50 => Self::R8_UNORM,           // L8
            0x3154_5844 => Self::BC1_UNORM, // DXT1
            0x3354_5844 => Self::BC2_UNORM, // DXT3
            0x3554_5844 => Self::BC3_UNORM, // DXT5
            0x3149_5441 => Self::BC4_UNORM, // ATI1
            0x3249_5441 => Self::BC5_UNORM, // ATI2
            0x2037_4342 => Self::BC7_UNORM, // "BC7 "
            _ => return None,
        })
    }

    /// The Direct3D 9 code GTA V Legacy uses for this format, if it has one.
    pub fn to_d3d9(self) -> Option<u32> {
        Some(match self {
            Self::B8G8R8A8_UNORM => 21,
            Self::B8G8R8X8_UNORM => 22,
            Self::B5G5R5A1_UNORM => 25,
            Self::A8_UNORM => 28,
            Self::R8G8B8A8_UNORM => 32,
            Self::R8_UNORM => 50,
            Self::BC1_UNORM => 0x3154_5844,
            Self::BC2_UNORM => 0x3354_5844,
            Self::BC3_UNORM => 0x3554_5844,
            Self::BC4_UNORM => 0x3149_5441,
            Self::BC5_UNORM => 0x3249_5441,
            Self::BC7_UNORM => 0x2037_4342,
            _ => return None,
        })
    }

    /// Bytes per block (compressed) or per pixel (uncompressed).
    pub fn block_or_pixel_bytes(self) -> u32 {
        self.block_bytes().map_or_else(|| self.bits_per_pixel().map_or(8, |b| (b / 8).max(1)) as u32, |b| b as u32)
    }

    /// Bytes per 4×4 block for block-compressed formats.
    pub fn block_bytes(self) -> Option<usize> {
        match self.0 {
            70..=72 | 79..=81 => Some(8),            // BC1, BC4
            73..=78 | 82..=84 | 94..=99 => Some(16), // BC2, BC3, BC5, BC6H, BC7
            _ => None,
        }
    }

    /// Bits per pixel for uncompressed formats.
    pub fn bits_per_pixel(self) -> Option<usize> {
        Some(match self.0 {
            1..=4 => 128,
            5..=8 => 96,
            9..=22 => 64,
            23..=47 => 32,
            48..=59 | 85 | 86 | 115 => 16,
            60..=65 => 8,
            87..=93 => 32,
            _ => return None,
        })
    }

    /// Bytes per row and per 2D slice at the given size.
    pub fn pitch(self, width: u32, height: u32) -> Option<(usize, usize)> {
        let (w, h) = (width.max(1) as usize, height.max(1) as usize);
        if let Some(block) = self.block_bytes() {
            let row = w.div_ceil(4) * block;
            Some((row, row * h.div_ceil(4)))
        } else {
            let row = (w * self.bits_per_pixel()?).div_ceil(8);
            Some((row, row * h))
        }
    }

    pub fn is_srgb(self) -> bool {
        matches!(self.0, 29 | 72 | 75 | 78 | 91 | 93 | 99)
    }

    pub fn name(self) -> String {
        let n = match self.0 {
            2 => "R32G32B32A32_FLOAT",
            10 => "R16G16B16A16_FLOAT",
            11 => "R16G16B16A16_UNORM",
            24 => "R10G10B10A2_UNORM",
            26 => "R11G11B10_FLOAT",
            28 => "R8G8B8A8_UNORM",
            29 => "R8G8B8A8_UNORM_SRGB",
            35 => "R16G16_UNORM",
            41 => "R32_FLOAT",
            49 => "R8G8_UNORM",
            54 => "R16_FLOAT",
            56 => "R16_UNORM",
            61 => "R8_UNORM",
            65 => "A8_UNORM",
            71 => "BC1",
            72 => "BC1_SRGB",
            74 => "BC2",
            75 => "BC2_SRGB",
            77 => "BC3",
            78 => "BC3_SRGB",
            80 => "BC4",
            81 => "BC4_SNORM",
            83 => "BC5",
            84 => "BC5_SNORM",
            85 => "B5G6R5",
            86 => "B5G5R5A1",
            87 => "B8G8R8A8",
            88 => "B8G8R8X8",
            91 => "B8G8R8A8_SRGB",
            95 => "BC6H_UF16",
            96 => "BC6H_SF16",
            98 => "BC7",
            99 => "BC7_SRGB",
            115 => "B4G4R4A4",
            other => return format!("DXGI {other}"),
        };
        n.to_owned()
    }
}

/// Shape of a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Texture2D,
    Cube,
    Texture3D,
}

#[derive(Debug, Clone)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Slices: 1 for 2D textures, 6 for cube maps, depth for volume textures.
    pub depth: u32,
    pub levels: u32,
    pub format: Format,
    pub dimension: Dimension,
    /// Usage class (diffuse, normal, …) as stored by the game.
    pub usage: u8,
    /// All mip levels (of all slices), as stored.
    pub data: Vec<u8>,
}

impl Texture {
    /// Size in bytes of mip level `level` of one slice.
    pub fn level_size(&self, level: u32) -> Option<usize> {
        self.format.pitch(self.width >> level, self.height >> level).map(|(_, slice)| slice)
    }

    /// Size of all mip levels of all slices.
    pub fn expected_size(&self) -> Option<usize> {
        let per_slice: usize = (0..self.levels).map(|l| self.level_size(l)).sum::<Option<usize>>()?;
        Some(per_slice * self.depth.max(1) as usize)
    }

    pub fn usage_name(&self) -> &'static str {
        match self.usage {
            1 => "default",
            2 => "terrain",
            3 => "cloud density",
            4 => "cloud normal",
            5 => "cable",
            6 => "fence",
            8 => "script",
            9 => "water flow",
            10 => "water foam",
            11 => "water fog",
            12 => "water ocean",
            14 => "foam opacity",
            16 => "diffuse mip sharpen",
            18 => "diffuse dark",
            19 => "diffuse alpha opaque",
            20 => "diffuse",
            21 => "detail",
            22 => "normal",
            23 => "specular",
            24 => "emissive",
            25 => "tint palette",
            26 => "skip processing",
            _ => "unknown",
        }
    }

    /// Decodes the top mip level of the first slice to RGBA8.
    /// Returns `None` for formats without a decoder.
    pub fn to_rgba(&self) -> Option<Vec<u8>> {
        let (w, h) = (self.width.max(1) as usize, self.height.max(1) as usize);
        let size = self.level_size(0)?;
        let src = self.data.get(..size)?;
        let mut out = vec![0u8; w * h * 4];
        if self.format.block_bytes().is_some() {
            let mut pixels = vec![0u32; w * h];
            let result = match self.format.0 {
                70..=72 => texture2ddecoder::decode_bc1(src, w, h, &mut pixels),
                73..=75 => texture2ddecoder::decode_bc2(src, w, h, &mut pixels),
                76..=78 => texture2ddecoder::decode_bc3(src, w, h, &mut pixels),
                79..=81 => texture2ddecoder::decode_bc4(src, w, h, &mut pixels),
                82..=84 => texture2ddecoder::decode_bc5(src, w, h, &mut pixels),
                94 | 95 => texture2ddecoder::decode_bc6_unsigned(src, w, h, &mut pixels),
                96 => texture2ddecoder::decode_bc6_signed(src, w, h, &mut pixels),
                97..=99 => texture2ddecoder::decode_bc7(src, w, h, &mut pixels),
                _ => return None,
            };
            result.ok()?;
            for (o, p) in out.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
                let [b, g, r, a] = p.to_le_bytes();
                o.copy_from_slice(&[r, g, b, a]);
            }
            return Some(out);
        }
        let (row, _) = self.format.pitch(self.width, self.height)?;
        // Single-channel floats (heights, densities) are scaled from their own range to grey.
        if matches!(self.format.0, 41 | 54) {
            let mut values = Vec::with_capacity(w * h);
            for y in 0..h {
                let line = &src[y * row..];
                for x in 0..w {
                    values.push(if self.format.0 == 41 {
                        f32::from_le_bytes(line[x * 4..x * 4 + 4].try_into().ok()?)
                    } else {
                        half(u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]))
                    });
                }
            }
            let finite = values.iter().copied().filter(|v| v.is_finite());
            let (lo, hi) = finite.fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
            let span = if hi > lo { hi - lo } else { 1.0 };
            for (o, v) in out.as_chunks_mut::<4>().0.iter_mut().zip(values) {
                let g = unit((v - lo) / span);
                o.copy_from_slice(&[g, g, g, 255]);
            }
            return Some(out);
        }
        for y in 0..h {
            let line = &src[y * row..];
            for x in 0..w {
                let o = &mut out[(y * w + x) * 4..][..4];
                let px: [u8; 4] = match self.format.0 {
                    28 | 29 => line[x * 4..x * 4 + 4].try_into().ok()?,
                    87 | 91 => {
                        let p = &line[x * 4..];
                        [p[2], p[1], p[0], p[3]]
                    }
                    88 => {
                        let p = &line[x * 4..];
                        [p[2], p[1], p[0], 255]
                    }
                    61 => [line[x], line[x], line[x], 255],
                    // Floating-point formats: shown with values clamped to 0..1
                    // (single-channel heights are scaled to their range below).
                    41 => {
                        let v = f32::from_le_bytes(line[x * 4..x * 4 + 4].try_into().ok()?);
                        let g = unit(v);
                        [g, g, g, 255]
                    }
                    54 => {
                        let v = half(u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]));
                        let g = unit(v);
                        [g, g, g, 255]
                    }
                    10 => {
                        let c =
                            |i: usize| unit(half(u16::from_le_bytes([line[x * 8 + i * 2], line[x * 8 + i * 2 + 1]])));
                        [c(0), c(1), c(2), c(3)]
                    }
                    2 => {
                        let c = |i: usize| {
                            unit(f32::from_le_bytes(
                                line[x * 16 + i * 4..x * 16 + i * 4 + 4].try_into().unwrap_or_default(),
                            ))
                        };
                        [c(0), c(1), c(2), c(3)]
                    }
                    26 => {
                        let v = u32::from_le_bytes(line[x * 4..x * 4 + 4].try_into().ok()?);
                        [
                            unit(small_float(v & 0x7FF, 6)),
                            unit(small_float((v >> 11) & 0x7FF, 6)),
                            unit(small_float(v >> 22, 5)),
                            255,
                        ]
                    }
                    24 => {
                        let v = u32::from_le_bytes(line[x * 4..x * 4 + 4].try_into().ok()?);
                        let c = |s: u32| ((v >> s & 0x3FF) * 255 / 0x3FF) as u8;
                        [c(0), c(10), c(20), ((v >> 30) * 85) as u8]
                    }
                    65 => [255, 255, 255, line[x]],
                    49 => [line[x * 2], line[x * 2 + 1], 0, 255],
                    56 => {
                        let v = line[x * 2 + 1];
                        [v, v, v, 255]
                    }
                    85 => {
                        let v = u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]);
                        [expand(v >> 11, 5), expand(v >> 5, 6), expand(v, 5), 255]
                    }
                    86 => {
                        let v = u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]);
                        [expand(v >> 10, 5), expand(v >> 5, 5), expand(v, 5), if v >> 15 == 1 { 255 } else { 0 }]
                    }
                    115 => {
                        let v = u16::from_le_bytes([line[x * 2], line[x * 2 + 1]]);
                        [expand(v >> 8, 4), expand(v >> 4, 4), expand(v, 4), expand(v >> 12, 4)]
                    }
                    _ => return None,
                };
                o.copy_from_slice(&px);
            }
        }
        Some(out)
    }
}

/// A value clamped to 0..1 as an 8-bit channel.
fn unit(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// IEEE half-precision float.
fn half(h: u16) -> f32 {
    let sign = if h >> 15 == 1 { -1.0 } else { 1.0 };
    let exp = (h >> 10) & 0x1F;
    let man = (h & 0x3FF) as f32;
    sign * match exp {
        0 => man * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + man / 1024.0) * 2f32.powi(e as i32 - 15),
    }
}

/// An unsigned small float (R11G11B10): 5-bit exponent, `man_bits` mantissa bits.
fn small_float(v: u32, man_bits: u32) -> f32 {
    let exp = (v >> man_bits) & 0x1F;
    let man = (v & ((1 << man_bits) - 1)) as f32 / (1u32 << man_bits) as f32;
    match exp {
        0 => man * 2f32.powi(-14),
        31 => f32::INFINITY,
        e => (1.0 + man) * 2f32.powi(e as i32 - 15),
    }
}

/// Expands an n-bit channel (in the low bits of `v`) to 8 bits.
fn expand(v: u16, bits: u32) -> u8 {
    let max = (1u32 << bits) - 1;
    ((v as u32 & max) * 255 / max) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitches() {
        assert_eq!(Format::BC1_UNORM.pitch(256, 256), Some((512, 512 * 64)));
        assert_eq!(Format::BC7_UNORM.pitch(2, 2), Some((16, 16)));
        assert_eq!(Format::B8G8R8A8_UNORM.pitch(3, 2), Some((12, 24)));
        assert_eq!(Format::from_d3d9(0x3154_5844), Some(Format::BC1_UNORM));
    }

    #[test]
    fn decodes_uncompressed_and_bc1() {
        let tex = |format, data: Vec<u8>| Texture {
            name: "t".into(),
            width: 4,
            height: 4,
            depth: 1,
            levels: 1,
            format,
            dimension: Dimension::Texture2D,
            usage: 0,
            data,
        };
        let bgra: Vec<u8> = (0..16).flat_map(|_| [10u8, 20, 30, 40]).collect();
        assert_eq!(&tex(Format::B8G8R8A8_UNORM, bgra).to_rgba().unwrap()[..4], &[30, 20, 10, 40]);
        // BC1 block: both endpoints pure red (0xF800), all indices 0.
        let bc1 = vec![0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        assert_eq!(&tex(Format::BC1_UNORM, bc1).to_rgba().unwrap()[..4], &[255, 0, 0, 255]);
    }
}
