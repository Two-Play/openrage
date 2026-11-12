//! Red Dead Redemption 2 metapeds: people and animals put together from parts.
//!
//! - `packs_0.rpf/packs/base/data/metapeds.ymt` lists every ped with its base
//!   skeleton (`META_Base_Male` → `meta_base_male.yft`).
//! - Each ped has an outfit file `<ped>.ymt` (in `…/data/metapeds/metapeds.rpf`;
//!   town variants in `metapeds_<town>.rpf`). An outfit is a list of assets:
//!   a model (`<drawable>.ydd`), texture dictionaries for colour, normals and
//!   material (`<albedo>.ytd`, …), a tint palette (a texture in
//!   `textures/graphics.ytd`) with three palette rows, plus face shape values.
//! - Outfits name their parts by hashes. Models are found in the drawable
//!   dictionaries of the ped archives (`packs_*.rpf`, updates, DLC packs) by
//!   the [`AssetIndex`]; colour and normal maps are texture names, found
//!   through the texture index. Materials name entries of
//!   `…/data/metapeds/assets_material.ymt`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rage_core::hash::joaat;
use rage_rpf::Rpf8;
use rage_rpf::rpf8::{Rdr2Keys, extension_id};

use crate::rdr2::Location;

/// The list of all peds.
pub const PED_LIST: &str = "packs_0.rpf/packs/base/data/metapeds.ymt";
/// Where the tint palettes are (`metaped_tint_*` textures).
pub const PALETTES: &str = "textures_0.rpf/textures/graphics.ytd";

/// A ped of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ped {
    pub name: String,
    /// Name of the base skeleton fragment, e.g. `META_Base_Male`.
    pub base: String,
}

/// One part of an outfit. Names are hashes; zero means none. `drawable` is
/// a model in a drawable dictionary, `albedo` and `normal` are texture names,
/// `material` an entry of `assets_material.ymt`, `palette` a texture of [`PALETTES`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asset {
    pub drawable: u32,
    pub albedo: u32,
    pub normal: u32,
    pub material: u32,
    pub palette: u32,
    /// Palette rows for the three tint channels.
    pub tints: [u8; 3],
}

/// One outfit: its parts and face shape values (`(id, value)`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outfit {
    pub assets: Vec<Asset>,
    pub expressions: Vec<(u32, f32)>,
}

/// The hash of a tag or value as written in metadata XML (`hash_XXXXXXXX` or a name).
fn hash_of(text: &str) -> u32 {
    match text.strip_prefix("hash_").and_then(|h| u32::from_str_radix(h, 16).ok()) {
        Some(h) => h,
        None if text.is_empty() => 0,
        None => joaat(text),
    }
}

fn tag_is(node: roxmltree::Node, name: &str) -> bool {
    node.is_element() && hash_of(node.tag_name().name()) == joaat(name)
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children().find(|c| tag_is(*c, name))
}

fn items<'a, 'i>(node: roxmltree::Node<'a, 'i>) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    node.children().filter(|c| c.is_element() && c.tag_name().name() == "Item")
}

fn text_hash(node: Option<roxmltree::Node>) -> u32 {
    node.and_then(|n| n.text()).map_or(0, |t| hash_of(t.trim()))
}

fn attr_value<T: std::str::FromStr>(node: Option<roxmltree::Node>) -> Option<T> {
    node?.attribute("value")?.parse().ok()
}

fn document(xml: &str) -> Result<roxmltree::Document<'_>> {
    Ok(roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: u32::MAX, ..Default::default() },
    )?)
}

/// The peds of `metapeds.ymt` (as XML).
pub fn parse_ped_list(xml: &str) -> Result<Vec<Ped>> {
    let doc = document(xml)?;
    let list = doc
        .root_element()
        .children()
        .find(|c| tag_is(*c, "InitDatas"))
        .context("no ped list (InitDatas) in this file")?;
    // The base skeleton field has no known name.
    const BASE: u32 = 0x9312_4EBD;
    Ok(items(list)
        .filter_map(|item| {
            let name = child(item, "Name")?.text()?.trim().to_owned();
            let base = item
                .children()
                .find(|c| c.is_element() && hash_of(c.tag_name().name()) == BASE)
                .and_then(|c| c.text())
                .map(|t| t.trim().to_owned())
                .unwrap_or_default();
            Some(Ped { name, base })
        })
        .collect())
}

