//! Persisted user settings shared by CLI and GUI
//! (`<config dir>/openrage/settings.conf`, `key=value` lines).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Default, Clone)]
pub struct Config {
    pub game_dir: Option<PathBuf>,
    /// Key blob used during setup, remembered so keys can be re-derived after game updates.
    pub magic_path: Option<PathBuf>,
}

fn dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("openrage"))
}

impl Config {
    pub fn load() -> Self {
        let mut cfg = Self::default();
        let Some(dir) = dir() else { return cfg };
        // `cli.conf` is the pre-GUI name of this file.
        let text = fs::read_to_string(dir.join("settings.conf"))
            .or_else(|_| fs::read_to_string(dir.join("cli.conf")))
            .unwrap_or_default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = PathBuf::from(v.trim());
            match k.trim() {
                "game_dir" => cfg.game_dir = Some(v),
                "magic_path" => cfg.magic_path = Some(v),
                _ => {}
            }
        }
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let dir = dir().context("no per-user config directory available")?;
        fs::create_dir_all(&dir)?;
        let mut text = String::new();
        if let Some(p) = &self.game_dir {
            text.push_str(&format!("game_dir={}\n", p.display()));
        }
        if let Some(p) = &self.magic_path {
            text.push_str(&format!("magic_path={}\n", p.display()));
        }
        let path = dir.join("settings.conf");
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }
}
