//! RAGE game file formats.
//!
//! * [`ytd`]: texture dictionaries (GTA V Legacy and Enhanced)
//! * [`texture`]: pixel formats, decoding to RGBA
//! * [`dds`], [`png`]: export, compatible with OpenIV/CodeWalker output
//! * [`gxt2`], [`ldb`]: text tables (GTA V, RDR2)
//! * [`meta`], [`pso`], [`rbf`]: metadata (`.ymap`, `.ytyp`, `.ymt`, `.cut`, …) as XML
#![forbid(unsafe_code)]

pub mod anim;
pub mod audio;
pub mod collision;
pub mod math;
pub mod meta;
pub mod model;
pub mod openformats;
pub mod script;
pub mod text;
pub mod texture;
pub mod video;
pub mod world;

// The modules were all at the top before they were grouped by kind; these
// keep `rage_formats::drawable` and the like working.
pub use anim::{clip, clip6, expression, wad};
pub use audio::{awc, ivaud};
pub use collision::{bn6, wbn, ybn, ybn_write};
pub use meta::{names, psin, pso, rbf, xml};
pub use model::{drawable, drawable6, geometry, gltf, model_textures, shaders, wdr, ydr_write};
pub use script::{sco, ysc};
pub use text::{gxt, gxt2, ldb};
pub use texture::{bcpack, dds, td6, ttpf, wtd_write, ytd, ytd_write};
pub use video::{bink, media};
pub use world::{paths, wpfl, wpl, ypt};

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