/// The outfits of a ped's outfit file (as XML).
pub fn parse_outfits(xml: &str) -> Result<Vec<Outfit>> {
    let doc = document(xml)?;
    let Some(list) = child(doc.root_element(), "outfits") else {
        bail!("no outfits in this file");
    };
    // The list of a ped's parts has no known name: it is the array whose
    // items have a `drawable`.
    fn parts<'a, 'i>(outfit: roxmltree::Node<'a, 'i>) -> Option<roxmltree::Node<'a, 'i>> {
        outfit.children().find(|c| c.is_element() && items(*c).any(|i| child(i, "drawable").is_some()))
    }
    Ok(items(list)
        .map(|outfit| {
            let assets = parts(outfit)
                .map(|p| {
                    items(p)
                        .map(|a| Asset {
                            drawable: text_hash(child(a, "drawable")),
                            albedo: text_hash(child(a, "albedo")),
                            normal: text_hash(child(a, "normal")),
                            material: text_hash(child(a, "material")),
                            palette: text_hash(child(a, "palette")),
                            tints: [
                                attr_value(child(a, "tint0")).unwrap_or(0),
                                attr_value(child(a, "tint1")).unwrap_or(0),
                                attr_value(child(a, "tint2")).unwrap_or(0),
                            ],
                        })
                        .collect()
                })
                .unwrap_or_default();
            let expressions = child(outfit, "expressions")
                .map(|e| {
                    items(e)
                        .filter_map(|i| Some((attr_value(child(i, "id"))?, attr_value(child(i, "value"))?)))
                        .collect()
                })
                .unwrap_or_default();
            Outfit { assets, expressions }
        })
        .collect())
}

const HEADER: &str = "openrage-rdr2-metapeds 1";

/// A model in a drawable dictionary (`.ydd`): the file and its place in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawableAt {
    pub file: Location,
    pub index: u16,
}

/// Where the peds' outfit files and models are. Outfits name their models by
/// a hash that is not a file name: the models sit in drawable dictionaries
/// (often several in one `…_merge_2.ydd`) under that hash. Textures are found
/// through the texture index ([`crate::rdr2::TextureIndex`]).
///
/// Building it reads every drawable dictionary of the ped archives (half a
/// minute), so it is cached like the texture index.
#[derive(Default, Debug, PartialEq)]
pub struct AssetIndex {
    /// Outfit files by the hash of the ped name; the one the game uses first.
    outfits: HashMap<u32, Vec<Location>>,
    /// Models by hash; the one the game uses first.
    drawables: HashMap<u32, Vec<DrawableAt>>,
}

impl AssetIndex {
    /// Where the index for a game folder is cached.
    pub fn default_path(game: &Path) -> Option<PathBuf> {
        let canonical = game.canonicalize().unwrap_or_else(|_| game.to_path_buf());
        let id = rage_core::hash::jenkins_bytes(canonical.to_string_lossy().as_bytes());
        Some(dirs::cache_dir()?.join("openrage").join(format!("rdr2-metapeds-{id:08x}.tsv")))
    }

