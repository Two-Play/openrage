//! GTA VI: the textures of models, which are in the texture pools.
//!
//! A model's shader names a texture by a 64-bit key; each texture pool
//! (`.ttpf`, see [`rage_formats::ttpf`]) lists the keys of its textures. So
//! the pools of a game are read once into a catalog of key → pool and number.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rage_formats::drawable::Drawable;
use rage_formats::drawable6::texture_name;
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
    Arc::clone(catalogs.entry(game_dir.to_path_buf()).or_insert_with(|| Arc::new(Catalog::build(game_dir))))
}

/// The textures the shaders of these drawables name, from the pools of the
/// game: a dictionary of them, `None` if none was found.
pub fn for_drawables(game_dir: &Path, drawables: &[(String, Drawable)]) -> Option<TextureDictionary> {
    let mut names: Vec<&str> =
        drawables.iter().flat_map(|(_, d)| &d.shaders).flat_map(|s| &s.textures).map(String::as_str).collect();
    names.sort_unstable();
    names.dedup();
    if names.is_empty() {
        return None;
    }
    let catalog = catalog(game_dir);
    let total = names.len().min(MAX_TEXTURES);
    let mut textures = Vec::new();
    for (i, name) in names.into_iter().take(MAX_TEXTURES).enumerate() {
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
