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

/// Final (name, structure) per dictionary, when its lists change.
type Lists = Vec<Option<Vec<(String, At)>>>;

/// The model being edited, with the new blocks and pointers collected so far.
struct Editor<'a> {
    dicts: &'a [u64],
    enhanced: bool,
    layout: Layout,
    parsed: Vec<ytd::TextureDictionary>,
    /// Structure addresses, parallel to each dictionary's textures.
    addresses: Vec<Vec<u64>>,
    out: Resource,
    blocks: Vec<NewBlock>,
    /// (where, what it points to)
    fixups: Vec<(At, At)>,
    lists: Lists,
    replaced: usize,
    added: usize,
    removed: usize,
}

/// Parses the dictionaries; none may be missing or unreadable.
fn parse_dictionaries(
    res: &Resource,
    dicts: &[u64],
    enhanced: bool,
) -> Result<Vec<ytd::TextureDictionary>, ResourceError> {
    if dicts.is_empty() {
        return Err(bad("the model has no textures of its own"));
    }
    let parsed: Vec<ytd::TextureDictionary> =
        dicts.iter().map(|&d| ytd::parse_at(res, d, enhanced)).collect::<Result<_, _>>()?;
    if let Some(e) = parsed.iter().flat_map(|d| &d.errors).next() {
        return Err(bad(format!("the model has unreadable textures ({e}); refusing to change it")));
    }
    Ok(parsed)
}

/// Rejects replacements the model's format cannot take.
fn check_edits(edits: &[Edit], enhanced: bool) -> Result<(), ResourceError> {
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
    Ok(())
}

