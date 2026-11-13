//! GTA IV: key setup from the user's own `GTAIV.exe`, game detection and extraction.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rage_crypto::Gta4Key;
use rage_rpf::{Gta4Archive, Gta4EntryId, vfs};

use crate::extract::ExtractSummary;

/// The game executable, which holds the archive key.
pub const EXECUTABLE: &str = "GTAIV.exe";

/// An encrypted archive every GTA IV installation has, used to check the key.
const PROOF_ARCHIVE: &str = "pc/models/cdimages/vehicles.img";

/// Whether `dir` is a GTA IV game folder (the one holding `GTAIV.exe`).
pub fn is_gta4_dir(dir: &Path) -> bool {
    dir.join(EXECUTABLE).is_file()
}

/// The Steam folder `Grand Theft Auto IV` keeps the game in its `GTAIV`
/// subfolder: return that subfolder when `dir` is the outer one.
pub fn normalize_dir(dir: &Path) -> PathBuf {
    let inner = dir.join("GTAIV");
    if !is_gta4_dir(dir) && is_gta4_dir(&inner) { inner } else { dir.to_owned() }
}

/// Outcome of a key setup.
#[derive(Debug)]
pub struct Gta4Setup {
    pub exe: PathBuf,
    pub scan_seconds: f32,
    /// Entries of the archive that was decrypted to prove the key works.
    pub verified_entries: Option<usize>,
    pub cache_path: PathBuf,
}

/// Finds the archive key in the game's executable, checks it on one of the
/// game's archives and caches it in the config folder.
pub fn derive_key(game_dir: &Path) -> Result<(Arc<Gta4Key>, Gta4Setup)> {
    let exe = game_dir.join(EXECUTABLE);
    if !exe.is_file() {
        bail!("{} does not contain {EXECUTABLE}", game_dir.display());
    }
    let started = Instant::now();
    let data = fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?;
    let key = Gta4Key::find_in_executable(&data)
        .with_context(|| format!("the archive key was not found in {}", exe.display()))?;
    let scan_seconds = started.elapsed().as_secs_f32();
    let key = Arc::new(key);
    let verified_entries = match vfs::find_case_insensitive(game_dir, PROOF_ARCHIVE) {
        Some(p) => Some(Gta4Archive::open(&p, Some(key.clone()))?.len() - 1),
        None => None,
    };
    let cache_path = Gta4Key::default_cache_path()?;
    key.save(&cache_path)?;
    Ok((key, Gta4Setup { exe, scan_seconds, verified_entries, cache_path }))
}

/// Makes the key available to archives opened through the virtual file
/// system: from the cache, or found in `GTAIV.exe` and cached the first time.
pub fn register(game_dir: &Path) -> Result<Arc<Gta4Key>> {
    let cached = Gta4Key::default_cache_path().ok().filter(|p| p.is_file()).and_then(|p| Gta4Key::load(&p).ok());
    let key = match cached {
        Some(k) => Arc::new(k),
        None => derive_key(game_dir)?.0,
    };
    rage_rpf::gta4::set_default_key(Some(key.clone()));
    Ok(key)
}

/// Extracts an entry of a GTA IV archive (a file, a folder or the whole
/// archive) below `out`. Files keep their stored form: resources stay RSC5
/// files, the form OpenIV exports.
pub fn extract(
    archive: &Gta4Archive,
    id: Gta4EntryId,
    out: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&ExtractSummary, &str),
) -> Result<ExtractSummary> {
    let mut summary = ExtractSummary { target: out.to_path_buf(), ..Default::default() };
    let mut write = |i: Gta4EntryId, rel: &str, s: &mut ExtractSummary| -> Result<()> {
        let target = out.join(vfs::safe_relative_path(rel).with_context(|| format!("unsafe path {rel}"))?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        match archive.read(i) {
            Ok(data) => {
                fs::write(&target, &data)?;
                s.files += 1;
                s.bytes += data.len() as u64;
            }
            Err(e) => s.failures.push(e.to_string()),
        }
        progress(s, rel);
        Ok(())
    };
    let entry = archive.entry(id);
    if !entry.is_dir() {
        write(id, &entry.name, &mut summary)?;
        summary.target = out.join(&entry.name);
        return Ok(summary);
    }
    // The archive itself extracts into a folder named after it.
    let base = if id == Gta4Archive::ROOT { archive.display_name().to_owned() } else { entry.name.clone() };
    summary.target = out.join(&base);
    let mut files = Vec::new();
    vfs::walk_gta4(archive, id, &base, &mut |i, p| {
        if !archive.entry(i).is_dir() {
            files.push((i, p.to_owned()));
        }
    });
    for (i, rel) in files {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        write(i, &rel, &mut summary)?;
    }
    Ok(summary)
}

/// Short type label of an entry for listings: `dir`, `resource` or `file`.
pub fn kind_label(archive: &Gta4Archive, id: Gta4EntryId) -> &'static str {
    let e = archive.entry(id);
    if e.is_dir() {
        "dir"
    } else if e.resource_type().is_some() {
        "resource"
    } else {
        "file"
    }
}

/// What a resource type number stands for.
pub fn resource_type_name(t: u32) -> &'static str {
    match t {
        1 => "generic",
        7 | 8 => "texture dictionary",
        32 => "bounds",
        110 => "drawable",
        112 => "fragment",
        _ => "resource",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_the_steam_folder() {
        let dir = std::env::temp_dir().join(format!("openrage-gta4-norm-{}", std::process::id()));
        let inner = dir.join("GTAIV");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join(EXECUTABLE), b"").unwrap();
        assert_eq!(normalize_dir(&dir), inner);
        assert_eq!(normalize_dir(&inner), inner);
        let _ = fs::remove_dir_all(&dir);
    }
}
