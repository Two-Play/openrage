//! Names of script natives, from a `natives.json` the user provides (the
//! common NativeDB format: `{ "NAMESPACE": { "0xHASH": { "name": "…" } } }`).
//!
//! OpenRAGE ships no native list; `openrage natives <file>` (or Help → Load
//! native names… in the GUI) copies one into the config folder. GTA V's
//! scripts call natives by their original hashes, which such lists use.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use anyhow::{Context, Result, bail};

/// Where the imported list lives.
pub fn path() -> Option<PathBuf> {
    Some(crate::paths::config_dir()?.join("natives.json"))
}

fn table() -> &'static RwLock<Option<HashMap<u64, String>>> {
    static TABLE: OnceLock<RwLock<Option<HashMap<u64, String>>>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(None))
}

/// Reads a NativeDB-style list into hash → `NAMESPACE::NAME`.
pub fn parse(json: &str) -> Result<HashMap<u64, String>> {
    let value: serde_json::Value = serde_json::from_str(json).context("not valid JSON")?;
    let namespaces = value.as_object().context("expected an object of namespaces")?;
    let mut out = HashMap::new();
    for (ns, natives) in namespaces {
        let Some(natives) = natives.as_object() else { continue };
        for (hash, info) in natives {
            let Some(hash) = hash.strip_prefix("0x").and_then(|h| u64::from_str_radix(h, 16).ok()) else { continue };
            let Some(name) = info.get("name").and_then(|n| n.as_str()) else { continue };
            out.insert(hash, format!("{ns}::{name}"));
        }
    }
    if out.is_empty() {
        bail!("no natives found (expected {{\"NAMESPACE\": {{\"0xHASH\": {{\"name\": …}}}}}})");
    }
    Ok(out)
}

fn ensure_loaded() {
    if table().read().map(|t| t.is_some()).unwrap_or(true) {
        return;
    }
    let loaded = path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|j| parse(&j).ok()).unwrap_or_default();
    if let Ok(mut t) = table().write() {
        *t = Some(loaded);
    }
}

/// The name of a native, if a list was imported.
pub fn name(hash: u64) -> Option<String> {
    ensure_loaded();
    table().read().ok()?.as_ref()?.get(&hash).cloned()
}

/// How many natives are known (0 without an imported list).
pub fn count() -> usize {
    ensure_loaded();
    table().read().ok().and_then(|t| t.as_ref().map(HashMap::len)).unwrap_or(0)
}

/// Checks a list and copies it into the config folder; returns how many natives it names.
pub fn import(file: &Path) -> Result<usize> {
    let json = std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let natives = parse(&json)?;
    let target = path().context("no config folder")?;
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&target, json)?;
    let n = natives.len();
    if let Ok(mut t) = table().write() {
        *t = Some(natives);
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nativedb_json() {
        let json = r#"{"PLAYER": {"0xD80958FC74E988A6": {"name": "PLAYER_PED_ID", "params": []}},
                       "SYSTEM": {"0x4EDE34FBADD967A6": {"name": "WAIT"}, "bad": {}}}"#;
        let n = parse(json).unwrap();
        assert_eq!(n.get(&0xD80958FC74E988A6).map(String::as_str), Some("PLAYER::PLAYER_PED_ID"));
        assert_eq!(n.len(), 2);
        assert!(parse("{}").is_err());
    }
}
