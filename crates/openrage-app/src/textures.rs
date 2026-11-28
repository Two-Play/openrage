//! Replacing, adding and removing textures of a dictionary inside the game.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rage_crypto::GtaVKeys;
use rage_formats::ytd_write::{self, Edit, EditResult};
use rage_rpf::EntryKind;
use rage_rpf::vfs::Node;

use crate::install::{Op, apply_at};

/// Applies texture edits to the `.ytd` (GTA IV: `.wtd`) at `vpath` (as the
/// game sees it, i.e. including mods) and writes the result into the mods
/// folder (GTA IV: the update folder or, when chosen, the game files).
pub fn edit_textures(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    vpath: &str,
    edits: &[Edit],
) -> Result<(EditResult, PathBuf)> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys.clone())?;
    let result = edit_node(node, vpath, edits)?;
    let applied = apply_at(game_dir, keys, vpath, Op::Set(result.file.clone()), &describe(vpath, edits))?;
    Ok((result, applied.path))
}

fn edit_node(node: Node, vpath: &str, edits: &[Edit]) -> Result<EditResult> {
    match node {
        Node::Rpf8File(rpf, i) => edit_rdr2_packed(rpf, i, vpath, edits),
        Node::PlainFile(path) => {
            edit_rdr2_replaced(&path, edits).and_then(|r| r.with_context(|| not_in_archive(vpath)))
        }
        Node::Gta4File(archive, id) => edit_gta4(archive, id, vpath, edits),
        Node::ArchiveFile(rpf, id) => edit_gta5(rpf, id, vpath, edits),
        _ => bail!(not_in_archive(vpath)),
    }
}

fn not_in_archive(vpath: &str) -> String {
    format!("{vpath} is not a file inside an archive")
}

/// The result of rewriting an RDR2 dictionary: `file` is its RSC8 header
/// followed by the changed pages.
fn rdr2_result(mut file: Vec<u8>, changed: &rage_resource::Resource, replaced: usize) -> EditResult {
    file.truncate(16);
    file.extend(&changed.system);
    file.extend(&changed.graphics);
    EditResult { file, in_place: true, replaced, added: 0, removed: 0 }
}

fn rdr2_rewrite(res: &rage_resource::Resource, edits: &[Edit]) -> Result<rage_resource::Resource> {
    let dict = rage_formats::ytd::parse_rdr2(res).context("reading the texture dictionary")?;
    Ok(ytd_write::apply_rdr2(res, &dict, edits)?)
}

/// RDR2: same-shape replacements, written into the padded layout.
fn edit_rdr2_packed(rpf: Arc<rage_rpf::Rpf8>, i: usize, vpath: &str, edits: &[Edit]) -> Result<EditResult> {
    let entry = rpf.entries().and_then(|e| e.get(i)).with_context(|| format!("{vpath} not found"))?.clone();
    let res = rpf.read_resource(&entry)?;
    let changed = rdr2_rewrite(&res, edits)?;
    let file = rpf.resource_file(&entry)?;
    Ok(rdr2_result(file, &changed, edits.len()))
}

/// RDR2: a dictionary already replaced in lml/replace (an RSC8 file); `None`
/// when `path` is not one.
fn edit_rdr2_replaced(path: &Path, edits: &[Edit]) -> Result<Option<EditResult>> {
    let Some(res) = std::fs::read(path).ok().as_deref().and_then(rage_rpf::rpf8::parse_resource_file) else {
        return Ok(None);
    };
    let changed = rdr2_rewrite(&res, edits)?;
    let file = std::fs::read(path)?;
    Ok(Some(rdr2_result(file, &changed, edits.len())))
}

/// GTA IV (.wtd): an RSC5 resource inside an image archive.
fn edit_gta4(
    archive: Arc<rage_rpf::Gta4Archive>,
    id: rage_rpf::Gta4EntryId,
    vpath: &str,
    edits: &[Edit],
) -> Result<EditResult> {
    let raw = archive.read(id)?;
    let (kind, flags) = rage_resource::rsc5_header(&raw).with_context(|| format!("{vpath} is not a resource"))?;
    let res = rage_resource::Resource::from_rsc5(&raw)?;
    if !is_gta4_model(vpath) {
        let dict = rage_formats::ytd::parse_gta4(&res).context("reading the texture dictionary")?;
        return Ok(rage_formats::wtd_write::apply(&res, kind, flags, &dict, edits)?);
    }
    // A model (.wdr) with its own textures.
    let embedded = rage_formats::wdr::embedded_textures(&res, rage_resource::Resource::ROOT)
        .with_context(|| format!("{vpath} has no textures of its own"))?;
    let dict = rage_formats::ytd::parse_gta4_at(&res, embedded.root).context("reading the model's textures")?;
    Ok(rage_formats::wtd_write::apply_embedded(&res, kind, flags, embedded.root, &embedded.in_use, &dict, edits)?)
}

