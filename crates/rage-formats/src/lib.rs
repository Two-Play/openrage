//! RAGE game file formats.
//!
//! * [`ytd`]: texture dictionaries (GTA V Legacy and Enhanced)
//! * [`texture`]: pixel formats, decoding to RGBA
//! * [`dds`], [`png`]: export, compatible with OpenIV/CodeWalker output
//! * [`meta`], [`pso`], [`rbf`]: metadata (`.ymap`, `.ytyp`, `.ymt`, `.cut`, …) as XML

pub mod awc;
pub mod clip;
pub mod dds;
pub mod drawable;
pub mod geometry;
pub mod gltf;
pub mod gxt2;
pub mod math;
pub mod media;
pub mod meta;
pub mod names;
pub mod paths;
pub mod pso;
pub mod rbf;
pub mod shaders;
pub mod texture;
pub mod xml;
pub mod ybn;
pub mod ypt;
pub mod ysc;
pub mod ytd;
pub mod ytd_write;

/// Encodes RGBA8 pixels as a PNG file.
pub fn png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, ::png::EncodingError> {
    let mut out = Vec::new();
    {
        let mut enc = ::png::Encoder::new(&mut out, width, height);
        enc.set_color(::png::ColorType::Rgba);
        enc.set_depth(::png::BitDepth::Eight);
        let mut w = enc.write_header()?;
        w.write_image_data(rgba)?;
    }
    Ok(out)
}
