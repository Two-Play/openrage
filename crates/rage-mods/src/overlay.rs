//! The effective file system: game directory overlaid with `mods`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_crypto::GtaVKeys;
use rage_rpf::RpfError;
use rage_rpf::vfs::{self, Node};

/// Name of OpenIV's mods folder inside the game directory.
pub const MODS_FOLDER: &str = "mods";

/// OpenRAGE's own data inside the mods folder (journal, temporary files); never part of a mod.
pub const OPENRAGE_FOLDER: &str = ".openrage";

/// The `mods` folder of a game installation, if present.
pub fn mods_dir(game_dir: &Path) -> Option<PathBuf> {
    vfs::match_case_insensitive(game_dir, MODS_FOLDER).filter(|p| p.is_dir())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModFileKind {
    /// Replaces a file of the same path in the game directory.
    Replaces,
    /// Exists only in `mods` (e.g. a new DLC pack).
    New,
}

/// A file inside the mods folder.
#[derive(Debug, Clone)]
pub struct ModFile {
    /// Path relative to the mods folder (= path in the game it overrides), `/`-separated.
    pub rel: String,
    pub kind: ModFileKind,
    pub size: u64,
    pub is_archive: bool,
}

/// Lists every file in the mods folder and whether it replaces a game file.
pub fn scan(game_dir: &Path) -> Vec<ModFile> {
    let Some(mods) = mods_dir(game_dir) else { return Vec::new() };
    let mut out = Vec::new();
    let mut stack = vec![(mods, String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") };
            if meta.is_dir() {
                if !name.eq_ignore_ascii_case(OPENRAGE_FOLDER) {
                    stack.push((entry.path(), rel));
                }
                continue;
            }
            let replaces = vfs::find_case_insensitive(game_dir, &rel).is_some_and(|p| p.is_file());
            out.push(ModFile {
                kind: if replaces { ModFileKind::Replaces } else { ModFileKind::New },
                size: meta.len(),
                is_archive: name.to_ascii_lowercase().ends_with(".rpf"),
                rel,
            });
        }
    }
    out.sort_by_key(|a| a.rel.to_lowercase());
    out
}

/// Where a resolved node comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Unmodified game file (or a game folder without a mods counterpart).
    Base,
    /// The top-level file comes from the mods folder.
    Mods,
    /// A folder that exists in both the game and the mods folder.
    Merged,
}

/// Resolves a virtual path the way the game sees it: the first top-level
/// *file* on the path is taken from `mods` if it exists there.
///
/// For [`Node::Dir`] results with [`Origin::Merged`], callers should merge the
/// listing of the returned (game) directory with the same folder in `mods`.
pub fn resolve(game_dir: &Path, vpath: &str, keys: Option<Arc<GtaVKeys>>) -> Result<(Node, Origin), RpfError> {
    let Some(mods) = mods_dir(game_dir) else { return Ok((vfs::resolve(game_dir, vpath, keys)?, Origin::Base)) };
    let segments: Vec<&str> = vpath.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let mut prefix = String::new();
    for seg in &segments {
        prefix = if prefix.is_empty() { (*seg).to_owned() } else { format!("{prefix}/{seg}") };
        let in_mods = vfs::find_case_insensitive(&mods, &prefix);
        let in_base = vfs::find_case_insensitive(game_dir, &prefix);
        if in_mods.as_ref().is_some_and(|p| p.is_file()) {
            return Ok((vfs::resolve(&mods, vpath, keys)?, Origin::Mods));
        }
        if in_base.as_ref().is_some_and(|p| p.is_file()) {
            return Ok((vfs::resolve(game_dir, vpath, keys)?, Origin::Base));
        }
        match (in_base, in_mods) {
            (Some(_), _) => continue,
            (None, Some(_)) => {
                // A folder that only exists in mods (e.g. a new DLC pack folder).
                let node = vfs::resolve(&mods, vpath, keys)?;
                return Ok((node, Origin::Mods));
            }
            (None, None) => return Err(RpfError::NotFound(vpath.to_owned())),
        }
    }
    // The whole path is a folder in the game directory.
    let node = vfs::resolve(game_dir, vpath, keys)?;
    let merged = vfs::find_case_insensitive(&mods, vpath).is_some_and(|p| p.is_dir()) || segments.is_empty();
    Ok((node, if merged { Origin::Merged } else { Origin::Base }))
}
