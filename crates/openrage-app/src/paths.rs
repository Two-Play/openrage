//! Where OpenRAGE keeps its files.
//!
//! The format libraries (`rage-crypto`, `rage-rpf`, …) take every path as a
//! parameter; the locations below are OpenRAGE's own choice, so they live here.

use std::path::{Path, PathBuf};

/// `<config dir>/openrage`: settings and archive keys.
pub fn config_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("openrage"))
}

/// `<cache dir>/openrage`: indexes that can be rebuilt from the game.
pub fn cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("openrage"))
}

/// The GTA V key cache (`<config dir>/openrage/gtav.keys`).
pub fn gtav_keys() -> Option<PathBuf> {
    Some(config_dir()?.join("gtav.keys"))
}

/// The GTA IV key cache (`<config dir>/openrage/gta4.keys`).
pub fn gta4_keys() -> Option<PathBuf> {
    Some(config_dir()?.join("gta4.keys"))
}

/// The RDR2 key store (`<config dir>/openrage/rdr2.keys`).
pub fn rdr2_keys() -> Option<PathBuf> {
    Some(config_dir()?.join("rdr2.keys"))
}

/// A cache file that belongs to one game folder:
/// `<cache dir>/openrage/<prefix>-<hash of the game path>.<ext>`.
pub fn game_cache(game: &Path, prefix: &str, ext: &str) -> Option<PathBuf> {
    let canonical = game.canonicalize().unwrap_or_else(|_| game.to_path_buf());
    let id = rage_core::hash::jenkins_bytes(canonical.to_string_lossy().as_bytes());
    Some(cache_dir()?.join(format!("{prefix}-{id:08x}.{ext}")))
}

/// The cached RDR2 archive file names of a game folder.
pub fn rdr2_names(game: &Path) -> Option<PathBuf> {
    game_cache(game, "rdr2-names", "tsv")
}
