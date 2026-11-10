//! RAGE game file formats.
//!
//! * [`ytd`]: texture dictionaries (GTA V Legacy and Enhanced)
//! * [`texture`]: pixel formats, decoding to RGBA
//! * [`dds`], [`png`]: export, compatible with OpenIV/CodeWalker output

pub mod dds;
pub mod drawable;
pub mod gltf;
pub mod texture;
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
