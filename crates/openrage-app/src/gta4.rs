//! GTA IV: key setup from the user's own `GTAIV.exe`, game detection and extraction.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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

// ---------------------------------------------------------------------------
// Textures for models

/// Where the game's texture dictionaries are and which dictionary a model uses.
#[derive(Default)]
pub struct TextureIndex {
    /// `.wtd` name (lower case, without extension) → archives holding one.
    dictionaries: HashMap<String, Vec<PathBuf>>,
    /// Model name → texture dictionary name (from the `.ide` files), lower case.
    model_txd: HashMap<String, String>,
    /// Texture dictionary → its parent (`txdp` sections), lower case.
    parent: HashMap<String, String>,
    /// Model names by hash (joaat of the lower-case name), as the `.ide` files write them.
    model_names: HashMap<u32, String>,
}

impl TextureIndex {
    /// Scans a GTA IV game folder: every `.ide` file and the tables of every `.img`.
    pub fn build(game: &Path) -> Self {
        let mut index = TextureIndex::default();
        let mut files = Vec::new();
        collect_files(game, &mut files);
        files.sort();
        for path in files {
            let ext = path.extension().map(|e| e.to_ascii_lowercase()).unwrap_or_default();
            if ext == "ide" {
                if let Ok(text) = fs::read(&path) {
                    index.add_ide(&String::from_utf8_lossy(&text));
                }
            } else if ext == "img"
                && Gta4Archive::file_is_gta4(&path)
                && let Ok(a) = Gta4Archive::open_default(&path)
            {
                for id in a.children(Gta4Archive::ROOT) {
                    let name = a.entry(id).name.to_ascii_lowercase();
                    if let Some(stem) = name.strip_suffix(".wtd") {
                        index.dictionaries.entry(stem.to_owned()).or_default().push(path.clone());
                    }
                }
            }
        }
        index
    }

    /// Reads the model → dictionary lines of an `.ide` file.
    pub fn add_ide(&mut self, text: &str) {
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if !line.contains(',') {
                section = if line.eq_ignore_ascii_case("end") { String::new() } else { line.to_ascii_lowercase() };
                continue;
            }
            if matches!(section.as_str(), "objs" | "tobj" | "anim" | "cars" | "peds" | "weap")
                && let Some(model) = line.split(',').next().map(str::trim).filter(|m| !m.is_empty())
            {
                self.model_names.entry(rage_core::hash::joaat(model)).or_insert_with(|| model.to_owned());
            }
            let mut fields = line.split(',').map(|f| f.trim().to_ascii_lowercase());
            let (Some(a), Some(b)) = (fields.next(), fields.next()) else { continue };
            if a.is_empty() || b.is_empty() || b == "null" {
                continue;
            }
            match section.as_str() {
                "objs" | "tobj" | "anim" | "cars" => {
                    self.model_txd.entry(a).or_insert(b);
                }
                "txdp" => {
                    self.parent.entry(a).or_insert(b);
                }
                _ => {}
            }
        }
    }

    /// A model's name from its hash, if an `.ide` file defines it.
    pub fn model_name(&self, hash: u32) -> Option<String> {
        self.model_names.get(&hash).cloned()
    }

    /// The dictionaries a model's textures may come from, most specific first:
    /// its own dictionary from the `.ide` files and that one's parents.
    pub fn dictionaries_for(&self, model: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut next = self.model_txd.get(&model.to_ascii_lowercase()).cloned();
        while let Some(name) = next.take() {
            if out.contains(&name) || out.len() > 16 {
                break;
            }
            next = self.parent.get(&name).cloned();
            out.push(name);
        }
        out
    }

    /// Archives that hold `<name>.wtd`, those below `prefer` first (an
    /// episode's own files before the base game's).
    pub fn archives_of(&self, name: &str, prefer: &Path) -> Vec<PathBuf> {
        let mut found = self.dictionaries.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default();
        found.sort_by_key(|p| !p.starts_with(prefer));
        found
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ide") || e.eq_ignore_ascii_case("img")) {
            out.push(p);
        }
    }
}

/// The texture index of a game folder, built once.
pub fn texture_index(game: &Path) -> Arc<TextureIndex> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<TextureIndex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(i) = cache.lock().ok().and_then(|c| c.get(game).cloned()) {
        return i;
    }
    let index = Arc::new(TextureIndex::build(game));
    if let Ok(mut c) = cache.lock() {
        c.insert(game.to_owned(), index.clone());
    }
    index
}

