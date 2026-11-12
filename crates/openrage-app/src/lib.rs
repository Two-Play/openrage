//! Application logic shared by the OpenRAGE CLI and GUI:
//! settings, key setup and game detection.

pub mod anim;
pub mod audio;
pub mod config;
pub mod cutscene;
pub mod detect;
pub mod expr;
pub mod extract;
pub mod install;
pub mod journal;
pub mod media;
pub mod metadata;
pub mod metaped;
pub mod models;
pub mod natives;
pub mod oiv;
pub mod progress;
pub mod rdr2;
pub mod render;
pub mod repack;
pub mod setup;
pub mod suggest;
pub mod texttable;
pub mod textures;
pub mod world;
pub mod xmltree;

use std::sync::Arc;

use rage_crypto::GtaVKeys;

/// Loads the cached key set, if setup has been completed.
pub fn load_keys() -> Result<Option<Arc<GtaVKeys>>, rage_crypto::KeyCacheError> {
    let path = GtaVKeys::default_cache_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let mut keys = GtaVKeys::load(&path)?;
    // Key caches from before audio support lack the AWC key: take it from the
    // remembered key blob once.
    if keys.awc_key.is_none()
        && let Some(magic) = config::Config::load().magic_path
        && let Ok(blob) = std::fs::read(&magic)
        && let Ok(contents) = rage_crypto::decode_codewalker_magic(&blob, &keys.aes_key)
    {
        keys.awc_key = Some(contents.awc_key);
        let _ = keys.save(&path);
    }
    Ok(Some(Arc::new(keys)))
}

/// Location of the search index for a game installation
/// (`<cache dir>/openrage/index-<hash of the game path>.sqlite`).
pub fn index_path(game_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let canonical = game_dir.canonicalize().unwrap_or_else(|_| game_dir.to_owned());
    let id = rage_core::hash::jenkins_bytes(canonical.to_string_lossy().as_bytes());
    Some(dirs::cache_dir()?.join("openrage").join(format!("index-{id:08x}.sqlite")))
}

/// GTA V text language matching the user's locale (`LC_ALL`, `LC_MESSAGES`, `LANG`), if any.
pub fn game_language_from_locale() -> Option<&'static str> {
    let locale =
        ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))?;
    let locale = locale.to_ascii_lowercase();
    let (lang, region) = locale.split(['.', '@']).next()?.split_once('_').unwrap_or((&locale, ""));
    Some(match (lang, region) {
        ("en", _) => "american",
        ("de", _) => "german",
        ("fr", _) => "french",
        ("it", _) => "italian",
        ("es", "mx") => "mexican",
        ("es", _) => "spanish",
        ("pt", _) => "portuguese",
        ("pl", _) => "polish",
        ("ru", _) => "russian",
        ("ja", _) => "japanese",
        ("ko", _) => "korean",
        ("zh", "tw" | "hk") => "chinese",
        ("zh", _) => "chinesesimp",
        _ => return None,
    })
}
