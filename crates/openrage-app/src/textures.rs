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
    if let Node::Gta4File(archive, id) = node {
        // GTA IV (.wtd): an RSC5 resource inside an image archive.
        let raw = archive.read(id)?;
        let (kind, flags) = rage_resource::rsc5_header(&raw).with_context(|| format!("{vpath} is not a resource"))?;
        let res = rage_resource::Resource::from_rsc5(&raw)?;
        let result = if is_gta4_model(vpath) {
            // A model (.wdr) with its own textures.
            let embedded = rage_formats::wdr::embedded_textures(&res, rage_resource::Resource::ROOT)
                .with_context(|| format!("{vpath} has no textures of its own"))?;
            let dict = rage_formats::ytd::parse_gta4_at(&res, embedded.root).context("reading the model's textures")?;
            rage_formats::wtd_write::apply_embedded(&res, kind, flags, embedded.root, &embedded.in_use, &dict, edits)?
        } else {
            let dict = rage_formats::ytd::parse_gta4(&res).context("reading the texture dictionary")?;
            rage_formats::wtd_write::apply(&res, kind, flags, &dict, edits)?
        };
        drop(archive);
        let applied = apply_at(game_dir, keys, vpath, Op::Set(result.file.clone()), &describe(vpath, edits))?;
        return Ok((result, applied.path));
    }
    let Node::ArchiveFile(rpf, id) = node else { bail!("{vpath} is not a file inside an archive") };
    let EntryKind::Resource { system_flags, graphics_flags, .. } = rpf.entry(id).kind else {
        bail!("{vpath} is not a resource")
    };
    let res = rpf.read_resource(id)?;
    let dict = rage_formats::ytd::parse(&res).context("reading the texture dictionary")?;
    let result = ytd_write::apply(&res, system_flags, graphics_flags, &dict, edits)?;
    drop(rpf);
    let applied = apply_at(game_dir, keys, vpath, Op::Set(result.file.clone()), &describe(vpath, edits))?;
    Ok((result, applied.path))
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
        [Edit::Set(t)] => format!("Replaced texture {} in {name}", t.name),
        [Edit::Remove(n)] => format!("Removed texture {n} from {name}"),
        _ => format!("Changed {} textures in {name}", edits.len()),
    }
}

/// Reads a DDS file as a texture to put into a dictionary under `name`.
pub fn texture_from_dds(name: &str, path: &Path) -> Result<rage_formats::texture::Texture> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    rage_formats::dds::from_dds(name, &bytes).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}
