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
///
/// GTA V Enhanced never has them in plain form; some updates of GTA V Legacy
/// do not either, so this says nothing about the edition.
#[derive(Debug)]
pub struct KeyBlobRequired {
    /// The executable that was scanned.
    pub exe: PathBuf,
}

impl KeyBlobRequired {
    /// The executable's file name, for messages.
    pub fn exe_name(&self) -> String {
        self.exe.file_name().map_or_else(|| self.exe.display().to_string(), |n| n.to_string_lossy().into_owned())
    }
}

impl std::fmt::Display for KeyBlobRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} does not contain the NG keys in plain form (GTA V Enhanced never does, some GTA V Legacy \
             updates do not either); a CodeWalker key blob (magic.dat) is required",
            self.exe_name()
        )
    }
}

impl std::error::Error for KeyBlobRequired {}

/// CodeWalker's key blob in its repository, for people to look at.
pub const CODEWALKER_MAGIC_PAGE: &str =
    "https://github.com/dexyfex/CodeWalker/blob/master/CodeWalker.Core/Resources/magic.dat";

/// The same file at a fixed commit, so a download always gets these bytes.
const CODEWALKER_MAGIC_URL: &str = "https://raw.githubusercontent.com/dexyfex/CodeWalker/\
     485d56bec00262ed7fa472261cce7bbc6202b96e/CodeWalker.Core/Resources/magic.dat";

/// SHA-256 of that file (154,069 bytes). A download with other bytes is refused.
const CODEWALKER_MAGIC_SHA256: &str = "dc35981f822e892ced3aa81d31e7a96927d573ee28f67417592b5afeaf330832";

/// More than any key blob needs; a larger answer is not the file.
const MAGIC_MAX_BYTES: u64 = 1 << 20;

/// What a download of CodeWalker's key blob is, for asking the user first.
pub const CODEWALKER_MAGIC_NOTICE: &str = "OpenRAGE ships no keys. This downloads magic.dat from the CodeWalker \
     project on GitHub: CodeWalker's key blob, which holds Rockstar's archive keys in obfuscated form and opens \
     GTA V Legacy and Enhanced. It is saved in OpenRAGE's settings folder and only used on this computer.";

/// Downloads CodeWalker's key blob (only when the user asked for it), checks
/// that it is the expected file and saves it in OpenRAGE's settings folder.
/// Returns where it was saved.
pub fn download_codewalker_magic() -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    use ureq::tls::{Certificate, RootCerts, TlsConfig};

    let path = crate::paths::downloaded_magic().context("no per-user config directory available")?;
    // The system's certificate authorities (no bundled list).
    let native = rustls_native_certs::load_native_certs();
    let certs: Vec<Certificate<'static>> =
        native.certs.iter().map(|c| Certificate::from_der(c.as_ref()).to_owned()).collect();
    if certs.is_empty() {
        bail!("no certificate authorities found on this system to check the download with");
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .tls_config(TlsConfig::builder().root_certs(RootCerts::new_with_certs(&certs)).build())
        .build()
        .into();
    let data = agent
        .get(CODEWALKER_MAGIC_URL)
        .call()
        .context("downloading magic.dat from CodeWalker's repository")?
        .into_body()
        .with_config()
        .limit(MAGIC_MAX_BYTES)
        .read_to_vec()
        .context("reading magic.dat")?;
    let hash: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    if hash != CODEWALKER_MAGIC_SHA256 {
        bail!("the downloaded file is not CodeWalker's magic.dat (SHA-256 {hash})");
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let partial = path.with_extension("dat.part");
    fs::write(&partial, &data).with_context(|| format!("writing {}", partial.display()))?;
    fs::rename(&partial, &path).with_context(|| format!("saving {}", path.display()))?;
    Ok(path)
}

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
        return Err(KeyBlobRequired { exe }.into());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_blob_message_names_the_executable_not_an_edition() {
        let legacy = KeyBlobRequired { exe: PathBuf::from("/games/Grand Theft Auto V/GTA5.exe") };
        assert_eq!(legacy.exe_name(), "GTA5.exe");
        let message = legacy.to_string();
        assert!(message.starts_with("GTA5.exe does not contain the NG keys"), "{message}");
        assert!(message.contains("magic.dat"), "{message}");
        let enhanced = KeyBlobRequired { exe: PathBuf::from("GTA5_Enhanced.exe") };
        assert!(enhanced.to_string().starts_with("GTA5_Enhanced.exe "));
    }
}
