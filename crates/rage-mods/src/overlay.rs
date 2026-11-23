//! The effective file system: game directory overlaid with `mods`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rage_crypto::GtaVKeys;
use rage_rpf::RpfError;
use rage_rpf::vfs::{self, Node};

/// Name of OpenIV's mods folder inside the game directory.
pub const MODS_FOLDER: &str = "mods";

/// GTA IV's counterpart: the `update` folder FusionFix (with the Ultimate
/// ASI Loader) loads files from instead of the game's own.
pub const UPDATE_FOLDER: &str = "update";

/// Red Dead Redemption 2's counterpart: the "replace" folder of Lenny's Mod
/// Loader (LML), which loads `lml/replace/<archive>/<path>` in place of the
/// file at `<path>` inside the game's `<archive>.rpf` (see [`rdr2_rel`]).
pub const LML_REPLACE_FOLDER: &str = "lml/replace";

/// Whether `game_dir` holds Red Dead Redemption 2 (`RDR2.exe`).
pub fn is_rdr2(game_dir: &Path) -> bool {
    game_dir.join("RDR2.exe").is_file()
}

/// The overlay folder's name for the game in `game_dir`: `update` for GTA IV
/// (the folder holding `GTAIV.exe`), `lml/replace` for RDR2, `mods` otherwise.
pub fn folder_name(game_dir: &Path) -> &'static str {
    if game_dir.join("GTAIV.exe").is_file() {
        UPDATE_FOLDER
    } else if is_rdr2(game_dir) {
        LML_REPLACE_FOLDER
    } else {
        MODS_FOLDER
    }
}

/// RDR2: a virtual path split at its top-level archive, `(archive, inner)`
/// (`update_4.rpf`, `x64/data/foo.ymt`); the archive path as found in the
/// game folder. `None` for paths outside the game's archives.
pub fn rdr2_split(game_dir: &Path, vpath: &str) -> Option<(String, String)> {
    let segs: Vec<&str> = vpath.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    for n in 1..=segs.len() {
        if !segs[n - 1].to_ascii_lowercase().ends_with(".rpf") {
            continue;
        }
        let prefix = segs[..n].join("/");
        if let Some(found) = vfs::find_case_insensitive(game_dir, &prefix).filter(|p| p.is_file()) {
            let archive = found.strip_prefix(game_dir).ok()?.to_string_lossy().replace('\\', "/");
            return Some((archive, segs[n..].join("/")));
        }
        return None;
    }
    None
}

/// RDR2: where a file inside one of the game's archives goes in the
/// `lml/replace` folder: the archive's path without `.rpf`, then the path
/// inside it (`update_4.rpf` + `x64/data/foo.ymt` → `update_4/x64/data/foo.ymt`).
pub fn rdr2_rel(archive: &str, inner: &str) -> String {
    let stem = archive
        .len()
        .checked_sub(4)
        .filter(|&n| archive[n..].eq_ignore_ascii_case(".rpf"))
        .map_or(archive, |n| &archive[..n]);
    if inner.is_empty() { stem.to_owned() } else { format!("{stem}/{inner}") }
}

/// RDR2: the virtual path a file in the `lml/replace` folder stands for
/// (`update_4/x64/data/foo.ymt` → `update_4.rpf/x64/data/foo.ymt`), if the
/// game has the archive it names.
pub fn rdr2_vpath(game_dir: &Path, rel: &str) -> Option<String> {
    let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    for n in 1..segs.len() {
        let archive = format!("{}.rpf", segs[..n].join("/"));
        if let Some(found) = vfs::find_case_insensitive(game_dir, &archive).filter(|p| p.is_file()) {
            let archive = found.strip_prefix(game_dir).ok()?.to_string_lossy().replace('\\', "/");
            return Some(format!("{archive}/{}", segs[n..].join("/")));
        }
    }
    None
}

/// Where the overlay folder keeps the counterpart of a virtual path:
/// `mods/<vpath>` (GTA IV: `update/<vpath>`), or for RDR2 the `lml/replace`
/// path of a file inside an archive. The path need not exist.
pub fn overlay_path(game_dir: &Path, vpath: &str) -> Option<PathBuf> {
    let mods = game_dir.join(folder_name(game_dir));
    if is_rdr2(game_dir) {
        let (archive, inner) = rdr2_split(game_dir, vpath)?;
        let rel = rdr2_rel(&archive, &inner);
        return Some(vfs::find_case_insensitive(&mods, &rel).unwrap_or_else(|| mods.join(rel)));
    }
    Some(vfs::find_case_insensitive(&mods, vpath).unwrap_or_else(|| mods.join(vpath)))
}

/// OpenRAGE's own data inside the mods folder (journal, temporary files); never part of a mod.
pub const OPENRAGE_FOLDER: &str = ".openrage";

/// Inside [`OPENRAGE_FOLDER`]: originals of game files changed in place (GTA IV).
pub const ORIGINALS_FOLDER: &str = "originals";

/// The `mods` folder of a game installation (GTA IV: the `update` folder), if present.
pub fn mods_dir(game_dir: &Path) -> Option<PathBuf> {
    vfs::find_case_insensitive(game_dir, folder_name(game_dir)).filter(|p| p.is_dir())
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
    let rdr2 = is_rdr2(game_dir);
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
            let (rel, replaces) = if rdr2 {
                // `update_4/x64/…` stands for `update_4.rpf/x64/…`; whether the
                // archive really has that file is not checked here.
                match rdr2_vpath(game_dir, &rel) {
                    Some(v) => (v, true),
                    None => (rel, false),
                }
            } else {
                let replaces = vfs::find_case_insensitive(game_dir, &rel).is_some_and(|p| p.is_file());
                (rel, replaces)
            };
            out.push(ModFile {
                kind: if replaces { ModFileKind::Replaces } else { ModFileKind::New },
                size: meta.len(),
                is_archive: name.to_ascii_lowercase().ends_with(".rpf") || name.to_ascii_lowercase().ends_with(".img"),
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
    if is_rdr2(game_dir) {
        return resolve_rdr2(game_dir, &mods, vpath, keys);
    }
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

/// [`resolve`] for RDR2: inside a game archive, the first file on the path
/// that the `lml/replace` folder has (a replaced file, or a nested archive
/// that was changed) is taken from there.
fn resolve_rdr2(
    game_dir: &Path,
    mods: &Path,
    vpath: &str,
    keys: Option<Arc<GtaVKeys>>,
) -> Result<(Node, Origin), RpfError> {
    let Some((archive, inner)) = rdr2_split(game_dir, vpath) else {
        return Ok((vfs::resolve(game_dir, vpath, keys)?, Origin::Base));
    };
    let segs: Vec<&str> = inner.split('/').filter(|s| !s.is_empty()).collect();
    let root = vfs::find_case_insensitive(mods, &rdr2_rel(&archive, ""));
    if let Some(root) = &root {
        for n in 1..=segs.len() {
            if vfs::find_case_insensitive(root, &segs[..n].join("/")).is_some_and(|p| p.is_file()) {
                return Ok((vfs::resolve(root, &inner, keys)?, Origin::Mods));
            }
        }
    }
    let node = vfs::resolve(game_dir, vpath, keys)?;
    let merged = root.is_some_and(|r| vfs::find_case_insensitive(&r, &inner).is_some_and(|p| p.is_dir()));
    Ok((node, if merged { Origin::Merged } else { Origin::Base }))
}
