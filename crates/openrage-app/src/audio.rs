//! Loading audio containers (`.awc`) as the game sees them.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, bail};
use rage_crypto::GtaVKeys;
use rage_formats::awc::Awc;
use rage_rpf::vfs::Node;

pub fn is_audio(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".awc")
}

/// Reads and decrypts an `.awc` (mods included).
pub fn load(game_dir: &Path, keys: Option<Arc<GtaVKeys>>, vpath: &str) -> Result<Awc> {
    let (node, _) = rage_mods::resolve(game_dir, vpath, keys.clone())?;
    let data = match node {
        Node::ArchiveFile(rpf, id) => rpf.read(id)?,
        Node::PlainFile(p) => std::fs::read(p)?,
        _ => bail!("{vpath} is not a file"),
    };
    parse(data, keys.as_deref())
}

pub fn parse(data: Vec<u8>, keys: Option<&GtaVKeys>) -> Result<Awc> {
    let decrypt = |buf: &mut [u8]| {
        if let Some(k) = keys {
            k.decrypt_awc(buf);
        }
    };
    let has_key = keys.is_some_and(|k| k.awc_key.is_some());
    let decrypt: Option<rage_formats::awc::Decrypt> = if has_key { Some(&decrypt) } else { None };
    Ok(Awc::parse(data, decrypt)?)
}
