//! Loading models (`.ydr`, `.yft`, `.ydd`) with the textures they use.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rage_crypto::GtaVKeys;
use rage_formats::drawable::{self, Drawable};
use rage_formats::texture::Texture;
use rage_formats::ytd::TextureDictionary;
use rage_rpf::vfs::Node;

pub struct LoadedModel {
    /// Drawables with display names (one for `.ydr`/`.yft`, several for `.ydd`).
    pub drawables: Vec<(String, Drawable)>,
    /// Texture dictionaries found next to the model (`<name>.ytd`, `vehshare.ytd`).
    pub external: Vec<(String, TextureDictionary)>,
}

impl LoadedModel {
    /// Finds a texture by name: embedded in the drawable first, then next to the model.
    pub fn texture<'a>(&'a self, drawable: &'a Drawable, name: &str) -> Option<&'a Texture> {
        let find = |d: &'a TextureDictionary| d.textures.iter().find(|t| t.name.eq_ignore_ascii_case(name));
        drawable.textures.as_ref().and_then(find).or_else(|| self.external.iter().find_map(|(_, d)| find(d)))
    }
}

pub fn is_model(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".ydr") || n.ends_with(".yft") || n.ends_with(".ydd")
}

/// Loads a model as the game sees it (mods included).
pub fn load(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<LoadedModel> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys)?;
    let Node::ArchiveFile(rpf, id) = node else { bail!("{vpath} is not a file inside an archive") };
    let name = rpf.entry(id).name.clone();
    let res = rpf.read_resource(id)?;
    let lower = name.to_ascii_lowercase();
    let stem = name.split('.').next().unwrap_or(&name).to_owned();
    let drawables = if lower.ends_with(".ydd") {
        drawable::parse_ydd(&res)?
            .into_iter()
            .enumerate()
            .map(|(i, (hash, d))| (if hash != 0 { format!("{stem}#{hash:08x}") } else { format!("{stem}#{i}") }, d))
            .collect()
    } else if lower.ends_with(".yft") {
        vec![(stem.clone(), drawable::parse_yft(&res)?)]
    } else if lower.ends_with(".ydr") {
        vec![(stem.clone(), drawable::parse_ydr(&res)?)]
    } else {
        bail!("{name} is not a model (.ydr, .yft, .ydd)");
    };

    // Textures usually live next to the model: adder.yft → adder.ytd (+ vehshare.ytd for vehicles).
    let mut external = Vec::new();
    let dir = rpf.entry(id).parent.unwrap_or(rage_rpf::Rpf7::ROOT);
    let mut candidates = vec![format!("{stem}.ytd"), format!("{stem}+hi.ytd")];
    if lower.ends_with(".yft") {
        candidates.push("vehshare.ytd".into());
    }
    for c in candidates {
        if let Some(tid) = rpf.find_child(dir, &c)
            && let Ok(dict) = rpf
                .read_resource(tid)
                .map_err(anyhow::Error::from)
                .and_then(|r| rage_formats::ytd::parse(&r).context("texture dictionary"))
        {
            external.push((c, dict));
        }
    }
    Ok(LoadedModel { drawables, external })
}

/// Exports one drawable of a loaded model as `.glb`.
pub fn to_glb(model: &LoadedModel, index: usize) -> Option<Vec<u8>> {
    let (name, d) = model.drawables.get(index)?;
    Some(rage_formats::gltf::write_glb(d, name, &|t| model.texture(d, t)))
}