fn edit_gta5(rpf: Arc<rage_rpf::Rpf7>, id: rage_rpf::EntryId, vpath: &str, edits: &[Edit]) -> Result<EditResult> {
    let EntryKind::Resource { system_flags, graphics_flags, .. } = rpf.entry(id).kind else {
        bail!("{vpath} is not a resource")
    };
    let res = rpf.read_resource(id)?;
    let Some(ext) = gta5_model_ext(vpath) else {
        let dict = rage_formats::ytd::parse(&res).context("reading the texture dictionary")?;
        return Ok(ytd_write::apply(&res, system_flags, graphics_flags, &dict, edits)?);
    };
    // A model (.ydr, .yft, .ydd) with textures of its own.
    let (enhanced, dicts) = rage_formats::drawable::embedded_texture_dicts(&res, ext)?;
    if dicts.is_empty() {
        bail!("{vpath} has no textures of its own");
    }
    Ok(rage_formats::model_textures::apply(&res, system_flags, graphics_flags, enhanced, &dicts, edits)?)
}

/// The textures of a GTA VI dictionary (`.#td`).
pub fn gta6_dictionary(rpf: &rage_rpf::Rpf9, index: usize) -> Result<rage_formats::ytd::TextureDictionary> {
    let (system, graphics) = crate::rpf9::resource(rpf, index)?;
    Ok(rage_formats::td6::parse(&system, &graphics)?)
}

/// The extension of a GTA V model (`ydr`, `yft`, `ydd`) named by `vpath`.
pub fn gta5_model_ext(vpath: &str) -> Option<&str> {
    let ext = vpath.rsplit('.').next()?;
    ["ydr", "yft", "ydd"].into_iter().find(|e| ext.eq_ignore_ascii_case(e))
}

/// Whether `vpath` names a model that can carry textures of its own
/// (GTA IV `.wdr`; GTA V `.ydr`, `.yft`, `.ydd`).
pub fn is_model(vpath: &str) -> bool {
    is_gta4_model(vpath) || gta5_model_ext(vpath).is_some()
}

/// The textures a GTA V model carries, all its texture lists together (each
/// name once).
pub fn gta5_model_textures(res: &rage_resource::Resource, vpath: &str) -> Option<rage_formats::ytd::TextureDictionary> {
    let (enhanced, dicts) = rage_formats::drawable::embedded_texture_dicts(res, gta5_model_ext(vpath)?).ok()?;
    let mut merged: Option<rage_formats::ytd::TextureDictionary> = None;
    for d in dicts {
        let Ok(dict) = rage_formats::ytd::parse_at(res, d, enhanced) else { continue };
        match &mut merged {
            None => merged = Some(dict),
            Some(m) => {
                for (t, s) in dict.textures.into_iter().zip(dict.stored) {
                    if !m.textures.iter().any(|x| x.name.eq_ignore_ascii_case(&t.name)) {
                        m.textures.push(t);
                        m.stored.push(s);
                    }
                }
                m.errors.extend(dict.errors);
            }
        }
    }
    merged
}

/// Whether `vpath` names a GTA IV drawable (`.wdr`), whose textures live inside it.
pub fn is_gta4_model(vpath: &str) -> bool {
    vpath.to_ascii_lowercase().ends_with(".wdr")
}

/// The textures embedded in a GTA IV drawable (`.wdr`), if it has any.
pub fn gta4_model_textures(res: &rage_resource::Resource) -> Option<rage_formats::ytd::TextureDictionary> {
    let embedded = rage_formats::wdr::embedded_textures(res, rage_resource::Resource::ROOT)?;
    rage_formats::ytd::parse_gta4_at(res, embedded.root).ok()
}