fn texture_addresses(
    res: &Resource,
    dicts: &[u64],
    parsed: &[ytd::TextureDictionary],
) -> Result<Vec<Vec<u64>>, ResourceError> {
    dicts
        .iter()
        .zip(parsed)
        .map(|(&d, p)| {
            let list = res.u64(d + 0x30)?;
            (0..p.textures.len() as u64).map(|i| res.u64(list + i * 8)).collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<_, _>>()
}

/// The address of every block, from the offsets of its page segment's blocks.
fn block_addresses(count: usize, gfx: (&[usize], &[u64]), sys: (&[usize], &[u64])) -> Vec<u64> {
    let mut address = vec![0u64; count];
    for (k, &b) in gfx.0.iter().enumerate() {
        address[b] = GRAPHICS_BASE + gfx.1[k];
    }
    for (k, &b) in sys.0.iter().enumerate() {
        address[b] = SYSTEM_BASE + sys.1[k];
    }
    address
}

impl Editor<'_> {
    fn entries(&self, d: usize) -> Vec<(String, At)> {
        self.parsed[d].textures.iter().zip(&self.addresses[d]).map(|(t, &a)| (t.name.clone(), At::Address(a))).collect()
    }

    fn apply_edit(&mut self, edit: &Edit) -> Result<(), ResourceError> {
        match edit {
            Edit::Set(new) => self.set(new),
            Edit::Remove(name) => self.remove(name),
        }
    }

    fn set(&mut self, new: &Texture) -> Result<(), ResourceError> {
        let mut found = false;
        for d in 0..self.dicts.len() {
            let Some(i) = self.parsed[d].textures.iter().position(|t| t.name.eq_ignore_ascii_case(&new.name)) else {
                continue;
            };
            found = true;
            self.replace(d, i, new)?;
        }
        if found {
            self.replaced += 1;
            return Ok(());
        }
        self.add(new)
    }

    /// Replaces texture `i` of dictionary `d`.
    fn replace(&mut self, d: usize, i: usize, new: &Texture) -> Result<(), ResourceError> {
        let size = self.layout.texture_size();
        let old = &self.parsed[d].textures[i];
        let tex = Texture {
            name: old.name.clone(),
            usage: if new.usage == 0 { old.usage } else { new.usage },
            ..new.clone()
        };
        let len = tex.expected_size().expect("checked above");
        let at = self.addresses[d][i];
        write_fields(self.layout, self.out.bytes_mut(at, size)?, &tex, false)?;
        if len <= old.data.len() {
            self.out.bytes_mut(self.parsed[d].stored[i].data_ptr, len)?.copy_from_slice(&tex.data[..len]);
        } else {
            self.blocks.push(NewBlock {
                graphics: true,
                data: tex.data[..len].to_vec(),
                align: DATA_ALIGN,
                texture: Some((tex.name.clone(), old.data.len())),
            });
            self.fixups.push((At::Address(at + data_field(self.layout)), At::Block(self.blocks.len() - 1, 0)));
        }
        Ok(())
    }

    /// Adds a texture to the model's only list.
    fn add(&mut self, new: &Texture) -> Result<(), ResourceError> {
        if self.dicts.len() != 1 {
            return Err(bad(format!(
                "{}: this model has {} texture lists; new textures can only be added to a model with one",
                new.name,
                self.dicts.len()
            )));
        }
        let df = data_field(self.layout);
        let mut t = self.parsed[0]
            .stored
            .first()
            .map(|s| s.template.clone())
            .ok_or_else(|| bad("cannot add a texture to an empty texture list (no template texture)"))?;
        t.resize(self.layout.texture_size(), 0);
        write_fields(self.layout, &mut t, new, true)?;
        let len = new.expected_size().expect("checked above");
        self.blocks.push(NewBlock { graphics: false, data: t, align: 16, texture: None });
        let tex_block = self.blocks.len() - 1;
        let mut name = new.name.clone().into_bytes();
        name.push(0);
        self.blocks.push(NewBlock { graphics: false, data: name, align: 16, texture: None });
        self.fixups.push((At::Block(tex_block, NAME_FIELD), At::Block(self.blocks.len() - 1, 0)));
        self.blocks.push(NewBlock { graphics: true, data: new.data[..len].to_vec(), align: DATA_ALIGN, texture: None });
        self.fixups.push((At::Block(tex_block, df), At::Block(self.blocks.len() - 1, 0)));
        if self.enhanced {
            // The embedded shader resource view.
            self.fixups.push((At::Block(tex_block, 0x30), At::Block(tex_block, 0x58)));
        }
        let entry = (new.name.clone(), At::Block(tex_block, 0));
        if self.lists[0].is_none() {
            self.lists[0] = Some(self.entries(0));
        }
        if let Some(list) = &mut self.lists[0] {
            list.push(entry);
        }
        self.added += 1;
        Ok(())
    }

    fn remove(&mut self, name: &str) -> Result<(), ResourceError> {
        let mut found = false;
        for d in 0..self.lists.len() {
            if self.lists[d].is_none() {
                self.lists[d] = Some(self.entries(d));
            }
            let Some(list) = &mut self.lists[d] else { continue };
            let before = list.len();
            list.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
            found |= list.len() != before;
        }
        if !found {
            return Err(bad(format!("{name}: no such texture")));
        }
        self.removed += 1;
        Ok(())
    }

    /// Name hash and pointer lists of changed dictionaries.
    fn write_lists(&mut self) -> Result<(), ResourceError> {
        let lists = std::mem::take(&mut self.lists);
        for (d, list) in lists.into_iter().enumerate() {
            let Some(mut list) = list else { continue };
            list.sort_by_key(|(n, _)| name_hash(n));
            self.write_list(self.dicts[d], &list)?;
        }
        Ok(())
    }

    fn write_list(&mut self, txd: u64, list: &[(String, At)]) -> Result<(), ResourceError> {
        let count = list.len() as u16;
        let capacity = self.out.u16(txd + 0x3A)?.min(self.out.u16(txd + 0x2A)?);
        let has_new = list.iter().any(|(_, a)| matches!(a, At::Block(..)));
        let hashes: Vec<u8> = list.iter().flat_map(|(n, _)| name_hash(n).to_le_bytes()).collect();
        if !has_new && count <= capacity {
            self.rewrite_in_place(txd, list, &hashes)?;
        } else {
            self.append_list(txd, list, hashes)?;
        }
        for at in [0x28, 0x38] {
            self.out.bytes_mut(txd + at, 2)?.copy_from_slice(&count.to_le_bytes());
        }
        Ok(())
    }

    /// The lists still fit where they are.
    fn rewrite_in_place(&mut self, txd: u64, list: &[(String, At)], hashes: &[u8]) -> Result<(), ResourceError> {
        let (h, p) = (self.out.u64(txd + 0x20)?, self.out.u64(txd + 0x30)?);
        self.out.bytes_mut(h, hashes.len())?.copy_from_slice(hashes);
        for (k, (_, a)) in list.iter().enumerate() {
            let At::Address(a) = a else { unreachable!("no new entries") };
            self.out.bytes_mut(p + k as u64 * 8, 8)?.copy_from_slice(&a.to_le_bytes());
        }
        Ok(())
    }

    /// The lists move to new blocks, with room for `list.len()` entries.
    fn append_list(&mut self, txd: u64, list: &[(String, At)], hashes: Vec<u8>) -> Result<(), ResourceError> {
        let count = list.len() as u16;
        self.blocks.push(NewBlock { graphics: false, data: hashes, align: 16, texture: None });
        self.fixups.push((At::Address(txd + 0x20), At::Block(self.blocks.len() - 1, 0)));
        self.blocks.push(NewBlock { graphics: false, data: vec![0; list.len() * 8], align: 16, texture: None });
        let ptrs = self.blocks.len() - 1;
        self.fixups.push((At::Address(txd + 0x30), At::Block(ptrs, 0)));
        for (k, (_, a)) in list.iter().enumerate() {
            self.fixups.push((At::Block(ptrs, k as u64 * 8), *a));
        }
        for at in [0x2A, 0x3A] {
            self.out.bytes_mut(txd + at, 2)?.copy_from_slice(&count.to_le_bytes());
        }
        Ok(())
    }

    /// Appended pages for the graphics blocks; says which texture does not fit and what would.
    fn graphics_pages(
        &self,
        graphics_flags: u32,
        sys_pages: u32,
        gfx_blocks: &[usize],
    ) -> Result<pages::Appended, ResourceError> {
        let blocks = &self.blocks;
        pages::append(
            graphics_flags,
            sys_pages,
            &gfx_blocks.iter().map(|&b| (blocks[b].data.len() as u64, blocks[b].align)).collect::<Vec<_>>(),
        )
        .map_err(|e| {
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
        })
    }

    /// The appended system pages. The pages-info block (if any) lists every
    /// page, so it grows with them: iterate to a fixed point. Returns the
    /// pages and the final guess of the total page count.
    fn system_pages(
        &self,
        system_flags: u32,
        gfx_count: u32,
        sys_pages: u32,
        sys_blocks: &[usize],
        pages_info: u64,
    ) -> Result<(pages::Appended, u32), ResourceError> {
        let mut guess = sys_pages + gfx_count + 1;
        loop {
            let mut sizes: Vec<(u64, u64)> =
                sys_blocks.iter().map(|&b| (self.blocks[b].data.len() as u64, self.blocks[b].align)).collect();
            if pages_info != 0 {
                sizes.push((16 + 8 * guess as u64, 16));
            }
            let sys = pages::append(system_flags, gfx_count, &sizes)?;
            let total = pages::Pages::from_flags(sys.flags).count() + gfx_count;
            if pages_info == 0 || total == guess {
                return Ok((sys, guess));
            }
            guess = total;
        }
    }

    /// Copies the blocks to their addresses and fills in the pointers.
    fn write_blocks(&mut self, address: &[u64]) -> Result<(), ResourceError> {
        for (b, block) in self.blocks.iter().enumerate() {
            self.out.bytes_mut(address[b], block.data.len())?.copy_from_slice(&block.data);
        }
        let resolve = |a: At| match a {
            At::Address(x) => x,
            At::Block(b, off) => address[b] + off,
        };
        for &(at, to) in &self.fixups {
            let value = resolve(to);
            self.out.bytes_mut(resolve(at), 8)?.copy_from_slice(&value.to_le_bytes());
        }
        Ok(())
    }

    /// Lays out the new blocks in appended pages and repacks the resource.
    fn finish(mut self, system_flags: u32, graphics_flags: u32) -> Result<EditResult, ResourceError> {
        let sys_pages = pages::Pages::from_flags(system_flags).count();
        let gfx_blocks: Vec<usize> = (0..self.blocks.len()).filter(|&b| self.blocks[b].graphics).collect();
        let gfx = self.graphics_pages(graphics_flags, sys_pages, &gfx_blocks)?;
        let gfx_count = pages::Pages::from_flags(gfx.flags).count();
        let pages_info = self.out.u64(Resource::ROOT + 0x08).unwrap_or(0);
        let mut sys_blocks: Vec<usize> = (0..self.blocks.len()).filter(|&b| !self.blocks[b].graphics).collect();
        let (sys, guess) = self.system_pages(system_flags, gfx_count, sys_pages, &sys_blocks, pages_info)?;
        let sys_count = pages::Pages::from_flags(sys.flags).count();
        if pages_info != 0 {
            let mut info = self.out.bytes(pages_info, 16)?.to_vec();
            info[8] = sys_count as u8;
            info[9] = gfx_count as u8;
            info.resize(16 + 8 * guess as usize, 0);
            self.blocks.push(NewBlock { graphics: false, data: info, align: 16, texture: None });
            sys_blocks.push(self.blocks.len() - 1);
            self.fixups.push((At::Address(Resource::ROOT + 0x08), At::Block(self.blocks.len() - 1, 0)));
        }
        let address = block_addresses(self.blocks.len(), (&gfx_blocks, &gfx.offsets), (&sys_blocks, &sys.offsets));
        self.out.system.resize(sys.len as usize, 0);
        self.out.graphics.resize(gfx.len as usize, 0);
        self.write_blocks(&address)?;
        let file = rage_resource::builder::repack(&self.out, sys.flags, gfx.flags)?;
        Ok(EditResult { file, in_place: false, replaced: self.replaced, added: self.added, removed: self.removed })
    }
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
    let parsed = parse_dictionaries(res, dicts, enhanced)?;
    check_edits(edits, enhanced)?;
    let addresses = texture_addresses(res, dicts, &parsed)?;
    let mut editor = Editor {
        dicts,
        enhanced,
        layout,
        parsed,
        addresses,
        out: res.clone(),
        blocks: Vec::new(),
        fixups: Vec::new(),
        lists: vec![None; dicts.len()],
        replaced: 0,
        added: 0,
        removed: 0,
    };
    for edit in edits {
        editor.apply_edit(edit)?;
    }
    editor.write_lists()?;
    if editor.blocks.is_empty() {
        let file = rage_resource::builder::repack(&editor.out, system_flags, graphics_flags)?;
        let Editor { replaced, added, removed, .. } = editor;
        return Ok(EditResult { file, in_place: true, replaced, added, removed });
    }
    editor.finish(system_flags, graphics_flags)
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
