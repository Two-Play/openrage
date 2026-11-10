//! Writing changes into the OpenIV `mods` folder.
//!
//! A change to a file inside a game archive is written the OpenIV way: the
//! top-level archive is copied to `mods/<same path>` (or the copy already
//! there is used) and rewritten with the change. The new archive is written
//! to a temporary file next to it and moved into place only when complete.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rage_crypto::GtaVKeys;
use rage_rpf::{Rpf7, vfs};

use crate::repack::{Edit, repack_into};

/// A virtual path split into its top-level archive and the path inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Archive path relative to the game (and mods) folder, e.g. `x64e.rpf`.
    pub archive: String,
    /// Path inside that archive, e.g. `levels/gta5/vehicles.rpf/adder.ytd`.
    pub inner: String,
}

/// Splits `vpath` at its first `.rpf` file found in the game or mods folder.
pub fn split(game_dir: &Path, vpath: &str) -> Option<Target> {
    let mods = rage_mods::mods_dir(game_dir);
    let segs: Vec<&str> = vpath.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    for n in 1..segs.len() {
        let prefix = segs[..n].join("/");
        if !prefix.to_ascii_lowercase().ends_with(".rpf") {
            continue;
        }
        let is_file = |root: &Path| vfs::find_case_insensitive(root, &prefix).is_some_and(|p| p.is_file());
        if is_file(game_dir) || mods.as_deref().is_some_and(is_file) {
            return Some(Target { archive: prefix, inner: segs[n..].join("/") });
        }
    }
    None
}

/// Applies `edits` (paths inside `archive`) and writes the result to `mods/<archive>`.
/// Returns the path of the written file.
pub fn write_archive_into_mods(
    game_dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    archive: &str,
    edits: &BTreeMap<String, Edit>,
) -> Result<PathBuf> {
    let mods = match rage_mods::mods_dir(game_dir) {
        Some(m) => m,
        None => {
            let m = game_dir.join(rage_mods::overlay::MODS_FOLDER);
            fs::create_dir_all(&m).with_context(|| format!("creating {}", m.display()))?;
            m
        }
    };
    let in_mods = vfs::find_case_insensitive(&mods, archive).filter(|p| p.is_file());
    let source = match &in_mods {
        Some(p) => p.clone(),
        None => vfs::find_case_insensitive(game_dir, archive)
            .filter(|p| p.is_file())
            .with_context(|| format!("{archive} not found in the game folder"))?,
    };
    // Keep the game's casing for new copies so OpenIV.asi finds them.
    let rel = source.strip_prefix(if in_mods.is_some() { &mods } else { game_dir }).unwrap_or(Path::new(archive));
    let dest = mods.join(rel);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = dest.with_extension("rpf.openrage-tmp");
    {
        let rpf = Arc::new(Rpf7::open(&source, keys)?);
        let file = fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        let result = repack_into(&rpf, edits, &mut out).and_then(|_| {
            out.flush()?;
            out.get_ref().sync_all()?;
            Ok(())
        });
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    } // source archive closed before replacing it
    fs::rename(&tmp, &dest).with_context(|| format!("moving {} into place", dest.display()))?;
    Ok(dest)
}

/// Writes one changed entry (virtual path inside an archive) into the mods folder.
pub fn write_entry_into_mods(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str, edit: Edit) -> Result<PathBuf> {
    let Some(target) = split(game_dir, vpath) else { bail!("{vpath} is not inside an archive") };
    let edits = BTreeMap::from([(target.inner, edit)]);
    write_archive_into_mods(game_dir, keys, &target.archive, &edits)
}