/// Journal text for a set of texture edits.
fn describe(vpath: &str, edits: &[Edit]) -> String {
    let name = vpath.rsplit('/').next().unwrap_or(vpath);
    match edits {
        [Edit::Set(t)] => format!("Put texture {} into {name}", t.name),
        [Edit::Remove(n)] => format!("Removed texture {n} from {name}"),
        _ => format!("Changed {} textures in {name}", edits.len()),
    }
}

/// Reads a DDS file as a texture to put into a dictionary under `name`.
pub fn texture_from_dds(name: &str, path: &Path) -> Result<rage_formats::texture::Texture> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    rage_formats::dds::from_dds(name, &bytes).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

/// Whether a model edit failed only because the model's pages have no room.
fn no_room(e: &rage_resource::ResourceError) -> bool {
    let msg = e.to_string();
    msg.contains("does not fit") || msg.contains("no room") || msg.contains("pages")
}

/// What the in-memory check of a GTA V model's textures found
/// (`openrage verify --textures --rebuild`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCheck {
    /// The model has no textures of its own.
    NoTextures,
    /// A texture of the model cannot be read, so it cannot be changed either.
    Unreadable(String),
    /// Every edit read back as expected; `grown` tells whether a texture of
    /// twice the size fitted into appended pages.
    Ok { grown: bool },
}

/// A comparable summary of a model's drawables: per drawable its meshes
/// (vertex and index counts, shader), shaders and bone count.
fn model_signature(res: &rage_resource::Resource, ext: &str) -> Result<Vec<String>> {
    use rage_formats::drawable::{self, Drawable};
    let drawables: Vec<Drawable> = match ext {
        "ydr" => vec![drawable::parse_ydr(res)?],
        "yft" => {
            let f = drawable::parse_fragment(res)?;
            std::iter::once(f.drawable).chain(f.children.into_iter().map(|c| c.drawable)).collect()
        }
        _ => drawable::parse_ydd(res)?.into_iter().map(|(_, d)| d).collect(),
    };
    Ok(drawables
        .iter()
        .map(|d| {
            let meshes: Vec<String> =
                d.meshes.iter().map(|m| format!("{}/{}/{}", m.positions.len(), m.indices.len(), m.shader)).collect();
            let shaders: Vec<String> =
                d.shaders.iter().map(|s| format!("{:08x}:{}", s.name_hash, s.textures.join(","))).collect();
            let bones = d.skeleton.as_ref().map_or(0, |s| s.bones.len());
            format!("{} | {} | {bones}", meshes.join(" "), shaders.join(" "))
        })
        .collect())
}

type TextureKey = (String, u32, u32, u32, u32, Vec<u8>);

fn texture_key(t: &rage_formats::texture::Texture) -> TextureKey {
    (t.name.to_ascii_lowercase(), t.width, t.height, t.levels, t.format.0, t.data.clone())
}

/// Whether the textures of `dict` are, in any order, exactly `expect`.
fn textures_match(mut expect: Vec<TextureKey>, dict: &rage_formats::ytd::TextureDictionary) -> bool {
    let mut got: Vec<_> = dict.textures.iter().map(texture_key).collect();
    expect.sort();
    got.sort();
    expect == got
}

/// The model under check, with what is needed to write and read it back.
struct ModelCheckCtx<'a> {
    res: &'a rage_resource::Resource,
    vpath: &'a str,
    ext: String,
    system_flags: u32,
    graphics_flags: u32,
    enhanced: bool,
    dicts: Vec<u64>,
    signature: Vec<String>,
}

impl<'a> ModelCheckCtx<'a> {
    /// The check of `vpath` and its textures as they are; `Err(verdict)` when there is nothing more to check.
    fn open(
        res: &'a rage_resource::Resource,
        vpath: &'a str,
        flags: (u32, u32),
    ) -> Result<std::result::Result<(Self, rage_formats::ytd::TextureDictionary), ModelCheck>> {
        let Some(ext) = gta5_model_ext(vpath).map(str::to_ascii_lowercase) else {
            return Ok(Err(ModelCheck::NoTextures));
        };
        let (enhanced, dicts) = rage_formats::drawable::embedded_texture_dicts(res, &ext)?;
        let Some(before) = gta5_model_textures(res, vpath).filter(|d| !dicts.is_empty() && !d.textures.is_empty())
        else {
            return Ok(Err(ModelCheck::NoTextures));
        };
        if let Some(e) = before.errors.first() {
            return Ok(Err(ModelCheck::Unreadable(e.clone())));
        }
        let signature = model_signature(res, &ext)?;
        let (system_flags, graphics_flags) = flags;
        Ok(Ok((Self { res, vpath, ext, system_flags, graphics_flags, enhanced, dicts, signature }, before)))
    }

