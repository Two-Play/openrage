//! GTA VI: the textures of models, which are in the texture pools.
//!
//! A model's shader names a texture by a 64-bit key; each texture pool
//! (`.ttpf`, see [`rage_formats::ttpf`]) lists the keys of its textures. So
//! the pools of a game are read once into a catalog of key → pool and number.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use rage_formats::drawable::Drawable;
use rage_formats::drawable::param::{BUMP_TEX, DIFFUSE_TEX, DIFFUSE_TEX2, LAYER_1_TEX, SPECULAR_TEX};
use rage_formats::drawable6::texture_name;
use rage_formats::texture::Texture;
use rage_formats::ttpf::Pool;
use rage_formats::ytd::{Layout, TextureDictionary};

/// Textures on a model are read at most this many pixels wide and high.
const MAX_SIDE: u32 = 1024;
/// A model gets at most this many textures.
const MAX_TEXTURES: usize = 64;

/// Where the textures of a game are: per key, the pool and the number in it.
#[derive(Default)]
pub struct Catalog {
    pools: Vec<(PathBuf, Pool)>,
    by_key: HashMap<u64, (usize, usize)>,
}

impl Catalog {
    /// Reads the tables of all texture pools below `game_dir`.
    fn build(game_dir: &Path) -> Catalog {
        let mut files = Vec::new();
        crate::detect::collect_archives(game_dir, &["ttpf"], &mut files);
        let mut catalog = Catalog::default();
        for file in files {
            let Ok(pool) = Pool::open(&file) else { continue };
            for (number, key) in pool.keys().iter().enumerate().filter(|(_, key)| **key != 0) {
                // The same texture can be in several pools: the first one that has data wins.
                let has_data = pool.descriptor(number).is_some_and(|d| d.base.is_some());
                let known = catalog
                    .by_key
                    .get(key)
                    .is_some_and(|(p, n)| catalog.pools[*p].1.descriptor(*n).is_some_and(|d| d.base.is_some()));
                if !known && (has_data || !catalog.by_key.contains_key(key)) {
                    catalog.by_key.insert(*key, (catalog.pools.len(), number));
                }
            }
            catalog.pools.push((file, pool));
        }
        catalog
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// The texture with this key, with mip levels, the same for all that use
    /// it at the time.
    pub fn shared(&self, key: u64, side: u32) -> Option<Arc<Texture>> {
        type Known = HashMap<(u64, u32), Weak<Texture>>;
        static KNOWN: OnceLock<Mutex<Known>> = OnceLock::new();
        let lock = || KNOWN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        if let Some(known) = lock().get(&(key, side)).and_then(Weak::upgrade) {
            return Some(known);
        }
        let (pool, number) = *self.by_key.get(&key)?;
        let mut texture = self.pools[pool].1.texture_with_mips(number, Some(side)).ok()?;
        texture.name = texture_name(key);
        let mut known = lock();
        // Another thread may have been quicker.
        if let Some(theirs) = known.get(&(key, side)).and_then(Weak::upgrade) {
            return Some(theirs);
        }
        let texture = Arc::new(texture);
        if known.len().is_power_of_two() {
            known.retain(|_, t| t.strong_count() > 0);
        }
        known.insert((key, side), Arc::downgrade(&texture));
        Some(texture)
    }

    /// The texture with this key, named by the key.
    pub fn texture(&self, key: u64) -> Option<rage_formats::texture::Texture> {
        let (pool, number) = *self.by_key.get(&key)?;
        let mut texture = self.pools[pool].1.texture(number, Some(MAX_SIDE)).ok()?;
        texture.name = texture_name(key);
        Some(texture)
    }
}

/// The catalog of a game's texture pools; read on first use and kept.
pub fn catalog(game_dir: &Path) -> Arc<Catalog> {
    static CATALOGS: OnceLock<Mutex<HashMap<PathBuf, Arc<Catalog>>>> = OnceLock::new();
    let mut catalogs = CATALOGS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(
        catalogs
            .entry(game_dir.to_path_buf())
            .or_insert_with(|| Arc::new(rage_rpf::in_own_threads(|| Catalog::build(game_dir)))),
    )
}

/// The names of the textures of shaders, of these kinds.
fn names<'a>(drawables: &'a [(String, Drawable)], wanted: &[u32]) -> Vec<&'a str> {
    let mut names: Vec<&str> = drawables
        .iter()
        .flat_map(|(_, d)| &d.shaders)
        .flat_map(|s| &s.samplers)
        .filter(|(parameter, _)| wanted.contains(parameter))
        .map(|(_, name)| name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names.truncate(MAX_TEXTURES);
    names
}

/// The textures of drawables, shared with other models: their colours, with
/// `all` normal and specular maps too.
pub fn shared_for(game_dir: &Path, drawables: &[(String, Drawable)], side: u32, all: bool) -> Vec<Arc<Texture>> {
    let wanted: &[u32] = if all {
        &[DIFFUSE_TEX, DIFFUSE_TEX2, LAYER_1_TEX, BUMP_TEX, SPECULAR_TEX]
    } else if side > 256 {
        &[DIFFUSE_TEX, DIFFUSE_TEX2, LAYER_1_TEX]
    } else {
        &[DIFFUSE_TEX]
    };
    let names = names(drawables, wanted);
    if names.is_empty() {
        return Vec::new();
    }
    let catalog = catalog(game_dir);
    let found = names.iter().filter_map(|n| catalog.shared(u64::from_str_radix(n, 16).ok()?, side));
    // Not the white stand-ins for a colour the game computes.
    let stands_in = |t: &Texture| {
        t.width <= 8
            && t.to_rgba().is_some_and(|rgba| rgba.as_chunks::<4>().0.iter().all(|p| p[..3].iter().all(|c| *c > 225)))
    };
    found.filter(|t| !stands_in(t)).collect()
}

/// The textures the shaders of these drawables name, from the pools of the
/// game: a dictionary of them, `None` if none was found.
pub fn for_drawables(game_dir: &Path, drawables: &[(String, Drawable)]) -> Option<TextureDictionary> {
    let names = names(drawables, &[DIFFUSE_TEX, DIFFUSE_TEX2, LAYER_1_TEX, BUMP_TEX, SPECULAR_TEX]);
    if names.is_empty() {
        return None;
    }
    let catalog = catalog(game_dir);
    let total = names.len();
    let mut textures = Vec::new();
    for (i, name) in names.into_iter().enumerate() {
        crate::progress::set("Loading textures", i as u64, total as u64);
        if let Some(texture) = u64::from_str_radix(name, 16).ok().and_then(|key| catalog.texture(key)) {
            textures.push(texture);
        }
    }
    (!textures.is_empty()).then(|| TextureDictionary {
        textures,
        errors: Vec::new(),
        layout: Layout::Enhanced,
        version: 0,
        root: Vec::new(),
        stored: Vec::new(),
    })
}
