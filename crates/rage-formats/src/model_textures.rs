//! Changing the textures embedded in GTA V models (`.ydr`, `.yft`, `.ydd`).
//!
//! A model's resource holds much more than its textures (geometry,
//! skeletons, physics), linked by pointers OpenRAGE does not all know, so it
//! is never laid out anew. Everything stays where it is:
//!
//! * a replacement whose pixel data fits into the old texture's data is
//!   written over it (any size, format and mip count), and the texture's
//!   structure gets the new shape;
//! * larger pixel data, added textures (structure, name), grown name lists
//!   and a grown pages-info block go into pages appended to the resource
//!   ([`rage_resource::pages`]), which moves nothing that is already there;
//! * removed textures leave their lists in place (shorter); their data stays
//!   as unreferenced bytes.
//!
//! Shaders refer to textures by name, so a texture can be replaced, added or
//! removed without touching them. The hash list stays sorted, as the game
//! looks names up by binary search. A `.ydd` has a texture list per drawable:
//! a replacement or removal applies to every list with that name; adding
//! needs a model with exactly one list.

use rage_resource::pages;
use rage_resource::{Resource, ResourceError};

use crate::texture::Texture;
use crate::ytd::{self, Layout};
use crate::ytd_write::{Edit, EditResult, name_hash, write_fields};

const SYSTEM_BASE: u64 = 0x5000_0000;
const GRAPHICS_BASE: u64 = 0x6000_0000;
/// Alignment of new pixel data.
const DATA_ALIGN: u64 = 256;

fn bad(msg: impl Into<String>) -> ResourceError {
    ResourceError::Format(msg.into())
}

/// Offsets in a GTA V texture structure.
fn data_field(layout: Layout) -> u64 {
    if layout == Layout::Enhanced { 0x38 } else { 0x70 }
}
const NAME_FIELD: u64 = 0x28;

/// A block of new bytes in an appended page.
struct NewBlock {
    graphics: bool,
    data: Vec<u8>,
    align: u64,
    /// For pixel data: the texture and the size of its old data (for messages).
    texture: Option<(String, usize)>,
}

/// Where a pointer is written: existing memory, or inside a new block.
#[derive(Clone, Copy)]
enum At {
    Address(u64),
    Block(usize, u64),
}

