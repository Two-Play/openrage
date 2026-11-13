//! Plugins in the application: which ones the user enabled, and the game
//! files they may read.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use openrage_plugins::{DirEntry, GameFiles, GameId, Permission, Registry};
use rage_crypto::GtaVKeys;
use rage_rpf::vfs::Node;

use crate::config::Config;

/// What the user granted each enabled plugin.
pub fn grants(cfg: &Config) -> HashMap<String, Vec<Permission>> {
    cfg.plugins.iter().map(|(id, g)| (id.clone(), openrage_plugins::parse_grants(g))).collect()
}

/// All installed plugins, with their state from the settings.
pub fn registry(cfg: &Config) -> Registry {
    Registry::discover(&openrage_plugins::search_dirs(), &grants(cfg))
}

/// Enables a plugin with the permissions it asks for.
pub fn enable(cfg: &mut Config, id: &str, permissions: &[Permission]) {
    let grants: Vec<&str> = permissions.iter().map(|p| p.as_str()).collect();
    let grants = grants.join("+");
    match cfg.plugins.iter_mut().find(|(i, _)| i == id) {
        Some(entry) => entry.1 = grants,
        None => cfg.plugins.push((id.to_owned(), grants)),
    }
}

/// Disables a plugin; returns whether it was enabled.
pub fn disable(cfg: &mut Config, id: &str) -> bool {
    let before = cfg.plugins.len();
    cfg.plugins.retain(|(i, _)| i != id);
    cfg.plugins.len() != before
}

/// The game a folder holds.
pub fn game_id(dir: &Path) -> GameId {
    if dir.join("RDR2.exe").is_file() {
        GameId::Rdr2
    } else if crate::gta4::is_gta4_dir(dir) {
        GameId::Gta4
    } else {
        GameId::Gta5
    }
}

/// The game's files as plugins see them: read-only, with the changes in
/// `mods/` applied.
pub struct GameDir {
    dir: PathBuf,
    game: GameId,
    keys: Option<Arc<GtaVKeys>>,
}

impl GameDir {
    pub fn new(dir: &Path, keys: Option<Arc<GtaVKeys>>) -> Self {
        Self { dir: dir.to_path_buf(), game: game_id(dir), keys }
    }

    fn resolve(&self, vpath: &str) -> Result<Node> {
        Ok(rage_mods::resolve(&self.dir, vpath, self.keys.clone())?.0)
    }
}

impl GameFiles for GameDir {
    fn game(&self) -> Option<GameId> {
        Some(self.game)
    }

    fn read(&self, vpath: &str) -> Result<Vec<u8>> {
        Ok(match self.resolve(vpath)? {
            Node::PlainFile(p) => fs::read(p)?,
            Node::ArchiveFile(rpf, id) => rpf.read(id)?,
            Node::Rpf8File(rpf, i) => match rpf.entries().and_then(|e| e.get(i)) {
                Some(entry) => rpf.read(entry)?,
                None => bail!("{vpath}: not found"),
            },
            Node::Gta4File(a, id) => a.read(id)?,
            Node::Dir(_) | Node::ArchiveDir(..) | Node::Rpf8Dir(..) | Node::Gta4Dir(..) => {
                bail!("{vpath} is a folder")
            }
        })
    }

    fn list(&self, vpath: &str) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        match self.resolve(vpath)? {
            Node::Dir(dir) => {
                // A folder of the game, merged with the same folder in `mods`.
                let mut dirs = vec![dir];
                if let Some(mods) = rage_mods::mods_dir(&self.dir)
                    && let Some(m) = rage_rpf::vfs::find_case_insensitive(&mods, vpath)
                {
                    dirs.push(m);
                }
                if vpath.is_empty() {
                    dirs.truncate(1);
                }
                for d in dirs {
                    for e in fs::read_dir(&d)?.flatten() {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if out.iter().any(|o: &DirEntry| o.name.eq_ignore_ascii_case(&name)) {
                            continue;
                        }
                        let meta = e.metadata()?;
                        let is_dir = meta.is_dir() || name.to_ascii_lowercase().ends_with(".rpf");
                        out.push(DirEntry { name, is_dir, size: if meta.is_dir() { 0 } else { meta.len() } });
                    }
                }
            }
            Node::ArchiveDir(rpf, id) => {
                for c in rpf.children(id) {
                    let e = rpf.entry(c);
                    out.push(DirEntry { name: e.name.clone(), is_dir: e.is_dir() || e.is_archive(), size: e.size() });
                }
            }
            Node::Rpf8Dir(rpf, dir) => {
                let (dirs, files) = rpf.list_dir(&dir);
                for d in dirs {
                    out.push(DirEntry { name: d, is_dir: true, size: 0 });
                }
                for i in files {
                    let name = rpf.entry_name(i);
                    let size = rpf.entries().and_then(|e| e.get(i)).map_or(0, |e| e.size());
                    out.push(DirEntry {
                        name: name.rsplit('/').next().unwrap_or(&name).to_owned(),
                        is_dir: false,
                        size,
                    });
                }
            }
            Node::Gta4Dir(a, dir) => {
                for c in a.children(dir) {
                    let e = a.entry(c);
                    out.push(DirEntry { name: e.name.clone(), is_dir: e.is_dir(), size: e.size() });
                }
            }
            Node::PlainFile(_) | Node::ArchiveFile(..) | Node::Rpf8File(..) | Node::Gta4File(..) => {
                bail!("{vpath} is a file")
            }
        }
        out.sort_by_key(|e| (!e.is_dir, e.name.to_ascii_lowercase()));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabling_stores_grants() {
        let mut cfg = Config::default();
        enable(&mut cfg, "strings", &[]);
        enable(&mut cfg, "probe", &[Permission::ReadGame]);
        enable(&mut cfg, "strings", &[Permission::ReadGame]);
        assert_eq!(cfg.plugins, [("strings".into(), "read-game".into()), ("probe".into(), "read-game".into())]);
        assert_eq!(grants(&cfg)["probe"], [Permission::ReadGame]);
        assert!(disable(&mut cfg, "strings"));
        assert!(!disable(&mut cfg, "strings"));
        assert_eq!(cfg.plugins.len(), 1);
    }

    #[test]
    fn reads_and_lists_a_game_folder_with_mods() {
        let tmp = std::env::temp_dir().join(format!("openrage-app-plugins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("common/data")).unwrap();
        fs::create_dir_all(tmp.join("mods/common/data")).unwrap();
        fs::write(tmp.join("common/data/a.meta"), "base").unwrap();
        fs::write(tmp.join("common/data/b.meta"), "base b").unwrap();
        fs::write(tmp.join("mods/common/data/a.meta"), "modded").unwrap();
        fs::write(tmp.join("mods/common/data/c.meta"), "new").unwrap();
        let game = GameDir::new(&tmp, None);
        assert_eq!(game.game(), Some(GameId::Gta5));
        assert_eq!(game.read("common/data/a.meta").unwrap(), b"modded");
        assert_eq!(game.read("common/data/b.meta").unwrap(), b"base b");
        let names: Vec<_> = game.list("common/data").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["a.meta", "b.meta", "c.meta"]);
        assert!(game.list("").unwrap().iter().any(|e| e.name == "common" && e.is_dir));
        assert!(game.read("common").is_err());
        let _ = fs::remove_dir_all(&tmp);
    }
}
