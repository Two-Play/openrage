//! Vehicle mod parts (tuning: bonnets, bumpers, spoilers …): the `.yft`
//! files in a vehicle's `<vehicle>_mods.rpf` archives, and the bone the
//! game's `carcols` files put each of them on. A mod part's meshes are in the
//! space of that bone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rage_crypto::GtaVKeys;
use rage_rpf::{Rpf7, vfs};
use rayon::prelude::*;

/// A mod part of a vehicle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModPart {
    /// Its model name (`sultan_hood_1`).
    pub name: String,
    /// Its virtual path in the game.
    pub vpath: String,
    /// The bone it goes on (`bonnet`), from `carcols`.
    pub bone: Option<String>,
    /// What kind of part it is (`VMT_BONNET`), from `carcols`.
    pub kind: Option<String>,
}

/// What `carcols` says about a mod part: (bone, kind).
type Placement = (Option<String>, Option<String>);

/// What one archive holds: (vehicle, mod part path) and placements.
type Found = (Vec<(String, String)>, Vec<(u32, Placement)>);

#[derive(Default)]
struct Index {
    /// Vehicle name → its mod parts' virtual paths.
    parts: HashMap<String, Vec<String>>,
    /// Mod part name hash → where it goes (PSO files keep names as hashes).
    placements: HashMap<u32, Placement>,
}

fn indexes() -> &'static Mutex<HashMap<PathBuf, Arc<Index>>> {
    static INDEXES: OnceLock<Mutex<HashMap<PathBuf, Arc<Index>>>> = OnceLock::new();
    INDEXES.get_or_init(Default::default)
}

/// The mod parts of `vehicle` (its model name, `sultan`), sorted by name.
/// The first call reads every archive of the game once.
pub fn mod_parts(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vehicle: &str) -> Vec<ModPart> {
    let index = {
        let cached = indexes().lock().ok().and_then(|m| m.get(game_dir).cloned());
        match cached {
            Some(i) => i,
            None => {
                let built = Arc::new(build(game_dir, keys));
                if let Ok(mut m) = indexes().lock() {
                    m.insert(game_dir.to_path_buf(), built.clone());
                }
                built
            }
        }
    };
    let mut out: Vec<ModPart> = Vec::new();
    for vpath in index.parts.get(&vehicle.to_ascii_lowercase()).into_iter().flatten() {
        let file = vpath.rsplit('/').next().unwrap_or(vpath);
        let name = file.strip_suffix(".yft").unwrap_or(file).to_owned();
        // The same part in several packs: the last one found (updates come after the base game).
        out.retain(|p| p.name != name);
        let key = rage_formats::names::parse(&name.to_ascii_lowercase());
        let (bone, kind) = index.placements.get(&key).cloned().unwrap_or_default();
        out.push(ModPart { name, vpath: vpath.clone(), bone, kind });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A readable name for a kind of mod part: `VMT_BUMPER_F` → `bumper f`.
pub fn kind_label(kind: &str) -> String {
    kind.trim_start_matches("VMT_").to_ascii_lowercase().replace('_', " ")
}

fn build(game_dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Index {
    crate::progress::set("Finding the vehicle mod parts", 0, 0);
    let mut rpfs = Vec::new();
    crate::detect::collect_rpfs(game_dir, &mut rpfs);
    // Updates last, so their parts and placements win.
    rpfs.sort_by_key(|p| p.strip_prefix(game_dir).is_ok_and(|r| r.starts_with("update")));
    let found: Vec<Found> = rpfs
        .par_iter()
        .map(|p| {
            let rel = p.strip_prefix(game_dir).unwrap_or(p).to_string_lossy().replace('\\', "/");
            let (mut parts, mut placements) = (Vec::new(), Vec::new());
            let Ok(rpf) = Rpf7::open(p, keys.clone()) else { return (parts, placements) };
            vfs::walk(
                &rpf,
                Rpf7::ROOT,
                &rel,
                true,
                &mut |a, id, path| {
                    let name = a.entry(id).name.to_ascii_lowercase();
                    if name.ends_with(".yft") {
                        let lower = path.to_ascii_lowercase();
                        let archive = lower.rsplit('/').nth(1).unwrap_or("");
                        if let Some(vehicle) = archive.strip_suffix("_mods.rpf") {
                            parts.push((vehicle.to_owned(), path.to_owned()));
                        }
                    } else if name.starts_with("carcols")
                        && (name.ends_with(".meta") || name.ends_with(".ymt"))
                        && let Ok(xml) =
                            a.read(id).map_err(anyhow::Error::from).and_then(|d| crate::metadata::from_bytes(&d))
                    {
                        placements.extend(read_placements(&xml.text));
                    }
                },
                &mut |_| {},
            );
            (parts, placements)
        })
        .collect();
    let mut index = Index::default();
    for (parts, placements) in found {
        for (vehicle, vpath) in parts {
            index.parts.entry(vehicle).or_default().push(vpath);
        }
        index.placements.extend(placements);
    }
    crate::progress::clear();
    index
}

/// The mod parts a `carcols` file places (its `visibleMods`): model name
/// hash, bone (a name, or `hash_XXXXXXXX`) and kind.
fn read_placements(xml: &str) -> Vec<(u32, Placement)> {
    let tag = |s: &str, name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let start = s.find(&open)? + open.len();
        let end = s[start..].find(&format!("</{name}>"))? + start;
        let v = s[start..end].trim();
        (!v.is_empty()).then(|| v.to_owned())
    };
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<modelName>") {
        let item = &rest[at..];
        // This item ends where the next one's model name begins.
        let end = item[1..].find("<modelName>").map_or(item.len(), |e| e + 1);
        let item = &item[..end];
        if let Some(model) = tag(item, "modelName") {
            out.push((rage_formats::names::parse(&model.to_ascii_lowercase()), (tag(item, "bone"), tag(item, "type"))));
        }
        rest = &rest[at + end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_where_mod_parts_go() {
        let xml = "<visibleMods><Item><modelName>sultan_hood_1</modelName><linkedModels />\
            <turnOffBones><Item>bonnet</Item></turnOffBones><type>VMT_BONNET</type><bone>bonnet</bone>\
            <collisionBone>chassis</collisionBone></Item><Item><modelName>Sultan_Wing_1</modelName>\
            <type>VMT_SPOILER</type><bone>boot</bone></Item><Item><modelName>x</modelName></Item></visibleMods>";
        let p = read_placements(xml);
        let h = |n: &str| rage_core::hash::joaat(n);
        assert_eq!(p[0], (h("sultan_hood_1"), (Some("bonnet".into()), Some("VMT_BONNET".into()))));
        assert_eq!(p[1], (h("sultan_wing_1"), (Some("boot".into()), Some("VMT_SPOILER".into()))));
        assert_eq!(p[2], (h("x"), (None, None)));
        assert_eq!(read_placements(&format!("<modelName>hash_{:08X}</modelName>", h("a")))[0].0, h("a"));
        assert_eq!(kind_label("VMT_BUMPER_F"), "bumper f");
    }
}