    /// Reads the archives that hold peds (update, DLC and `packs_*.rpf`):
    /// the entry tables for outfit files, the drawable dictionaries for models.
    pub fn build(game: &Path, keys: &Arc<Rdr2Keys>, progress: impl Fn(usize, usize) + Sync) -> AssetIndex {
        use rayon::prelude::*;
        use std::sync::atomic::{AtomicUsize, Ordering};
        /// A drawable dictionary: its archive, entry and location.
        type Dictionary = (Arc<Rpf8>, usize, Location);
        struct Found {
            /// Archive rank (lower wins), outfit files and drawable dictionaries.
            rank: usize,
            outfits: Vec<(u32, Location)>,
            dictionaries: Vec<Dictionary>,
        }
        fn walk(r: Rpf8, archive: &str, chain: &mut Vec<u32>, ids: (u8, u8), found: &mut Found) {
            let r = Arc::new(r);
            let Some(entries) = r.entries() else { return };
            for (i, e) in entries.iter().enumerate() {
                if e.ext_id == 0 {
                    if let Ok(child) = r.open_child(e) {
                        chain.push(e.hash);
                        walk(child, archive, chain, ids, found);
                        chain.pop();
                    }
                    continue;
                }
                if chain.is_empty() || (e.ext_id != ids.0 && e.ext_id != ids.1) {
                    continue;
                }
                let mut c = chain.clone();
                c.push(e.hash);
                let loc = Location { archive: archive.to_owned(), chain: c };
                if e.ext_id == ids.0 {
                    found.outfits.push((e.hash, loc));
                } else if e.is_resource {
                    found.dictionaries.push((r.clone(), i, loc));
                }
            }
        }
        let files = archives(game);
        let mut found: Vec<Found> = files
            .par_iter()
            .enumerate()
            .map(|(rank, p)| {
                let mut found = Found { rank, outfits: Vec::new(), dictionaries: Vec::new() };
                let rel = p.strip_prefix(game).unwrap_or(p).to_string_lossy().replace('\\', "/");
                if let Ok(r) = Rpf8::open_with_keys(p, Some(keys.clone())) {
                    let id = |ext| extension_id(ext, r.platform()).unwrap_or(0xFE);
                    let ids = (id("ymt"), id("ydd"));
                    walk(r, &rel, &mut Vec::new(), ids, &mut found);
                }
                found
            })
            .collect();
        found.sort_by_key(|f| f.rank);
        let jobs: Vec<(usize, &Dictionary)> =
            found.iter().flat_map(|f| f.dictionaries.iter().map(move |d| (f.rank, d))).collect();
        let done = AtomicUsize::new(0);
        let mut models: Vec<(usize, u32, DrawableAt)> = jobs
            .par_iter()
            .flat_map_iter(|(rank, (rpf, i, loc))| {
                progress(done.fetch_add(1, Ordering::Relaxed) + 1, jobs.len());
                let hashes = rpf
                    .entries()
                    .and_then(|e| e.get(*i))
                    .and_then(|e| rpf.read_resource(e).ok())
                    .map(|res| dictionary_hashes(&res))
                    .unwrap_or_default();
                hashes
                    .into_iter()
                    .enumerate()
                    .map(move |(k, h)| (*rank, h, DrawableAt { file: loc.clone(), index: k as u16 }))
            })
            .collect();
        models.sort_by_key(|(rank, _, _)| *rank);
        let mut index = AssetIndex::default();
        for f in &found {
            for (hash, loc) in &f.outfits {
                index.outfits.entry(*hash).or_default().push(loc.clone());
            }
        }
        for (_, hash, at) in models {
            index.drawables.entry(hash).or_default().push(at);
        }
        index
    }

    /// The outfit file of a ped (the one the game uses).
    pub fn outfit_file(&self, ped: &str) -> Option<&Location> {
        self.outfits.get(&joaat(&ped.to_ascii_lowercase()))?.first()
    }

    /// The model an outfit names (the one the game uses).
    pub fn drawable(&self, hash: u32) -> Option<&DrawableAt> {
        self.drawables.get(&hash)?.first()
    }

    /// Every copy of a model (base packs, updates and DLC packs).
    pub fn drawable_copies(&self, hash: u32) -> &[DrawableAt] {
        self.drawables.get(&hash).map_or(&[], Vec::as_slice)
    }

    /// Numbers of outfit files and models.
    pub fn counts(&self) -> (usize, usize) {
        (self.outfits.len(), self.drawables.len())
    }

