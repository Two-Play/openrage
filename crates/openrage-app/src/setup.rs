//! One-time key setup from the user's own game installation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rage_crypto::{GtaVKeys, decode_codewalker_magic, scan_executable};
use rage_rpf::Rpf7;

/// Game executables, in order of preference.
pub const EXECUTABLES: [&str; 2] = ["GTA5_Enhanced.exe", "GTA5.exe"];

/// The GTA V keys set up before, if they open this game's archives: switching
/// back to GTA V then needs neither a scan of the executable nor magic.dat.
pub fn cached_keys_for(game_dir: &Path) -> Option<Arc<GtaVKeys>> {
    let keys = crate::load_keys().ok().flatten()?;
    let probe = game_dir.join("common.rpf");
    (probe.is_file() && Rpf7::open(&probe, Some(Arc::clone(&keys))).is_ok()).then_some(keys)
}

pub fn find_exe(game_dir: &Path) -> Option<PathBuf> {
    EXECUTABLES.iter().map(|n| game_dir.join(n)).find(|p| p.is_file())
}

/// Where the NG keys came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Executable,
    KeyBlob,
}

/// Outcome of a successful setup.
#[derive(Debug, Clone)]
pub struct SetupReport {
    pub exe: PathBuf,
    pub scan_seconds: f32,
    pub ng_keys_in_exe: usize,
    pub ng_tables_in_exe: usize,
    pub source: KeySource,
    /// Entry count of `common.rpf`, decrypted as a proof that the keys work.
    pub verified_entries: Option<usize>,
    pub cache_path: PathBuf,
}

/// Error raised when the executable lacks the NG keys and no key blob was given.
#[derive(Debug)]
pub struct KeyBlobRequired;

impl std::fmt::Display for KeyBlobRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "this executable (GTA V Enhanced) does not contain the NG keys in plain form; \
             a CodeWalker key blob (magic.dat) is required",
        )
    }
}

impl std::error::Error for KeyBlobRequired {}

/// Derives the key set, verifies it against `common.rpf` and caches it.
///
/// `progress` receives short human-readable status lines.
pub fn derive_keys(
    game_dir: &Path,
    exe: Option<&Path>,
    magic: Option<&Path>,
    progress: &mut dyn FnMut(&str),
) -> Result<(Arc<GtaVKeys>, SetupReport)> {
    let exe = match exe {
        Some(p) => p.to_owned(),
        None => find_exe(game_dir).context("no GTA5_Enhanced.exe or GTA5.exe in the game directory")?,
    };
    progress(&format!("Scanning {} …", exe.display()));
    let started = Instant::now();
    let data = fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?;
    let scan = scan_executable(&data);
    let scan_seconds = started.elapsed().as_secs_f32();
    let aes = scan.aes_key.context("AES key not found — is this a GTA V executable?")?;

    let (keys, source) = if let Some(keys) = GtaVKeys::from_scan(&scan) {
        (keys, KeySource::Executable)
    } else if let Some(magic) = magic {
        progress("Decoding key blob …");
        let blob = fs::read(magic).with_context(|| format!("reading {}", magic.display()))?;
        let contents = decode_codewalker_magic(&blob, &aes)?;
        if scan.hash_lut.is_some_and(|lut| lut != contents.hash_lut) {
            bail!("the key blob does not match this executable (hash LUT differs)");
        }
        (GtaVKeys::from_magic(aes, &contents).context("key blob has an unexpected layout")?, KeySource::KeyBlob)
    } else {
        return Err(KeyBlobRequired.into());
    };

    let keys = Arc::new(keys);
    let probe = game_dir.join("common.rpf");
    let verified_entries = if probe.is_file() {
        progress("Verifying keys against common.rpf …");
        let rpf = Rpf7::open(&probe, Some(Arc::clone(&keys))).context("the keys failed to decrypt common.rpf")?;
        Some(rpf.entries().len())
    } else {
        None
    };

    let cache_path = crate::paths::gtav_keys().context("no per-user config directory available")?;
    keys.save(&cache_path)?;
    let report = SetupReport {
        exe,
        scan_seconds,
        ng_keys_in_exe: scan.ng_keys_found(),
        ng_tables_in_exe: scan.ng_tables_found(),
        source,
        verified_entries,
        cache_path,
    };
    Ok((keys, report))
}
