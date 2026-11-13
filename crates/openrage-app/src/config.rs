//! Persisted user settings shared by CLI and GUI
//! (`<config dir>/openrage/settings.conf`, `key=value` lines).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Default, Clone)]
pub struct Config {
    /// The game opened last.
    pub game_dir: Option<PathBuf>,
    /// Every game set up so far (GTA V, RDR2, …), to switch between them.
    pub games: Vec<PathBuf>,
    /// The game to open at start without asking (`None`: ask when there are several).
    pub default_game: Option<PathBuf>,
    /// Key blob used during setup, remembered so keys can be re-derived after game updates.
    pub magic_path: Option<PathBuf>,
    /// GUI color scheme: `system`, `light` or `dark`.
    pub theme: Option<String>,
    /// GUI accent color: `system`, `orange` or `#rrggbb`.
    pub accent: Option<String>,
    /// Show the change history in its own window (default) instead of the file list.
    pub history_window: Option<bool>,
    /// Enabled plugins with the permissions the user granted them:
    /// `(id, "perm+perm")` (see `crate::plugins`).
    pub plugins: Vec<(String, String)>,
    /// Games whose changes are written into the game folder itself instead of
    /// the overlay folder (GTA IV only; see `crate::install::WriteTarget`).
    pub write_in_game: Vec<PathBuf>,
}

fn dir() -> Option<PathBuf> {
    crate::paths::config_dir()
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
            let v = v.trim();
            match k.trim() {
                "game_dir" => cfg.game_dir = Some(PathBuf::from(v)),
                "game" => cfg.games.push(PathBuf::from(v)),
                "default_game" => cfg.default_game = Some(PathBuf::from(v)),
                "magic_path" => cfg.magic_path = Some(PathBuf::from(v)),
                "theme" => cfg.theme = Some(v.to_owned()),
                "accent" => cfg.accent = Some(v.to_owned()),
                "history_window" => cfg.history_window = Some(v == "true"),
                "write_in_game" => cfg.write_in_game.push(PathBuf::from(v)),
                "plugin" => {
                    let (id, grants) = v.split_once(':').unwrap_or((v, ""));
                    cfg.plugins.push((id.trim().to_owned(), grants.trim().to_owned()));
                }
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
        for g in &self.games {
            text.push_str(&format!("game={}\n", g.display()));
        }
        if let Some(p) = &self.default_game {
            text.push_str(&format!("default_game={}\n", p.display()));
        }
        if let Some(p) = &self.magic_path {
            text.push_str(&format!("magic_path={}\n", p.display()));
        }
        if let Some(t) = &self.theme {
            text.push_str(&format!("theme={t}\n"));
        }
        if let Some(a) = &self.accent {
            text.push_str(&format!("accent={a}\n"));
        }
        if let Some(h) = self.history_window {
            text.push_str(&format!("history_window={h}\n"));
        }
        for g in &self.write_in_game {
            text.push_str(&format!("write_in_game={}\n", g.display()));
        }
        for (id, grants) in &self.plugins {
            text.push_str(&format!("plugin={id}:{grants}\n"));
        }
        let path = dir.join("settings.conf");
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }
}

impl Config {
    /// Adds a game to the list of games set up (once).
    pub fn remember_game(&mut self, dir: &std::path::Path) {
        if !self.games.iter().any(|g| g == dir) {
            self.games.push(dir.to_path_buf());
        }
    }

    /// The games set up so far, including the one opened last (older settings
    /// only had that), without duplicates.
    pub fn known_games(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for g in self.game_dir.iter().chain(&self.games) {
            if !out.contains(g) {
                out.push(g.clone());
            }
        }
        out
    }
}