    /// Text form: `L` lines for locations (`archive`, entry hashes), then
    /// `O\t<hash>\t<location>` and `M\t<hash>\t<location>\t<index>` lines.
    pub fn to_text(&self) -> String {
        let mut places: HashMap<Location, usize> = HashMap::new();
        let mut out = format!("{HEADER}\n");
        let mut place = |loc: &Location, out: &mut String| -> usize {
            if let Some(p) = places.get(loc) {
                return *p;
            }
            let chain: Vec<String> = loc.chain.iter().map(|h| format!("{h:08x}")).collect();
            out.push_str(&format!("L\t{}\t{}\n", loc.archive, chain.join("/")));
            let p = places.len();
            places.insert(loc.clone(), p);
            p
        };
        let mut lines = Vec::new();
        let mut outfits: Vec<_> = self.outfits.iter().collect();
        outfits.sort_by_key(|(h, _)| **h);
        for (h, locs) in outfits {
            for l in locs {
                let p = place(l, &mut out);
                lines.push(format!("O\t{h:08x}\t{p}"));
            }
        }
        let mut models: Vec<_> = self.drawables.iter().collect();
        models.sort_by_key(|(h, _)| **h);
        for (h, ats) in models {
            for a in ats {
                let p = place(&a.file, &mut out);
                lines.push(format!("M\t{h:08x}\t{p}\t{}", a.index));
            }
        }
        for l in lines {
            out.push_str(&l);
            out.push('\n');
        }
        out
    }

    pub fn from_text(text: &str) -> Option<AssetIndex> {
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let mut places = Vec::new();
        let mut index = AssetIndex::default();
        let hex = |t: &str| u32::from_str_radix(t, 16).ok();
        for l in lines {
            let f: Vec<&str> = l.split('\t').collect();
            match f.as_slice() {
                ["L", archive, chain] => places.push(Location {
                    archive: (*archive).to_owned(),
                    chain: chain.split('/').filter_map(hex).collect(),
                }),
                ["O", h, p] => {
                    let (Some(h), Some(loc)) = (hex(h), p.parse::<usize>().ok().and_then(|p| places.get(p))) else {
                        continue;
                    };
                    index.outfits.entry(h).or_default().push(loc.clone());
                }
                ["M", h, p, k] => {
                    let (Some(h), Some(loc), Ok(k)) =
                        (hex(h), p.parse::<usize>().ok().and_then(|p| places.get(p)), k.parse::<u16>())
                    else {
                        continue;
                    };
                    index.drawables.entry(h).or_default().push(DrawableAt { file: loc.clone(), index: k });
                }
                _ => {}
            }
        }
        Some(index)
    }

    pub fn load(path: &Path) -> std::io::Result<Option<AssetIndex>> {
        match std::fs::read_to_string(path) {
            Ok(t) => Ok(AssetIndex::from_text(&t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_text())
    }
}

/// The hashes of the models in a drawable dictionary, without reading the models.
fn dictionary_hashes(res: &rage_resource::Resource) -> Vec<u32> {
    let root = rage_resource::Resource::ROOT;
    let (Ok(hashes), Ok(count)) = (res.u64(root + 0x20), res.u16(root + 0x38)) else { return Vec::new() };
    if hashes == 0 {
        return Vec::new();
    }
    (0..count.min(4096) as u64).map_while(|k| res.u32(hashes + k * 4).ok()).collect()
}

/// The archives that hold ped files, the ones the game prefers first: patch
/// packs (newest first), updates (newest first), DLC packs, then `packs_*.rpf`.
fn archives(game: &Path) -> Vec<PathBuf> {
    let listed = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir).into_iter().flatten().flatten().map(|d| d.path()).collect()
    };
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let dlc_name = |p: &Path| p.parent().map(name).unwrap_or_default();
    let mut dlc: Vec<PathBuf> =
        listed(&game.join("x64/dlcpacks")).into_iter().map(|d| d.join("dlc.rpf")).filter(|p| p.is_file()).collect();
    dlc.sort_by_key(|p| std::cmp::Reverse(dlc_name(p)));
    let (patches, content): (Vec<PathBuf>, Vec<PathBuf>) =
        dlc.into_iter().partition(|p| dlc_name(p).starts_with("patchpack"));
    let top = listed(game);
    let pick = |prefix: &str| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = top
            .iter()
            .filter(|p| {
                let n = name(p);
                n.starts_with(prefix) && n.ends_with(".rpf")
            })
            .cloned()
            .collect();
        v.sort_by_key(|p| std::cmp::Reverse(name(p)));
        v
    };
    let mut out = patches;
    out.extend(pick("update_"));
    out.extend(content);
    let mut packs = pick("packs_");
    packs.reverse();
    out.extend(packs);
    out
}

