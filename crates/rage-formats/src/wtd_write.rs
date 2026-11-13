//! Writing GTA IV texture dictionaries (`.wtd`, RSC5 type 8).
//!
//! When every edit replaces a texture with one of identical shape, only the
//! pixel data is overwritten and the resource keeps its exact layout. Any
//! other edit keeps the system memory as it is (root, block map and fields
//! OpenRAGE does not interpret), patches the structures of changed textures
//! in place, appends structures and names for added ones, and lays out the
//! graphics memory (pixel data) anew. Removed textures' structures stay as
//! unreferenced bytes.
//!
//! Texture fields written (see [`crate::ytd`]): `0x1C` width, height, `0x20`
//! D3D9 format, `0x24` stride (bytes per row of the top level), `0x27` mip
//! levels, and the data pointer (`0x48`, or `0x44` in older dictionaries).
//! Added textures copy the first texture's structure, with the name
//! `pack:/<name>.dds` and the texture chain pointers (`0x40`, `0x44`) cleared.
//!
//! Dictionaries embedded in models (`.wdr`) are edited with [`apply_embedded`]:
//! the graphics memory also holds vertex and index buffers, so it is kept and
//! new pixel data is appended. Texture structures never move, because the
//! model's shaders point at them; a texture a shader uses cannot be removed.

use rage_resource::{Resource, ResourceError};

use crate::texture::Texture;
use crate::ytd::{Layout, TextureDictionary};
use crate::ytd_write::{Edit, EditResult, name_hash};

const SYSTEM_BASE: u32 = 0x5000_0000;
const GRAPHICS_BASE: u32 = 0x6000_0000;
/// Alignment of pixel data in graphics memory (the game's files use 128 or more).
const DATA_ALIGN: usize = 256;

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

fn same_shape(a: &Texture, b: &Texture) -> bool {
    (a.format, a.width, a.height, a.levels) == (b.format, b.width, b.height, b.levels) && a.data.len() == b.data.len()
}

fn put_u32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_u16(buf: &mut [u8], at: usize, v: u16) {
    buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn get_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(buf[at..at + 4].try_into().expect("4 bytes"))
}

/// Applies `edits` to a GTA IV dictionary read from `res`. `kind` and
/// `flags` are the RSC5 header's; returns a standalone RSC5 file.
pub fn apply(
    res: &Resource,
    kind: u32,
    flags: u32,
    dict: &TextureDictionary,
    edits: &[Edit],
) -> Result<EditResult, ResourceError> {
    apply_inner(res, kind, flags, dict, edits, &Target::STANDALONE, false)
}

/// Applies `edits` to a dictionary embedded in a model at `root`. `in_use`
/// lists the texture structures the model's shaders point at, with the
/// shader's name (removing those is refused).
pub fn apply_embedded(
    res: &Resource,
    kind: u32,
    flags: u32,
    root: u64,
    in_use: &[(u64, String)],
    dict: &TextureDictionary,
    edits: &[Edit],
) -> Result<EditResult, ResourceError> {
    apply_inner(res, kind, flags, dict, edits, &Target { root, keep_graphics: true, in_use }, false)
}

/// Like [`rebuild`] for a dictionary embedded in a model.
pub fn rebuild_embedded(
    res: &Resource,
    kind: u32,
    flags: u32,
    root: u64,
    dict: &TextureDictionary,
) -> Result<Vec<u8>, ResourceError> {
    Ok(apply_inner(res, kind, flags, dict, &[], &Target { root, keep_graphics: true, in_use: &[] }, true)?.file)
}

/// Where the dictionary sits and how its resource is written.
struct Target<'a> {
    root: u64,
    /// Keep the graphics memory and append new pixel data (models), instead
    /// of laying it out anew (texture dictionaries).
    keep_graphics: bool,
    in_use: &'a [(u64, String)],
}

impl Target<'static> {
    const STANDALONE: Self = Self { root: Resource::ROOT, keep_graphics: false, in_use: &[] };
}

