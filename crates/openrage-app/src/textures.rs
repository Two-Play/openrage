//! Replacing, adding and removing textures of a dictionary inside the game.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rage_crypto::GtaVKeys;
use rage_formats::ytd_write::{self, Edit, EditResult};
use rage_rpf::EntryKind;
use rage_rpf::vfs::Node;

use crate::install::{Op, apply_at};

/// Applies texture edits to the `.ytd` at `vpath` (as the game sees it, i.e.
/// including mods) and writes the result into the mods folder.
pub fn edit_textures(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    vpath: &str,
    edits: &[Edit],
) -> Result<(EditResult, PathBuf)> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys.clone())?;
    let Node::ArchiveFile(rpf, id) = node else { bail!("{vpath} is not a file inside an archive") };
    let EntryKind::Resource { system_flags, graphics_flags, .. } = rpf.entry(id).kind else {
        bail!("{vpath} is not a resource")
    };
    let res = rpf.read_resource(id)?;
    let dict = rage_formats::ytd::parse(&res).context("reading the texture dictionary")?;
    let result = ytd_write::apply(&res, system_flags, graphics_flags, &dict, edits)?;
    drop(rpf);
    let name = vpath.rsplit('/').next().unwrap_or(vpath);
    let what = match edits {
        [ytd_write::Edit::Set(t)] => format!("Replaced texture {} in {name}", t.name),
        [ytd_write::Edit::Remove(n)] => format!("Removed texture {n} from {name}"),
        _ => format!("Changed {} textures in {name}", edits.len()),
    };
    let applied = apply_at(game_dir, keys, vpath, Op::Set(result.file.clone()), &what)?;
    Ok((result, applied.path))
}

/// Reads a DDS file as a texture to put into a dictionary under `name`.
pub fn texture_from_dds(name: &str, path: &Path) -> Result<rage_formats::texture::Texture> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    rage_formats::dds::from_dds(name, &bytes).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}