/// Opens the archive holding a location and finds the entry in it.
pub fn open_location(game: &Path, loc: &Location) -> Option<(Rpf8, usize)> {
    let mut rpf = Rpf8::open_default(&game.join(&loc.archive)).ok()?;
    let (last, nested) = loc.chain.split_last()?;
    for h in nested {
        let e = rpf.entries()?.iter().find(|e| e.hash == *h && e.ext_id == 0)?.clone();
        rpf = rpf.open_child(&e).ok()?;
    }
    let i = rpf.entries()?.iter().position(|e| e.hash == *last && e.ext_id != 0)?;
    Some((rpf, i))
}

/// Reads the ped list of a game folder.
pub fn ped_list(game: &Path) -> Result<Vec<Ped>> {
    let xml = crate::metadata::load(game, None, PED_LIST)?;
    parse_ped_list(&xml.text)
}

/// Reads a ped's outfits (from its outfit file in the base packs or a DLC pack).
pub fn load_outfits(game: &Path, index: &AssetIndex, ped: &str) -> Result<Vec<Outfit>> {
    let loc = index.outfit_file(ped).with_context(|| format!("no outfit file for {ped}"))?;
    let (rpf, i) = open_location(game, loc).with_context(|| format!("cannot open the outfit file of {ped}"))?;
    let xml = crate::metadata::from_rpf8(&rpf, i)?;
    parse_outfits(&xml.text)
}

/// The tint palettes (textures in [`PALETTES`]): hash and name.
pub fn palettes(game: &Path) -> Result<Vec<(u32, String)>> {
    let (node, _) = rage_mods::resolve(game, PALETTES, None)?;
    let rage_rpf::vfs::Node::Rpf8File(rpf, i) = node else { bail!("{PALETTES} is not a file") };
    let entry = rpf.entries().and_then(|e| e.get(i)).context("archive table not readable")?;
    let dict = rage_formats::ytd::parse_rdr2(&rpf.read_resource(entry)?)?;
    Ok(dict.textures.iter().map(|t| (joaat(&t.name.to_ascii_lowercase()), t.name.clone())).collect())
}

/// The ped index of a game folder: from the cache, or built (and cached) now.
/// `None` without keys.
pub fn index(game: &Path, progress: impl Fn(usize, usize) + Sync) -> Option<Arc<AssetIndex>> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<AssetIndex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(i) = cache.lock().ok()?.get(game) {
        return Some(i.clone());
    }
    let path = AssetIndex::default_path(game);
    let index = match path.as_deref().and_then(|p| AssetIndex::load(p).ok().flatten()) {
        Some(i) => i,
        None => {
            let keys = rage_rpf::rpf8::default_keys()?;
            let built = AssetIndex::build(game, &keys, progress);
            if let Some(p) = &path {
                let _ = built.save(p);
            }
            built
        }
    };
    let index = Arc::new(index);
    cache.lock().ok()?.insert(game.to_path_buf(), index.clone());
    Some(index)
}