/// Rebuilds a dictionary without changes, the way any non-trivial edit does
/// (used to check the writer against the game's files).
pub fn rebuild(res: &Resource, kind: u32, flags: u32, dict: &TextureDictionary) -> Result<Vec<u8>, ResourceError> {
    Ok(apply_inner(res, kind, flags, dict, &[], &Target::STANDALONE, true)?.file)
}

fn apply_inner(
    res: &Resource,
    kind: u32,
    flags: u32,
    dict: &TextureDictionary,
    edits: &[Edit],
    target: &Target,
    force_rebuild: bool,
) -> Result<EditResult, ResourceError> {
    if dict.layout != Layout::Gta4 {
        return Err(bad("not a GTA IV texture dictionary"));
    }
    if !dict.errors.is_empty() {
        return Err(bad(format!("dictionary has unreadable textures ({}); refusing to rewrite it", dict.errors[0])));
    }
    let find = |name: &str| dict.textures.iter().position(|t| t.name.eq_ignore_ascii_case(name));
    for e in edits {
        if let Edit::Set(t) = e {
            if t.expected_size().is_none_or(|n| n > t.data.len()) {
                return Err(bad(format!("{}: pixel data is shorter than its format and size require", t.name)));
            }
            if t.depth > 1 || t.format.to_d3d9().is_none() {
                return Err(bad(format!("{}: {} textures cannot go into GTA IV", t.name, t.format.name())));
            }
        }
    }

    // In place: only same-shape replacements.
    let in_place = !force_rebuild
        && edits.iter().all(|e| match e {
            Edit::Set(t) => find(&t.name).is_some_and(|i| same_shape(&dict.textures[i], t)),
            Edit::Remove(_) => false,
        });
    if in_place {
        let mut out = res.clone();
        for e in edits {
            let Edit::Set(t) = e else { unreachable!() };
            let i = find(&t.name).expect("checked");
            out.bytes_mut(dict.stored[i].data_ptr, t.data.len())?.copy_from_slice(&t.data);
        }
        let file = rage_resource::write_rsc5(kind, flags, &out.system, &out.graphics)?;
        return Ok(EditResult { file, in_place: true, replaced: edits.len(), added: 0, removed: 0 });
    }

    // The structures as laid out in system memory, in dictionary order.
    let root = target.root;
    let list = res.u32(root + 0x18)? as u64;
    let addresses: Vec<u32> =
        (0..dict.textures.len() as u64).map(|i| res.u32(list + i * 4)).collect::<Result<_, _>>()?;

    // Final list: (texture, original index or None when added, pixel data changed).
    let mut entries: Vec<(Texture, Option<usize>, bool)> =
        dict.textures.iter().cloned().enumerate().map(|(i, t)| (t, Some(i), false)).collect();
    let (mut replaced, mut added, mut removed) = (0, 0, 0);
    for e in edits {
        match e {
            Edit::Remove(name) => {
                if let Some(i) = find(name)
                    && let Some((_, shader)) = target.in_use.iter().find(|(a, _)| *a == addresses[i] as u64)
                {
                    return Err(bad(format!("{name} is used by the shader {shader}; replace it instead")));
                }
                let before = entries.len();
                entries.retain(|(t, _, _)| !t.name.eq_ignore_ascii_case(name));
                if entries.len() == before {
                    return Err(bad(format!("{name}: no such texture")));
                }
                removed += 1;
            }
            Edit::Set(new) => match entries.iter_mut().find(|(t, _, _)| t.name.eq_ignore_ascii_case(&new.name)) {
                Some(slot) => {
                    slot.0 = Texture { name: slot.0.name.clone(), ..new.clone() };
                    slot.2 = true;
                    replaced += 1;
                }
                None => {
                    if dict.stored.is_empty() {
                        return Err(bad("cannot add a texture to an empty dictionary (no template texture)"));
                    }
                    entries.push((new.clone(), None, true));
                    added += 1;
                }
            },
        }
    }
    // The game looks textures up by binary search over the sorted hash list.
    entries.sort_by_key(|(t, _, _)| name_hash(&t.name));

    let mut sys = res.system.clone();
    let append = |sys: &mut Vec<u8>, bytes: &[u8]| -> u32 {
        let at = sys.len().next_multiple_of(16);
        sys.resize(at, 0);
        sys.extend_from_slice(bytes);
        SYSTEM_BASE + at as u32
    };
    let mut gfx: Vec<u8> = if target.keep_graphics { res.graphics.clone() } else { Vec::new() };
    let mut tex_addrs = Vec::with_capacity(entries.len());
    for (tex, original, changed) in &entries {
        let size = tex.expected_size().ok_or_else(|| bad(format!("{}: unsupported format", tex.name)))?;
        let data_ptr = match original {
            // Unchanged pixel data stays where it is when the graphics memory is kept.
            Some(i) if target.keep_graphics && !changed => dict.stored[*i].data_ptr as u32,
            _ => {
                let data_at = gfx.len().next_multiple_of(DATA_ALIGN);
                gfx.resize(data_at, 0);
                gfx.extend_from_slice(&tex.data[..size]);
                GRAPHICS_BASE + data_at as u32
            }
        };

        let template = &dict.stored[original.unwrap_or(0)];
        // The data pointer sits at 0x48, or 0x44 in a few older dictionaries.
        let data_field = if get_u32(&template.template, 0x48) as u64 == template.data_ptr { 0x48 } else { 0x44 };
        let addr = match original {
            Some(i) => addresses[*i],
            None => {
                let mut name = format!("pack:/{}.dds", tex.name).into_bytes();
                name.push(0);
                let name_ptr = append(&mut sys, &name);
                let mut t = template.template.clone();
                t.resize(Layout::Gta4.texture_size(), 0);
                put_u32(&mut t, 0x14, name_ptr);
                put_u32(&mut t, 0x18, 0);
                if data_field == 0x48 {
                    put_u32(&mut t, 0x40, 0);
                    put_u32(&mut t, 0x44, 0);
                }
                append(&mut sys, &t)
            }
        };
        let code = tex.format.to_d3d9().expect("checked above");
        let stride = (tex.level_size(0).unwrap_or(0) / tex.height.max(1) as usize) as u16;
        let at = (addr - SYSTEM_BASE) as usize;
        let t = sys.get_mut(at..at + Layout::Gta4.texture_size()).ok_or_else(|| bad("texture outside memory"))?;
        put_u16(t, 0x1C, tex.width as u16);
        put_u16(t, 0x1E, tex.height as u16);
        put_u32(t, 0x20, code);
        put_u16(t, 0x24, stride);
        t[0x27] = tex.levels as u8;
        put_u32(t, data_field, data_ptr);
        tex_addrs.push(addr);
    }

    // Hash and pointer lists: rewritten where they are when they still fit.
    let count = entries.len() as u16;
    let hashes: Vec<u8> = entries.iter().flat_map(|(t, _, _)| name_hash(&t.name).to_le_bytes()).collect();
    let pointers: Vec<u8> = tex_addrs.iter().flat_map(|a| a.to_le_bytes()).collect();
    let root_at = (root as u32).checked_sub(SYSTEM_BASE).ok_or_else(|| bad("dictionary outside memory"))? as usize;
    for (at, bytes) in [(root_at + 0x10, hashes), (root_at + 0x18, pointers)] {
        let capacity = u16::from_le_bytes([sys[at + 6], sys[at + 7]]);
        let ptr = if count <= capacity && get_u32(&sys, at) >= SYSTEM_BASE {
            let ptr = get_u32(&sys, at);
            let off = (ptr - SYSTEM_BASE) as usize;
            sys.get_mut(off..off + bytes.len()).ok_or_else(|| bad("list outside memory"))?.copy_from_slice(&bytes);
            ptr
        } else {
            put_u16(&mut sys, at + 6, count);
            append(&mut sys, &bytes)
        };
        put_u32(&mut sys, at, ptr);
        put_u16(&mut sys, at + 4, count);
    }

    let (new_flags, sys_len, gfx_len) = if sys.len() == res.system.len() && gfx.len() == res.graphics.len() {
        (flags, sys.len(), gfx.len())
    } else {
        rage_resource::rsc5_flags_for(sys.len(), gfx.len(), flags)
    };
    sys.resize(sys_len, 0);
    gfx.resize(gfx_len, 0);
    let file = rage_resource::write_rsc5(kind, new_flags, &sys, &gfx)?;
    Ok(EditResult { file, in_place: false, replaced, added, removed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::{Dimension, Format};
    use crate::ytd::{parse_gta4, parse_gta4_at};

    fn texture(name: &str, size: u32, format: Format, fill: u8) -> Texture {
        let t = Texture {
            name: name.into(),
            width: size,
            height: size,
            depth: 1,
            levels: 1,
            format,
            dimension: Dimension::Texture2D,
            usage: 0,
            data: Vec::new(),
        };
        let n = t.expected_size().unwrap();
        Texture { data: vec![fill; n], ..t }
    }

    /// A one-texture dictionary ("stone", 4×4 DXT1) as an RSC5 file.
    fn dictionary() -> Vec<u8> {
        let mut sys = vec![0u8; 0x1000];
        put_u32(&mut sys, 0x10, 0x5000_0030); // hash list
        put_u16(&mut sys, 0x14, 1);
        put_u16(&mut sys, 0x16, 1);
        put_u32(&mut sys, 0x18, 0x5000_0040); // texture list
        put_u16(&mut sys, 0x1C, 1);
        put_u16(&mut sys, 0x1E, 1);
        put_u32(&mut sys, 0x30, name_hash("stone"));
        put_u32(&mut sys, 0x40, 0x5000_0080);
        let t = 0x80;
        put_u32(&mut sys, t + 0x14, 0x5000_00E0);
        put_u16(&mut sys, t + 0x1C, 4);
        put_u16(&mut sys, t + 0x1E, 4);
        put_u32(&mut sys, t + 0x20, 0x3154_5844); // DXT1
        put_u16(&mut sys, t + 0x24, 2); // stride: 8 bytes over 4 rows
        sys[t + 0x27] = 1;
        put_u32(&mut sys, t + 0x48, 0x6000_0000);
        sys[0xE0..0xE0 + 15].copy_from_slice(b"pack:/stone.dds");
        let mut gfx = vec![0u8; 0x1000];
        gfx[..8].fill(0xAB);
        let (flags, ..) = rage_resource::rsc5_flags_for(sys.len(), gfx.len(), 0xC000_0000);
        rage_resource::write_rsc5(8, flags, &sys, &gfx).unwrap()
    }

    fn open(file: &[u8]) -> (Resource, u32, u32, TextureDictionary) {
        let (kind, flags) = rage_resource::rsc5_header(file).unwrap();
        let res = Resource::from_rsc5(file).unwrap();
        let dict = parse_gta4(&res).unwrap();
        assert!(dict.errors.is_empty(), "{:?}", dict.errors);
        (res, kind, flags, dict)
    }

    #[test]
    fn replaces_in_place() {
        let (res, kind, flags, dict) = open(&dictionary());
        let r = apply(&res, kind, flags, &dict, &[Edit::Set(texture("STONE", 4, Format::BC1_UNORM, 7))]).unwrap();
        assert!(r.in_place);
        let (_, _, flags2, d2) = open(&r.file);
        assert_eq!(flags2, flags);
        assert_eq!(d2.textures[0].data, vec![7; 8]);
    }

    #[test]
    fn rebuilds_for_new_shapes_added_and_removed() {
        let (res, kind, flags, dict) = open(&dictionary());
        let edits = [
            Edit::Set(texture("stone", 64, Format::BC3_UNORM, 5)),
            Edit::Set(texture("grass", 8, Format::B8G8R8A8_UNORM, 3)),
            Edit::Set(texture("moss", 4, Format::BC1_UNORM, 4)),
        ];
        let r = apply(&res, kind, flags, &dict, &edits).unwrap();
        assert!(!r.in_place);
        assert_eq!((r.replaced, r.added, r.removed), (1, 2, 0));
        let (res2, kind2, flags2, d2) = open(&r.file);
        assert_eq!(kind2, 8);
        assert_eq!(flags2 & 0xC000_0000, 0xC000_0000);
        let get = |d: &TextureDictionary, n: &str| d.textures.iter().find(|t| t.name == n).cloned().unwrap();
        let stone = get(&d2, "stone");
        assert_eq!((stone.width, stone.format, stone.data[0]), (64, Format::BC3_UNORM, 5));
        let grass = get(&d2, "grass");
        assert_eq!((grass.width, grass.format, grass.data.len()), (8, Format::B8G8R8A8_UNORM, 256));
        // Hashes sorted, matching the textures.
        let hashes: Vec<u32> =
            (0..3).map(|i| res2.u32(res2.u32(Resource::ROOT + 0x10).unwrap() as u64 + i * 4).unwrap()).collect();
        assert!(hashes.is_sorted());
        let names: Vec<u32> = d2.textures.iter().map(|t| name_hash(&t.name)).collect();
        assert_eq!(hashes, names);

        // Remove one again.
        let r = apply(&res2, kind2, flags2, &d2, &[Edit::Remove("grass".into())]).unwrap();
        let (_, _, _, d3) = open(&r.file);
        let mut n: Vec<&str> = d3.textures.iter().map(|t| t.name.as_str()).collect();
        n.sort();
        assert_eq!(n, ["moss", "stone"]);
        assert!(apply(&res2, kind2, flags2, &d2, &[Edit::Remove("nope".into())]).is_err());
    }

    /// The one-texture dictionary moved to 0x200 inside a "model" whose
    /// graphics memory starts with other data (vertex buffers).
    fn model() -> (Resource, u32, u32) {
        let (res, kind, flags, _) = open(&dictionary());
        let mut sys = vec![0u8; 0x1000];
        // Same layout 0x200 further on: shift every system pointer by 0x200.
        sys[0x200..0x300].copy_from_slice(&res.system[..0x100]);
        for at in [0x210, 0x218, 0x240, 0x280 + 0x14] {
            let v = get_u32(&sys, at) + 0x200;
            put_u32(&mut sys, at, v);
        }
        let mut gfx = vec![0x11u8; 0x1000]; // "vertex data"
        gfx[0x800..0x808].fill(0xAB);
        put_u32(&mut sys, 0x280 + 0x48, 0x6000_0800);
        (Resource { version: kind, system: sys, graphics: gfx }, kind, flags)
    }

    #[test]
    fn embedded_keeps_other_data_and_structures() {
        let (res, kind, flags) = model();
        let dict = parse_gta4_at(&res, 0x5000_0200).unwrap();
        assert_eq!(dict.textures[0].data, vec![0xAB; 8]);
        let in_use = vec![(0x5000_0280u64, "gta_default".to_string())];
        let edits = [Edit::Set(texture("stone", 64, Format::BC3_UNORM, 5))];
        let r = apply_embedded(&res, kind, flags, 0x5000_0200, &in_use, &dict, &edits).unwrap();
        assert!(!r.in_place);
        let back = Resource::from_rsc5(&r.file).unwrap();
        // Other graphics data untouched, the structure still where the shader points.
        assert_eq!(&back.graphics[..0x800], &res.graphics[..0x800]);
        assert_eq!(back.u32(0x5000_0200 + 0x18).and_then(|l| back.u32(l as u64)).unwrap(), 0x5000_0280);
        let d2 = parse_gta4_at(&back, 0x5000_0200).unwrap();
        assert_eq!((d2.textures[0].width, d2.textures[0].data[0]), (64, 5));
        // A texture the shader uses cannot be removed.
        let err = apply_embedded(&res, kind, flags, 0x5000_0200, &in_use, &dict, &[Edit::Remove("stone".into())]);
        assert!(err.unwrap_err().to_string().contains("gta_default"));
        // An unchanged rebuild is byte for byte the same.
        let same = Resource::from_rsc5(&rebuild_embedded(&res, kind, flags, 0x5000_0200, &dict).unwrap()).unwrap();
        assert!(same.system == res.system && same.graphics == res.graphics);
    }
}