/// A placement file (`.wpl`) as IPL-style text, with model names from the
/// game's `.ide` files when `game` is given; also returns the number of records.
pub fn wpl_to_text(game: Option<&Path>, data: &[u8]) -> Result<(String, usize)> {
    let wpl = rage_formats::wpl::parse(data)?;
    let index = game.map(texture_index);
    let text = rage_formats::wpl::to_text(&wpl, |h| index.as_ref().and_then(|i| i.model_name(h)));
    Ok((text, rage_formats::wpl::record_count(&wpl)))
}

/// Header details of a compiled script (`.sco`) as (label, value) rows.
pub fn sco_rows(data: &[u8]) -> Option<Vec<(String, String)>> {
    let h = rage_formats::sco::header(data)?;
    Some(vec![
        ("Format".into(), format!("GTA IV script ({})", h.format_name())),
        ("Code".into(), format!("{} bytes", h.code_size)),
        ("Static variables".into(), h.statics.to_string()),
        ("Global variables".into(), h.globals.to_string()),
        ("Flags".into(), format!("0x{:08X}", h.flags)),
        ("Signature".into(), format!("0x{:08X}", h.signature)),
    ])
}

/// The GTA IV game folder an archive belongs to (the folder holding `GTAIV.exe`).
pub fn game_folder(archive: &Path) -> Option<PathBuf> {
    archive.ancestors().skip(1).find(|d| is_gta4_dir(d)).map(Path::to_owned)
}

/// Reads `<name>.wtd` from one of the archives the index lists.
fn load_dictionary(index: &TextureIndex, name: &str, prefer: &Path) -> Option<rage_formats::ytd::TextureDictionary> {
    for path in index.archives_of(name, prefer) {
        let Ok(a) = Gta4Archive::open_default(&path) else { continue };
        let Some(id) = a.find_child(Gta4Archive::ROOT, &format!("{name}.wtd")) else { continue };
        if let Some(dict) = a.read_resource(id).ok().and_then(|r| rage_formats::ytd::parse_gta4(&r).ok()) {
            return Some(dict);
        }
    }
    None
}

/// Texture dictionaries for a model of `archive` named `stem`: the one named
/// after the model in the same archive, the ones the `.ide` files give it
/// (with their parents), and for vehicles the shared `vehshare`. Dictionaries
/// none of whose textures the model uses are left out.
pub fn model_textures(
    archive: &Gta4Archive,
    stem: &str,
    used: &[String],
    vehicle: bool,
) -> Vec<(String, rage_formats::ytd::TextureDictionary)> {
    let mut out = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let wanted = |d: &rage_formats::ytd::TextureDictionary| {
        used.is_empty() || d.textures.iter().any(|t| used.iter().any(|u| u.eq_ignore_ascii_case(&t.name)))
    };
    if let Some(id) = archive.find_child(Gta4Archive::ROOT, &format!("{stem}.wtd"))
        && let Some(dict) = archive.read_resource(id).ok().and_then(|r| rage_formats::ytd::parse_gta4(&r).ok())
    {
        names.push(stem.to_ascii_lowercase());
        out.push((format!("{stem}.wtd"), dict));
    }
    let Some(game) = game_folder(archive.path()) else { return out };
    let index = texture_index(&game);
    let prefer = archive.path().parent().unwrap_or(&game).to_owned();
    // Episodes keep their own files in TLAD/ and TBoGT/: prefer those.
    let prefer = prefer.ancestors().find(|p| p.parent() == Some(game.as_path())).map(Path::to_owned).unwrap_or(prefer);
    let mut candidates = index.dictionaries_for(stem);
    if vehicle {
        candidates.push("vehshare".into());
    }
    for name in candidates {
        if names.contains(&name) || out.len() >= 8 {
            continue;
        }
        names.push(name.clone());
        if let Some(dict) = load_dictionary(&index, &name, &prefer).filter(|d| wanted(d)) {
            out.push((format!("{name}.wtd"), dict));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_model_dictionaries_from_ide_files() {
        let mut index = TextureIndex::default();
        index.add_ide(
            "objs\nNGE_Det_MH1, NGE_MH1, 60, 0\nend\ncars\nadmiral, admiral, car, ADMIRAL\nend\ntxdp\nNGE_MH1, manhat_generic\nend\n",
        );
        assert_eq!(index.dictionaries_for("nge_det_mh1"), ["nge_mh1", "manhat_generic"]);
        assert_eq!(index.dictionaries_for("ADMIRAL"), ["admiral"]);
        assert!(index.dictionaries_for("unknown").is_empty());
        assert_eq!(index.model_name(rage_core::hash::joaat("nge_det_mh1")).as_deref(), Some("NGE_Det_MH1"));
    }

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