    fn apply(&self, edits: &[Edit]) -> Result<EditResult, rage_resource::ResourceError> {
        rage_formats::model_textures::apply(
            self.res,
            self.system_flags,
            self.graphics_flags,
            self.enhanced,
            &self.dicts,
            edits,
        )
    }

    fn read_back(&self, file: &[u8]) -> Result<rage_formats::ytd::TextureDictionary> {
        let back = rage_resource::Resource::from_rsc7(file)?;
        let dict = gta5_model_textures(&back, self.vpath).context("textures gone")?;
        if model_signature(&back, &self.ext)? != self.signature {
            bail!("meshes, shaders or bones changed");
        }
        Ok(dict)
    }

    /// A texture written over with itself: nothing changes.
    fn check_same(&self, first: &rage_formats::texture::Texture) -> Result<()> {
        let same = self.apply(&[Edit::Set(first.clone())])?;
        let back = rage_resource::Resource::from_rsc7(&same.file)?;
        if !same.in_place || back.system != self.res.system || back.graphics != self.res.graphics {
            bail!("writing a texture over itself changed the resource");
        }
        Ok(())
    }

    /// A texture of twice the size, into appended pages if they allow it;
    /// whether they did.
    fn check_grown(&self, before: &rage_formats::ytd::TextureDictionary) -> Result<bool> {
        use rage_formats::texture::Texture;
        let first = &before.textures[0];
        let mut big = Texture { width: first.width * 2, height: first.height * 2, ..first.clone() };
        let size = big.expected_size().context("unsupported format")?;
        big.data = (0..size).map(|i| (i * 7 % 251) as u8).collect();
        match self.apply(&[Edit::Set(big.clone())]) {
            Ok(r) => {
                let dict = self.read_back(&r.file)?;
                let mut expect: Vec<_> = before.textures.iter().map(texture_key).collect();
                expect[0] = texture_key(&Texture { name: first.name.clone(), ..big });
                if !textures_match(expect, &dict) {
                    bail!("the grown texture or another texture reads back wrong");
                }
                Ok(true)
            }
            Err(e) if no_room(&e) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// A new texture (models with one texture list).
    fn check_added(&self, before: &rage_formats::ytd::TextureDictionary) -> Result<()> {
        use rage_formats::texture::Texture;
        let first = &before.textures[0];
        let mut extra = Texture { name: "openrage_check".into(), width: 8, height: 8, levels: 1, ..first.clone() };
        extra.data = vec![0x5A; extra.expected_size().context("unsupported format")?];
        let added = match self.apply(&[Edit::Set(extra.clone())]) {
            Ok(r) => r,
            Err(e) if no_room(&e) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let dict = self.read_back(&added.file)?;
        let mut expect: Vec<_> = before.textures.iter().map(texture_key).collect();
        expect.push(texture_key(&extra));
        if !textures_match(expect, &dict) {
            bail!("an added texture or another texture reads back wrong");
        }
        Ok(())
    }
}

/// Checks the model texture writer on one GTA V model, in memory: a texture
/// written over with itself must leave the resource byte for byte; a texture
/// twice the size (when the pages allow it) and, for models with one texture
/// list, an added texture must read back with every other texture, mesh,
/// shader and bone unchanged.
pub fn check_model_textures(
    res: &rage_resource::Resource,
    vpath: &str,
    system_flags: u32,
    graphics_flags: u32,
) -> Result<ModelCheck> {
    let (ctx, before) = match ModelCheckCtx::open(res, vpath, (system_flags, graphics_flags))? {
        Ok(opened) => opened,
        Err(verdict) => return Ok(verdict),
    };
    ctx.check_same(&before.textures[0])?;
    let grown = ctx.check_grown(&before)?;
    if ctx.dicts.len() == 1 {
        ctx.check_added(&before)?;
    }
    Ok(ModelCheck::Ok { grown })
}
