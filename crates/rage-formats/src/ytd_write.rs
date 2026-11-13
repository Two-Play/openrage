//! Writing texture dictionaries: replacing, adding and removing textures.
//!
//! When every edit replaces a texture with one of identical shape (format,
//! size, depth and mip count), only the pixel data is overwritten and the
//! resource keeps its exact layout. Any other edit rebuilds the dictionary
//! from the original structures (used as templates, so fields OpenRAGE does
//! not interpret are preserved) with a fresh page layout.

use rage_resource::builder::{Builder, Segment};
use rage_resource::{Resource, ResourceError};

use crate::texture::Texture;
use crate::ytd::{Layout, TextureDictionary};

#[derive(Debug, Clone)]
pub enum Edit {
    /// Replace the texture with the same name (case-insensitive) or add it.
    Set(Texture),
    /// Remove the texture with this name.
    Remove(String),
}

#[derive(Debug, Clone)]
pub struct EditResult {
    /// The new resource as a standalone RSC7 file.
    pub file: Vec<u8>,
    /// True if only pixel data was overwritten (layout untouched).
    pub in_place: bool,
    pub replaced: usize,
    pub added: usize,
    pub removed: usize,
}

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

/// joaat of the lowercase name, as used by the dictionary's hash list.
pub(crate) fn name_hash(name: &str) -> u32 {
    let mut h: u32 = 0;
    for b in name.bytes().map(|b| b.to_ascii_lowercase()) {
        h = h.wrapping_add(b as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

/// Usage class guessed from common GTA naming conventions (for added textures).
fn guess_usage(name: &str) -> u8 {
    let n = name.to_ascii_lowercase();
    if n.ends_with("_n") || n.contains("_normal") {
        22
    } else if n.ends_with("_s") || n.contains("_spec") {
        23
    } else {
        20
    }
}

fn same_shape(a: &Texture, b: &Texture) -> bool {
    (a.format, a.width, a.height, a.depth, a.levels) == (b.format, b.width, b.height, b.depth, b.levels)
        && a.data.len() == b.data.len()
}

/// Applies `edits` to a dictionary read from `res` (whose entry had the given page flags).
pub fn apply(
    res: &Resource,
    system_flags: u32,
    graphics_flags: u32,
    dict: &TextureDictionary,
    edits: &[Edit],
) -> Result<EditResult, ResourceError> {
    apply_inner(res, system_flags, graphics_flags, dict, edits, false)
}

/// Rebuilds a dictionary with a fresh layout and no changes (used to verify the writer).
pub fn rebuild(dict: &TextureDictionary) -> Result<Vec<u8>, ResourceError> {
    let res = Resource { version: dict.version, system: Vec::new(), graphics: Vec::new() };
    Ok(apply_inner(&res, 0, 0, dict, &[], true)?.file)
}

fn apply_inner(
    res: &Resource,
    system_flags: u32,
    graphics_flags: u32,
    dict: &TextureDictionary,
    edits: &[Edit],
    force_rebuild: bool,
) -> Result<EditResult, ResourceError> {
    if !dict.errors.is_empty() {
        return Err(bad(format!("dictionary has unreadable textures ({}); refusing to rewrite it", dict.errors[0])));
    }
    let find = |name: &str| dict.textures.iter().position(|t| t.name.eq_ignore_ascii_case(name));
    for e in edits {
        if let Edit::Set(t) = e
            && t.expected_size().is_none_or(|n| n > t.data.len())
        {
            return Err(bad(format!("{}: pixel data is shorter than its format and size require", t.name)));
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
        let file = rage_resource::builder::repack(&out, system_flags, graphics_flags)?;
        return Ok(EditResult { file, in_place: true, replaced: edits.len(), added: 0, removed: 0 });
    }

    // Rebuild: final texture list with templates.
    let mut list: Vec<(Texture, Vec<u8>, bool)> =
        dict.textures.iter().zip(&dict.stored).map(|(t, s)| (t.clone(), s.template.clone(), false)).collect();
    let (mut replaced, mut added, mut removed) = (0, 0, 0);
    for e in edits {
        match e {
            Edit::Remove(name) => {
                let before = list.len();
                list.retain(|(t, _, _)| !t.name.eq_ignore_ascii_case(name));
                if list.len() == before {
                    return Err(bad(format!("{name}: no such texture")));
                }
                removed += 1;
            }
            Edit::Set(new) => match list.iter_mut().find(|(t, _, _)| t.name.eq_ignore_ascii_case(&new.name)) {
                Some(slot) => {
                    let usage = if new.usage == 0 { slot.0.usage } else { new.usage };
                    slot.0 = Texture { usage, name: slot.0.name.clone(), ..new.clone() };
                    replaced += 1;
                }
                None => {
                    let template = dict
                        .stored
                        .first()
                        .map(|s| s.template.clone())
                        .ok_or_else(|| bad("cannot add a texture to an empty dictionary (no template texture)"))?;
                    let usage = if new.usage == 0 { guess_usage(&new.name) } else { new.usage };
                    list.push((Texture { usage, ..new.clone() }, template, true));
                    added += 1;
                }
            },
        }
    }
    // The game looks textures up by binary search over the sorted hash list.
    list.sort_by_key(|(t, _, _)| name_hash(&t.name));

    let mut b = Builder::new();
    let mut root_bytes = dict.root.clone();
    root_bytes.resize(64, 0);
    let root = b.add(Segment::System, root_bytes);
    if u64::from_le_bytes(dict.root[8..16].try_into().unwrap()) != 0 {
        b.add_pages_info(root, 0x08);
    }
    let count = list.len() as u16;
    let hashes: Vec<u8> = list.iter().flat_map(|(t, _, _)| name_hash(&t.name).to_le_bytes()).collect();
    let hash_block = b.add(Segment::System, hashes);
    let ptr_block = b.add(Segment::System, vec![0u8; list.len() * 8]);
    {
        let r = b.data_mut(root);
        r[0x28..0x2A].copy_from_slice(&count.to_le_bytes());
        r[0x2A..0x2C].copy_from_slice(&count.to_le_bytes());
        r[0x38..0x3A].copy_from_slice(&count.to_le_bytes());
        r[0x3A..0x3C].copy_from_slice(&count.to_le_bytes());
    }
    b.pointer(root, 0x20, hash_block, 0);
    b.pointer(root, 0x30, ptr_block, 0);

    for (i, (tex, template, is_new)) in list.iter().enumerate() {
        let mut t = template.clone();
        t.resize(dict.layout.texture_size(), 0);
        let size = tex.expected_size().ok_or_else(|| bad(format!("{}: unsupported format", tex.name)))?;
        match dict.layout {
            Layout::Legacy => {
                let code = tex.format.to_d3d9().ok_or_else(|| {
                    bad(format!("{}: {} is not available in GTA V Legacy", tex.name, tex.format.name()))
                })?;
                let slice = tex.level_size(0).unwrap_or(0);
                let stride = (slice / tex.height.max(1) as usize) as u16;
                t[0x50..0x52].copy_from_slice(&(tex.width as u16).to_le_bytes());
                t[0x52..0x54].copy_from_slice(&(tex.height as u16).to_le_bytes());
                t[0x54..0x56].copy_from_slice(&(tex.depth as u16).to_le_bytes());
                t[0x56..0x58].copy_from_slice(&stride.to_le_bytes());
                t[0x58..0x5C].copy_from_slice(&code.to_le_bytes());
                t[0x5D] = tex.levels as u8;
            }
            Layout::Rdr2 => return Err(bad("Red Dead Redemption 2 texture dictionaries cannot be written yet")),
            Layout::Gta4 => return Err(bad("GTA IV texture dictionaries cannot be written yet")),
            Layout::Enhanced => {
                let blocks = gen9_block_count(tex);
                t[0x08..0x0C].copy_from_slice(&blocks.to_le_bytes());
                t[0x0C..0x10].copy_from_slice(&tex.format.block_or_pixel_bytes().to_le_bytes());
                t[0x18..0x1A].copy_from_slice(&(tex.width as u16).to_le_bytes());
                t[0x1A..0x1C].copy_from_slice(&(tex.height as u16).to_le_bytes());
                t[0x1C..0x1E].copy_from_slice(&(tex.depth as u16).to_le_bytes());
                t[0x1F] = tex.format.0 as u8;
                t[0x22] = tex.levels as u8;
            }
        }
        if *is_new || tex.usage != 0 {
            let usage_at = 0x40;
            let old = u32::from_le_bytes(t[usage_at..usage_at + 4].try_into().unwrap());
            let new = (old & !0x1F) | (tex.usage as u32 & 0x1F);
            t[usage_at..usage_at + 4].copy_from_slice(&new.to_le_bytes());
        }
        let tex_block = b.add(Segment::System, t);
        let mut name = tex.name.clone().into_bytes();
        name.push(0);
        let name_block = b.add(Segment::System, name);
        let data_block = b.add(Segment::Graphics, tex.data[..size].to_vec());
        b.pointer(ptr_block, i * 8, tex_block, 0);
        b.pointer(tex_block, 0x28, name_block, 0);
        match dict.layout {
            Layout::Legacy => b.pointer(tex_block, 0x70, data_block, 0),
            Layout::Rdr2 | Layout::Gta4 => unreachable!("rejected above"),
            Layout::Enhanced => {
                b.pointer(tex_block, 0x30, tex_block, 0x58); // embedded shader resource view
                b.pointer(tex_block, 0x38, data_block, 0);
            }
        }
    }
    let file = b.build(dict.version)?;
    Ok(EditResult { file, in_place: false, replaced, added, removed })
}

/// Number of format blocks (or pixels) over all mips, as GTA V Enhanced stores it.
///
/// Every mip level is `max(1, size >> level)` (no rounding to powers of two;
/// verified against all textures of the game).
fn gen9_block_count(t: &Texture) -> u32 {
    let per_block = if t.format.block_bytes().is_some() { 4 } else { 1 };
    let depth = t.depth.max(1);
    (0..t.levels)
        .map(|l| {
            let w = (t.width >> l).max(1);
            let h = (t.height >> l).max(1);
            w.div_ceil(per_block) * h.div_ceil(per_block) * depth
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_counts_match_the_game() {
        let t = |w, h, levels, format| Texture {
            name: String::new(),
            width: w,
            height: h,
            depth: 1,
            levels,
            format,
            dimension: crate::texture::Dimension::Texture2D,
            usage: 0,
            data: Vec::new(),
        };
        // Values read from GTA V Enhanced dictionaries (non-power-of-two sizes).
        assert_eq!(gen9_block_count(&t(356, 512, 10, crate::texture::Format::BC3_UNORM)), 0x3B93);
        assert_eq!(gen9_block_count(&t(256, 144, 9, crate::texture::Format::BC1_UNORM)), 0x0C09);
    }

    #[test]
    fn hashes_match_joaat() {
        assert_eq!(name_hash("ADDER"), 0xB779_A091);
    }

    use crate::texture::{Dimension, Format};
    use crate::ytd::{Stored, parse};

    fn texture(name: &str, size: u32, format: Format, fill: u8) -> Texture {
        let t = Texture {
            name: name.into(),
            width: size,
            height: size,
            depth: 1,
            levels: 1,
            format,
            dimension: Dimension::Texture2D,
            usage: 20,
            data: Vec::new(),
        };
        let n = t.expected_size().unwrap();
        Texture { data: vec![fill; n], ..t }
    }

    /// A dictionary as if read from a file, with zeroed templates.
    fn synthetic(layout: Layout) -> TextureDictionary {
        let textures = vec![texture("alpha", 8, Format::BC1_UNORM, 1), texture("beta", 4, Format::B8G8R8A8_UNORM, 2)];
        let mut root = vec![0u8; 64];
        root[8] = 1; // has a pages-info block
        TextureDictionary {
            stored: textures.iter().map(|_| Stored { template: vec![0; layout.texture_size()], data_ptr: 0 }).collect(),
            textures,
            errors: Vec::new(),
            layout,
            version: if layout == Layout::Enhanced { 5 } else { 13 },
            root,
        }
    }

    fn names(d: &TextureDictionary) -> Vec<String> {
        let mut n: Vec<String> = d.textures.iter().map(|t| t.name.clone()).collect();
        n.sort();
        n
    }

    #[test]
    fn rebuild_and_edit_both_layouts() {
        for layout in [Layout::Legacy, Layout::Enhanced] {
            let dict = synthetic(layout);
            let file = rebuild(&dict).unwrap();
            let res = Resource::from_rsc7(&file).unwrap();
            let back = parse(&res).unwrap();
            assert_eq!(names(&back), ["alpha", "beta"], "{layout:?}");
            assert!(back.errors.is_empty());
            let alpha = back.textures.iter().find(|t| t.name == "alpha").unwrap();
            assert_eq!((alpha.width, alpha.format, alpha.data[0]), (8, Format::BC1_UNORM, 1));

            // Same shape: in place.
            let flags = |i: usize| u32::from_le_bytes(file[8 + i * 4..12 + i * 4].try_into().unwrap());
            let r = apply(&res, flags(0), flags(1), &back, &[Edit::Set(texture("ALPHA", 8, Format::BC1_UNORM, 9))])
                .unwrap();
            assert!(r.in_place);
            let d2 = parse(&Resource::from_rsc7(&r.file).unwrap()).unwrap();
            assert_eq!(d2.textures.iter().find(|t| t.name == "alpha").unwrap().data[0], 9);

            // Different shape, add and remove: rebuilt.
            let edits = [
                Edit::Set(texture("alpha", 16, Format::BC3_UNORM, 7)),
                Edit::Set(texture("gamma", 4, Format::BC1_UNORM, 3)),
                Edit::Remove("beta".into()),
            ];
            let r = apply(&res, flags(0), flags(1), &back, &edits).unwrap();
            assert!(!r.in_place);
            assert_eq!((r.replaced, r.added, r.removed), (1, 1, 1));
            let d3 = parse(&Resource::from_rsc7(&r.file).unwrap()).unwrap();
            assert_eq!(names(&d3), ["alpha", "gamma"]);
            let alpha = d3.textures.iter().find(|t| t.name == "alpha").unwrap();
            assert_eq!((alpha.width, alpha.format, alpha.usage), (16, Format::BC3_UNORM, 20));
            assert!(apply(&res, 0, 0, &back, &[Edit::Remove("nope".into())]).is_err());
        }
    }
}
