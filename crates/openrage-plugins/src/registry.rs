//! Finding plugins and deciding which may run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::manifest::{self, Action, GameId, Manifest, Permission};

/// Whether a plugin may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Enabled,
    /// New or switched off: plugins only run once the user enabled them.
    Disabled,
    /// Enabled before, but this version asks for permissions the user has
    /// not granted yet; it stays off until they are.
    NeedsApproval(Vec<Permission>),
}

/// A plugin with a valid manifest.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub status: Status,
    /// The permissions it runs with (granted and asked for).
    pub granted: Vec<Permission>,
}

impl Plugin {
    pub fn wasm_path(&self) -> PathBuf {
        self.dir.join(manifest::safe_relative(&self.manifest.wasm).unwrap_or_default())
    }

    pub fn enabled(&self) -> bool {
        self.status == Status::Enabled
    }
}

/// A plugin folder that could not be read.
#[derive(Debug, Clone)]
pub struct Broken {
    pub dir: PathBuf,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct Registry {
    pub plugins: Vec<Plugin>,
    pub broken: Vec<Broken>,
}

/// Where plugins are looked for, in order (the first plugin with an id wins):
/// the folders in `OPENRAGE_PLUGIN_PATH`, the user's `openrage/plugins`
/// config folder, and `plugins` next to the executable (portable installs).
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("OPENRAGE_PLUGIN_PATH")
        .map(|v| std::env::split_paths(&v).filter(|p| !p.as_os_str().is_empty()).collect())
        .unwrap_or_default();
    if let Some(config) = dirs::config_dir() {
        dirs.push(config.join("openrage").join("plugins"));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        dirs.push(exe_dir.join("plugins"));
    }
    let mut unique: Vec<PathBuf> = Vec::new();
    for d in dirs {
        if !unique.contains(&d) {
            unique.push(d);
        }
    }
    unique
}

impl Registry {
    /// Reads every plugin folder in `dirs` (each sub-folder with a
    /// `plugin.toml`). `grants` holds what the user granted: plugin id →
    /// permissions; a plugin without an entry is disabled.
    pub fn discover(dirs: &[PathBuf], grants: &HashMap<String, Vec<Permission>>) -> Self {
        let mut reg = Registry::default();
        for root in dirs {
            let Ok(read) = std::fs::read_dir(root) else { continue };
            let mut folders: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            folders.sort();
            for dir in folders {
                if !dir.join(manifest::FILE_NAME).is_file() {
                    continue;
                }
                match Manifest::load(&dir) {
                    Ok(m) => {
                        if let Some(first) = reg.plugins.iter().find(|p| p.manifest.id == m.id) {
                            let error = format!("id {:?} is already used by {}", m.id, first.dir.display());
                            reg.broken.push(Broken { dir, error });
                            continue;
                        }
                        let (status, granted) = match grants.get(&m.id) {
                            None => (Status::Disabled, Vec::new()),
                            Some(g) => {
                                let missing: Vec<_> =
                                    m.permissions.iter().filter(|p| !g.contains(p)).copied().collect();
                                if missing.is_empty() {
                                    (Status::Enabled, m.permissions.clone())
                                } else {
                                    (Status::NeedsApproval(missing), Vec::new())
                                }
                            }
                        };
                        reg.plugins.push(Plugin { dir, manifest: m, status, granted });
                    }
                    Err(e) => reg.broken.push(Broken { dir, error: format!("{e:#}") }),
                }
            }
        }
        reg
    }

    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.manifest.id == id)
    }

    fn usable(&self, game: Option<GameId>) -> impl Iterator<Item = &Plugin> {
        self.plugins.iter().filter(move |p| p.enabled() && p.manifest.supports(game))
    }

    /// The plugin that previews a file: one naming its extension, else one
    /// with the `*` fallback.
    pub fn preview_for(&self, name: &str, game: Option<GameId>) -> Option<&Plugin> {
        let specific = |fallback| {
            self.usable(game)
                .find(|p| p.manifest.preview.iter().any(|s| manifest::matches(&s.extensions, name, fallback)))
        };
        specific(false).or_else(|| specific(true))
    }

    /// The actions that apply to a file.
    pub fn actions_for(&self, name: &str, game: Option<GameId>) -> Vec<(&Plugin, &Action)> {
        self.usable(game)
            .flat_map(|p| p.manifest.actions.iter().map(move |a| (p, a)))
            .filter(|(_, a)| manifest::matches(&a.extensions, name, true))
            .collect()
    }

    /// The type name a plugin gives a file, with its icon group.
    pub fn file_type(&self, name: &str, game: Option<GameId>) -> Option<(&str, &str)> {
        self.usable(game)
            .flat_map(|p| &p.manifest.file_types)
            .find(|t| manifest::matches(&t.extensions, name, false))
            .map(|t| (t.name.as_str(), t.category.as_str()))
    }
}

/// Parses granted permissions as stored in the settings (`read-game+…`);
/// unknown names are dropped.
pub fn parse_grants(text: &str) -> Vec<Permission> {
    text.split('+').filter_map(|s| Permission::parse(s.trim())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(root: &Path, folder: &str, toml: &str) {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), toml).unwrap();
    }

    fn manifest(id: &str, extra: &str) -> String {
        format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"1\"\napi = \"{}\"\n{extra}", manifest::API_VERSION)
    }

    #[test]
    fn discovers_and_applies_grants() {
        let tmp = std::env::temp_dir().join(format!("openrage-plugins-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        write_plugin(&a, "one", &manifest("one", "[[preview]]\nextensions = [\"*\"]"));
        write_plugin(
            &a,
            "two",
            &manifest(
                "two",
                "permissions = [\"read-game\"]\n[[preview]]\nextensions = [\"xyz\"]\n\
                 [[actions]]\nid = \"go\"\ntitle = \"Go\"\nextensions = [\"xyz\"]",
            ),
        );
        write_plugin(&a, "broken", "id = 1");
        write_plugin(&b, "one-again", &manifest("one", ""));
        std::fs::create_dir_all(a.join("not-a-plugin")).unwrap();

        let mut grants = HashMap::new();
        grants.insert("one".to_owned(), Vec::new());
        let reg = Registry::discover(&[a.clone(), b.clone(), tmp.join("missing")], &grants);
        assert_eq!(reg.plugins.len(), 2);
        assert_eq!(reg.broken.len(), 2, "{:?}", reg.broken);
        assert_eq!(reg.get("one").unwrap().status, Status::Enabled);
        assert_eq!(reg.get("two").unwrap().status, Status::Disabled);
        assert_eq!(reg.preview_for("a.xyz", None).unwrap().manifest.id, "one");
        assert!(reg.actions_for("a.xyz", None).is_empty());

        grants.insert("two".to_owned(), Vec::new());
        let reg = Registry::discover(std::slice::from_ref(&a), &grants);
        assert_eq!(reg.get("two").unwrap().status, Status::NeedsApproval(vec![Permission::ReadGame]));

        grants.insert("two".to_owned(), parse_grants("read-game+bogus"));
        let reg = Registry::discover(&[a], &grants);
        let two = reg.get("two").unwrap();
        assert!(two.enabled());
        assert_eq!(two.granted, [Permission::ReadGame]);
        assert_eq!(reg.preview_for("a.XYZ", None).unwrap().manifest.id, "two");
        assert_eq!(reg.preview_for("a.bin", None).unwrap().manifest.id, "one");
        assert_eq!(reg.actions_for("dir/a.xyz", None).len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