/// Edits the dictionaries at `dicts` (addresses of `pgDictionary<grcTexture>`
/// inside the model `res`, whose page flags are given). Returns a standalone
/// RSC7 file; `in_place` means no pages were added.
pub fn apply(
    res: &Resource,
    system_flags: u32,
    graphics_flags: u32,
    enhanced: bool,
    dicts: &[u64],
    edits: &[Edit],
) -> Result<EditResult, ResourceError> {
    let layout = if enhanced { Layout::Enhanced } else { Layout::Legacy };
    if dicts.is_empty() {
        return Err(bad("the model has no textures of its own"));
    }
    let parsed: Vec<ytd::TextureDictionary> =
        dicts.iter().map(|&d| ytd::parse_at(res, d, enhanced)).collect::<Result<_, _>>()?;
    if let Some(e) = parsed.iter().flat_map(|d| &d.errors).next() {
        return Err(bad(format!("the model has unreadable textures ({e}); refusing to change it")));
    }
    for e in edits {
        if let Edit::Set(t) = e {
            if t.expected_size().is_none_or(|n| n > t.data.len()) {
                return Err(bad(format!("{}: pixel data is shorter than its format and size require", t.name)));
            }
            if !enhanced && t.format.to_d3d9().is_none() {
                return Err(bad(format!("{}: {} is not available in GTA V Legacy", t.name, t.format.name())));
            }
        }
    }
    // Structure addresses, parallel to each dictionary's textures.
    let addresses: Vec<Vec<u64>> = dicts
        .iter()
        .zip(&parsed)
        .map(|(&d, p)| {
            let list = res.u64(d + 0x30)?;
            (0..p.textures.len() as u64).map(|i| res.u64(list + i * 8)).collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<_, _>>()?;

    let mut out = res.clone();
    let mut blocks: Vec<NewBlock> = Vec::new();
    let mut fixups: Vec<(At, At)> = Vec::new(); // (where, what it points to)
    // Final (name, structure) per dictionary, when its lists change.
    let mut lists: Vec<Option<Vec<(String, At)>>> = vec![None; dicts.len()];
    let entries = |d: usize| -> Vec<(String, At)> {
        parsed[d].textures.iter().zip(&addresses[d]).map(|(t, &a)| (t.name.clone(), At::Address(a))).collect()
    };
    let (mut replaced, mut added, mut removed) = (0, 0, 0);
    let df = data_field(layout);
    let size = layout.texture_size();

    for edit in edits {
        match edit {
            Edit::Set(new) => {
                let mut found = false;
                for d in 0..dicts.len() {
                    let Some(i) = parsed[d].textures.iter().position(|t| t.name.eq_ignore_ascii_case(&new.name)) else {
                        continue;
                    };
                    found = true;
                    let old = &parsed[d].textures[i];
                    let tex = Texture {
                        name: old.name.clone(),
                        usage: if new.usage == 0 { old.usage } else { new.usage },
                        ..new.clone()
                    };
                    let len = tex.expected_size().expect("checked above");
                    let at = addresses[d][i];
                    write_fields(layout, out.bytes_mut(at, size)?, &tex, false)?;
                    if len <= old.data.len() {
                        out.bytes_mut(parsed[d].stored[i].data_ptr, len)?.copy_from_slice(&tex.data[..len]);
                    } else {
                        blocks.push(NewBlock {
                            graphics: true,
                            data: tex.data[..len].to_vec(),
                            align: DATA_ALIGN,
                            texture: Some((tex.name.clone(), old.data.len())),
                        });
                        fixups.push((At::Address(at + df), At::Block(blocks.len() - 1, 0)));
                    }
                }
                if found {
                    replaced += 1;
                    continue;
                }
                if dicts.len() != 1 {
                    return Err(bad(format!(
                        "{}: this model has {} texture lists; new textures can only be added to a model with one",
                        new.name,
                        dicts.len()
                    )));
                }
                let mut t = parsed[0]
                    .stored
                    .first()
                    .map(|s| s.template.clone())
                    .ok_or_else(|| bad("cannot add a texture to an empty texture list (no template texture)"))?;
                t.resize(size, 0);
                write_fields(layout, &mut t, new, true)?;
                let len = new.expected_size().expect("checked above");
                blocks.push(NewBlock { graphics: false, data: t, align: 16, texture: None });
                let tex_block = blocks.len() - 1;
                let mut name = new.name.clone().into_bytes();
                name.push(0);
                blocks.push(NewBlock { graphics: false, data: name, align: 16, texture: None });
                fixups.push((At::Block(tex_block, NAME_FIELD), At::Block(blocks.len() - 1, 0)));
                blocks.push(NewBlock {
                    graphics: true,
                    data: new.data[..len].to_vec(),
                    align: DATA_ALIGN,
                    texture: None,
                });
                fixups.push((At::Block(tex_block, df), At::Block(blocks.len() - 1, 0)));
                if enhanced {
                    // The embedded shader resource view.
                    fixups.push((At::Block(tex_block, 0x30), At::Block(tex_block, 0x58)));
                }
                lists[0].get_or_insert_with(|| entries(0)).push((new.name.clone(), At::Block(tex_block, 0)));
                added += 1;
            }
            Edit::Remove(name) => {
                let mut found = false;
                for (d, list) in lists.iter_mut().enumerate() {
                    let list = list.get_or_insert_with(|| entries(d));
                    let before = list.len();
                    list.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
                    found |= list.len() != before;
                }
                if !found {
                    return Err(bad(format!("{name}: no such texture")));
                }
                removed += 1;
            }
        }
    }

    // Name hash and pointer lists of changed dictionaries.
    for (d, list) in lists.iter_mut().enumerate() {
        let Some(list) = list else { continue };
        list.sort_by_key(|(n, _)| name_hash(n));
        let txd = dicts[d];
        let count = list.len() as u16;
        let capacity = out.u16(txd + 0x3A)?.min(out.u16(txd + 0x2A)?);
        let has_new = list.iter().any(|(_, a)| matches!(a, At::Block(..)));
        let hashes: Vec<u8> = list.iter().flat_map(|(n, _)| name_hash(n).to_le_bytes()).collect();
        if !has_new && count <= capacity {
            let (h, p) = (out.u64(txd + 0x20)?, out.u64(txd + 0x30)?);
            out.bytes_mut(h, hashes.len())?.copy_from_slice(&hashes);
            for (k, (_, a)) in list.iter().enumerate() {
                let At::Address(a) = a else { unreachable!("no new entries") };
                out.bytes_mut(p + k as u64 * 8, 8)?.copy_from_slice(&a.to_le_bytes());
            }
        } else {
            blocks.push(NewBlock { graphics: false, data: hashes, align: 16, texture: None });
            fixups.push((At::Address(txd + 0x20), At::Block(blocks.len() - 1, 0)));
            blocks.push(NewBlock { graphics: false, data: vec![0; list.len() * 8], align: 16, texture: None });
            let ptrs = blocks.len() - 1;
            fixups.push((At::Address(txd + 0x30), At::Block(ptrs, 0)));
            for (k, (_, a)) in list.iter().enumerate() {
                fixups.push((At::Block(ptrs, k as u64 * 8), *a));
            }
            for at in [0x2A, 0x3A] {
                out.bytes_mut(txd + at, 2)?.copy_from_slice(&count.to_le_bytes());
            }
        }
        for at in [0x28, 0x38] {
            out.bytes_mut(txd + at, 2)?.copy_from_slice(&count.to_le_bytes());
        }
    }

    if blocks.is_empty() {
        let file = rage_resource::builder::repack(&out, system_flags, graphics_flags)?;
        return Ok(EditResult { file, in_place: true, replaced, added, removed });
    }

    // Lay out the new blocks in appended pages. The pages-info block (if
    // any) lists every page, so it grows with them: iterate to a fixed point.
    let sys_pages = pages::Pages::from_flags(system_flags).count();
    let gfx_blocks: Vec<usize> = (0..blocks.len()).filter(|&b| blocks[b].graphics).collect();
    let gfx = pages::append(
        graphics_flags,
        sys_pages,
        &gfx_blocks.iter().map(|&b| (blocks[b].data.len() as u64, blocks[b].align)).collect::<Vec<_>>(),
    )
    .map_err(|e| {
        // Say which texture does not fit and what would.
        let room = pages::largest_new_page(graphics_flags, sys_pages);
        let Some(b) = gfx_blocks.iter().map(|&b| &blocks[b]).max_by_key(|b| b.data.len()) else { return e };
        match &b.texture {
            Some((name, old)) => bad(format!(
                "{name}: its pixel data ({} bytes) does not fit into the model. The old texture's place holds \
                 {old} bytes and the model has room for at most {room} more; use the same resolution and \
                 format as the original, or a smaller one",
                b.data.len()
            )),
            None => bad(format!(
                "the new texture's pixel data ({} bytes) does not fit into the model, which has room for at \
                 most {room} more bytes; use a smaller texture",
                b.data.len()
            )),
        }
    })?;
    let gfx_count = pages::Pages::from_flags(gfx.flags).count();
    let pages_info = out.u64(Resource::ROOT + 0x08).unwrap_or(0);
    let mut sys_blocks: Vec<usize> = (0..blocks.len()).filter(|&b| !blocks[b].graphics).collect();
    let mut guess = sys_pages + gfx_count + 1;
    let sys = loop {
        let mut sizes: Vec<(u64, u64)> =
            sys_blocks.iter().map(|&b| (blocks[b].data.len() as u64, blocks[b].align)).collect();
        if pages_info != 0 {
            sizes.push((16 + 8 * guess as u64, 16));
        }
        let sys = pages::append(system_flags, gfx_count, &sizes)?;
        let total = pages::Pages::from_flags(sys.flags).count() + gfx_count;
        if pages_info == 0 || total == guess {
            break sys;
        }
        guess = total;
    };
    let sys_count = pages::Pages::from_flags(sys.flags).count();
    if pages_info != 0 {
        let mut info = out.bytes(pages_info, 16)?.to_vec();
        info[8] = sys_count as u8;
        info[9] = gfx_count as u8;
        info.resize(16 + 8 * guess as usize, 0);
        blocks.push(NewBlock { graphics: false, data: info, align: 16, texture: None });
        sys_blocks.push(blocks.len() - 1);
        fixups.push((At::Address(Resource::ROOT + 0x08), At::Block(blocks.len() - 1, 0)));
    }
    let mut address = vec![0u64; blocks.len()];
    for (k, &b) in gfx_blocks.iter().enumerate() {
        address[b] = GRAPHICS_BASE + gfx.offsets[k];
    }
    for (k, &b) in sys_blocks.iter().enumerate() {
        address[b] = SYSTEM_BASE + sys.offsets[k];
    }
    out.system.resize(sys.len as usize, 0);
    out.graphics.resize(gfx.len as usize, 0);
    for (b, block) in blocks.iter().enumerate() {
        out.bytes_mut(address[b], block.data.len())?.copy_from_slice(&block.data);
    }
    let resolve = |a: At| match a {
        At::Address(x) => x,
        At::Block(b, off) => address[b] + off,
    };
    for (at, to) in fixups {
        let value = resolve(to);
        out.bytes_mut(resolve(at), 8)?.copy_from_slice(&value.to_le_bytes());
    }
    let file = rage_resource::builder::repack(&out, sys.flags, gfx.flags)?;
    Ok(EditResult { file, in_place: false, replaced, added, removed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::{Dimension, Format};
    use crate::ytd::{Stored, TextureDictionary};

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

    /// A dictionary resource standing in for a model (the dictionary at the root, with a pages-info block).
    fn model(layout: Layout) -> (Resource, u32, u32) {
        let textures = vec![texture("alpha", 64, Format::BC1_UNORM, 1), texture("beta", 16, Format::BC1_UNORM, 2)];
        let mut root = vec![0u8; 64];
        root[8] = 1;
        let dict = TextureDictionary {
            stored: textures.iter().map(|_| Stored { template: vec![0; layout.texture_size()], data_ptr: 0 }).collect(),
            textures,
            errors: Vec::new(),
            layout,
            version: if layout == Layout::Enhanced { 5 } else { 13 },
            root,
        };
        let file = crate::ytd_write::rebuild(&dict).unwrap();
        let word = |i: usize| u32::from_le_bytes(file[i..i + 4].try_into().unwrap());
        (Resource::from_rsc7(&file).unwrap(), word(8), word(12))
    }

    fn read(file: &[u8], enhanced: bool) -> (Resource, Vec<Texture>) {
        let res = Resource::from_rsc7(file).unwrap();
        let mut t = ytd::parse_at(&res, Resource::ROOT, enhanced).unwrap().textures;
        t.sort_by(|a, b| a.name.cmp(&b.name));
        (res, t)
    }

    #[test]
    fn edits_without_moving_anything() {
        for layout in [Layout::Legacy, Layout::Enhanced] {
            let enhanced = layout == Layout::Enhanced;
            let (res, sf, gf) = model(layout);
            let root = [Resource::ROOT];
            // Smaller data: written where the old texture was.
            let r =
                apply(&res, sf, gf, enhanced, &root, &[Edit::Set(texture("ALPHA", 32, Format::BC3_UNORM, 7))]).unwrap();
            assert!(r.in_place, "{layout:?}");
            let (_, t) = read(&r.file, enhanced);
            assert_eq!(
                (t[0].name.as_str(), t[0].width, t[0].format, t[0].data[0]),
                ("alpha", 32, Format::BC3_UNORM, 7)
            );
            // Larger data and an added texture: into appended pages; the old bytes stay where they were.
            let edits = [
                Edit::Set(texture("beta", 128, Format::BC1_UNORM, 5)),
                Edit::Set(texture("gamma", 8, Format::BC1_UNORM, 9)),
            ];
            let r = apply(&res, sf, gf, enhanced, &root, &edits).unwrap();
            assert!(!r.in_place);
            assert_eq!((r.replaced, r.added), (1, 1));
            let (back, t) = read(&r.file, enhanced);
            // (Only the pages-info and list pointers of the root change.)
            assert_eq!(&back.system[..0x08], &res.system[..0x08]);
            assert_ne!(back.u64(Resource::ROOT + 0x08).unwrap(), res.u64(Resource::ROOT + 0x08).unwrap());
            assert_eq!(back.graphics[..res.graphics.len()], res.graphics[..]);
            assert_eq!(t.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["alpha", "beta", "gamma"]);
            assert_eq!((t[1].width, t[1].data[0]), (128, 5));
            assert_eq!((t[2].width, t[2].data[0]), (8, 9));
            assert_eq!(t[0].data[0], 1, "untouched texture keeps its data");
            // The pages-info block counts every page.
            let info = back.u64(Resource::ROOT + 0x08).unwrap();
            let pages = |f: u32| rage_resource::pages::Pages::from_flags(f).count() as u8;
            let word = |i: usize| u32::from_le_bytes(r.file[i..i + 4].try_into().unwrap());
            assert_eq!((back.u8(info + 8).unwrap(), back.u8(info + 9).unwrap()), (pages(word(8)), pages(word(12))));
            // The hash list stays sorted.
            let hashes: Vec<u32> =
                (0..3).map(|i| back.u32(back.u64(Resource::ROOT + 0x20).unwrap() + i * 4).unwrap()).collect();
            assert!(hashes.is_sorted());
            // Removing shortens the lists in place.
            let r = apply(&res, sf, gf, enhanced, &root, &[Edit::Remove("beta".into())]).unwrap();
            assert!(r.in_place);
            let (_, t) = read(&r.file, enhanced);
            assert_eq!(t.len(), 1);
            assert!(apply(&res, sf, gf, enhanced, &root, &[Edit::Remove("nope".into())]).is_err());
        }
    }
}