/// The base skeleton fragment of a ped (`META_Base_Male` → `meta_base_male.yft`).
pub fn base_fragment(ped: &Ped) -> String {
    format!("{}.yft", ped.base.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ped_list() {
        let xml = r#"<CPedModelInfo__InitDataList><InitDatas itemType="CPedModelInfo__InitData">
            <Item><Name>CS_JohnMarston</Name><hash_93124EBD>META_Base_Male</hash_93124EBD></Item>
            <Item><Name>A_C_Cow</Name><hash_93124EBD>META_Base_Cow</hash_93124EBD></Item>
            </InitDatas></CPedModelInfo__InitDataList>"#;
        let peds = parse_ped_list(xml).unwrap();
        assert_eq!(peds.len(), 2);
        assert_eq!(peds[0], Ped { name: "CS_JohnMarston".into(), base: "META_Base_Male".into() });
        assert_eq!(base_fragment(&peds[1]), "meta_base_cow.yft");
    }

    #[test]
    fn index_round_trip() {
        let mut idx = AssetIndex::default();
        let a = Location { archive: "packs_1.rpf".into(), chain: vec![0x323A_6BF5, 0x5608_AB4C] };
        let b = Location { archive: "x64/dlcpacks/patchpack002/dlc.rpf".into(), chain: vec![1, 2, 3] };
        idx.outfits.insert(joaat("cs_johnmarston"), vec![b.clone(), a.clone()]);
        idx.drawables.insert(0x0933_3A20, vec![DrawableAt { file: a.clone(), index: 0 }]);
        idx.drawables.insert(0x850B_5085, vec![DrawableAt { file: a.clone(), index: 1 }]);
        let back = AssetIndex::from_text(&idx.to_text()).unwrap();
        assert_eq!(back, idx);
        assert_eq!(back.outfit_file("CS_JohnMarston"), Some(&b));
        assert_eq!(back.drawable(0x850B_5085).map(|d| d.index), Some(1));
        assert_eq!(idx.to_text().matches("\nL\t").count(), 2, "each location once");
    }

    #[test]
    fn reads_outfits() {
        // As OpenRAGE writes it: known field names, other fields as hashes.
        let xml = r#"<hash_04BD672F><outfits itemType="hash_5E641A6B">
            <Item>
              <hash_A9973338 itemType="hash_E5168B7B" />
              <hash_685A958D itemType="hash_7CF8A992">
                <Item>
                  <drawable>pant_mf1_061_alt</drawable><albedo>hash_9240183C</albedo>
                  <normal>hash_D21E77C3</normal><material>hash_FE391425</material>
                  <palette /><tint0 value="0" /><tint1 value="0" /><tint2 value="0" />
                </Item>
                <Item>
                  <drawable>hash_09333A20</drawable><albedo>hair_gen_999_c0_999_ab</albedo>
                  <normal>hair_gen_999_c0_999_nm</normal><material>hash_51536762</material>
                  <palette>metaped_tint_hair</palette><tint0 value="9" /><tint1 value="17" /><tint2 value="17" />
                </Item>
              </hash_685A958D>
              <expressions itemType="hash_AD065DDE"><Item><id value="10726" /><value value="-0.9" /></Item></expressions>
            </Item>
            <Item><hash_685A958D itemType="hash_7CF8A992" /></Item>
            </outfits></hash_04BD672F>"#;
        let outfits = parse_outfits(xml).unwrap();
        assert_eq!(outfits.len(), 2);
        let a = &outfits[0].assets;
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].drawable, joaat("pant_mf1_061_alt"));
        assert_eq!(a[0].albedo, 0x9240_183C);
        assert_eq!(a[0].palette, 0);
        assert_eq!(a[1].palette, 0xDFB1_F64C, "metaped_tint_hair");
        assert_eq!(a[1].tints, [9, 17, 17]);
        assert_eq!(outfits[0].expressions, vec![(10726, -0.9)]);
        assert!(outfits[1].assets.is_empty());
        // The same file with the field names still hashed.
        let hashed = xml
            .replace("outfits", "hash_124563FF")
            .replace("<drawable>", "<hash_C720CAAA>")
            .replace("</drawable>", "</hash_C720CAAA>")
            .replace("tint0", "hash_48EA2F7E");
        let back = parse_outfits(&hashed).unwrap();
        assert_eq!(back[0].assets[1].drawable, 0x0933_3A20);
        assert_eq!(back[0].assets[1].tints[0], 9);
    }
}
