//! Red Dead Redemption 2 helpers that need both archives and formats: the
//! texture index.
//!
//! RDR2 models mostly use textures from shared dictionaries elsewhere in the
//! game (`nbx_ground_paving14_ab` lives with other ground textures, not next
//! to the building). The index maps every texture name to the dictionary that
//! holds it. Locations are the archive file plus the chain of entry hashes
//! down to the dictionary, so they work for unnamed files too. It is built
//! once (a few minutes) and cached next to the file names.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rage_formats::ytd::TextureDictionary;
use rage_rpf::Rpf8;
use rage_rpf::rpf8::Rdr2Keys;

const HEADER: &str = "openrage-rdr2-textures 1";

/// Where a texture dictionary is: archive file (relative to the game folder)
/// and the hashes of the entries leading to it (nested archives, then the dictionary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub archive: String,
    pub chain: Vec<u32>,
}

/// Texture name (lower case) → dictionary.
#[derive(Default)]
pub struct TextureIndex {
    pub textures: HashMap<String, usize>,
    pub dictionaries: Vec<Location>,
}

impl TextureIndex {
    /// Where the index for a game folder is cached.
    pub fn default_path(game: &Path) -> Option<PathBuf> {
        let canonical = game.canonicalize().unwrap_or_else(|_| game.to_path_buf());
        let id = rage_core::hash::jenkins_bytes(canonical.to_string_lossy().as_bytes());
        Some(dirs::cache_dir()?.join("openrage").join(format!("rdr2-textures-{id:08x}.tsv")))
    }

    /// Text form: `D\t<archive>\t<hash>/<hash>…` lines for dictionaries, then
    /// `T\t<name>\t<dictionary index>` lines.
    pub fn to_text(&self) -> String {
        let mut out = format!("{HEADER}\n");
        for d in &self.dictionaries {
            let chain: Vec<String> = d.chain.iter().map(|h| format!("{h:08x}")).collect();
            out.push_str(&format!("D\t{}\t{}\n", d.archive, chain.join("/")));
        }
        let mut names: Vec<(&String, &usize)> = self.textures.iter().collect();
        names.sort();
        for (n, d) in names {
            out.push_str(&format!("T\t{n}\t{d}\n"));
        }
        out
    }

    pub fn from_text(text: &str) -> Option<TextureIndex> {
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let mut index = TextureIndex::default();
        for l in lines {
            let mut f = l.split('\t');
            match (f.next(), f.next(), f.next()) {
                (Some("D"), Some(archive), Some(chain)) => index.dictionaries.push(Location {
                    archive: archive.to_owned(),
                    chain: chain.split('/').filter_map(|h| u32::from_str_radix(h, 16).ok()).collect(),
                }),
                (Some("T"), Some(name), Some(d)) => {
                    if let Ok(d) = d.parse::<usize>() {
                        index.textures.insert(name.to_owned(), d);
                    }
                }
                _ => {}
            }
        }
        Some(index)
    }

    pub fn load(path: &Path) -> std::io::Result<Option<TextureIndex>> {
        match std::fs::read_to_string(path) {
            Ok(t) => Ok(TextureIndex::from_text(&t)),
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

    /// The dictionary that holds a texture.
    pub fn find(&self, texture: &str) -> Option<&Location> {
        self.dictionaries.get(*self.textures.get(&texture.to_ascii_lowercase())?)
    }
}

/// Names of the textures in a dictionary resource, without decoding them.
fn texture_names(res: &rage_resource::Resource) -> Vec<String> {
    let (Ok(list), Ok(count)) =
        (res.u64(rage_resource::Resource::ROOT + 0x30), res.u16(rage_resource::Resource::ROOT + 0x38))
    else {
        return Vec::new();
    };
    (0..count.min(4096) as u64)
        .filter_map(|i| {
            let tex = res.u64(list + i * 8).ok()?;
            res.string(res.u64(tex + 0x28).ok()?).ok()
        })
        .filter(|n| !n.is_empty())
        .collect()
}

/// Reads every texture dictionary in the game and records which holds which texture.
pub fn build_texture_index(
    game: &Path,
    files: &[PathBuf],
    keys: &Arc<Rdr2Keys>,
    progress: impl Fn(usize, usize) + Sync,
) -> TextureIndex {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    type Found = Vec<(Location, Vec<String>)>;
    fn walk(r: &Rpf8, archive: &str, chain: &mut Vec<u32>, out: &mut Found) {
        let Some(entries) = r.entries() else { return };
        for e in entries {
            if e.ext_id == 0 {
                if let Ok(child) = r.open_child(e) {
                    chain.push(e.hash);
                    walk(&child, archive, chain, out);
                    chain.pop();
                }
            } else if e.ext_id == 5
                && e.is_resource
                && let Ok(res) = r.read_resource(e)
            {
                let names = texture_names(&res);
                if !names.is_empty() {
                    let mut c = chain.clone();
                    c.push(e.hash);
                    out.push((Location { archive: archive.to_owned(), chain: c }, names));
                }
            }
        }
    }
    let done = AtomicUsize::new(0);
    let found: Found = files
        .par_iter()
        .map(|p| {
            let mut out = Found::new();
            let rel = p.strip_prefix(game).unwrap_or(p).to_string_lossy().replace('\\', "/");
            if let Ok(r) = Rpf8::open_with_keys(p, Some(keys.clone())) {
                walk(&r, &rel, &mut Vec::new(), &mut out);
            }
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, files.len());
            out
        })
        .flatten()
        .collect();
    let mut index = TextureIndex::default();
    for (loc, names) in found {
        let d = index.dictionaries.len();
        index.dictionaries.push(loc);
        for n in names {
            index.textures.entry(n.to_ascii_lowercase()).or_insert(d);
        }
    }
    index
}

/// Opens the dictionary at a location (keys and names as registered for the game).
pub fn open_dictionary(game: &Path, loc: &Location) -> Option<TextureDictionary> {
    let mut rpf = Rpf8::open_default(&game.join(&loc.archive)).ok()?;
    let (last, nested) = loc.chain.split_last()?;
    for h in nested {
        let e = rpf.entries()?.iter().find(|e| e.hash == *h && e.ext_id == 0)?.clone();
        rpf = rpf.open_child(&e).ok()?;
    }
    let e = rpf.entries()?.iter().find(|e| e.hash == *last && e.ext_id == 5)?.clone();
    rage_formats::ytd::parse_rdr2(&rpf.read_resource(&e).ok()?).ok()
}

/// The texture index of a game folder, loaded once (None until it was built).
pub fn texture_index(game: &Path) -> Option<Arc<TextureIndex>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<TextureIndex>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut map = cache.lock().ok()?;
    map.entry(game.to_path_buf())
        .or_insert_with(|| {
            TextureIndex::default_path(game).and_then(|p| TextureIndex::load(&p).ok().flatten()).map(Arc::new)
        })
        .clone()
}

/// Makes the file names found in the game known to metadata: archetype and
/// model names in `.ymap`/`.ytyp` are hashes of exactly these names.
pub fn register_names(index: &rage_rpf::rpf8_names::NameIndex) {
    let names: Vec<String> = index
        .names
        .keys()
        .filter(|h| index.trusted(**h))
        .filter_map(|h| index.names.get(h))
        .map(|n| {
            let base = n.rsplit('/').next().unwrap_or(n);
            base.split('.').next().unwrap_or(base).to_owned()
        })
        .collect();
    rage_formats::names::register(names);
}

/// The RDR2 game folder an archive belongs to.
pub fn game_folder(archive: &Path) -> Option<PathBuf> {
    archive.ancestors().find(|d| d.join("RDR2.exe").is_file()).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trip() {
        let mut idx = TextureIndex::default();
        idx.dictionaries.push(Location { archive: "textures_0.rpf".into(), chain: vec![0x1234, 0xabcdef01] });
        idx.textures.insert("nbx_ground_paving14_ab".into(), 0);
        let back = TextureIndex::from_text(&idx.to_text()).unwrap();
        assert_eq!(back.dictionaries, idx.dictionaries);
        assert_eq!(back.find("NBX_Ground_Paving14_ab"), Some(&idx.dictionaries[0]));
    }
}
