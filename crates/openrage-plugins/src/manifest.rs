//! `plugin.toml`: what a plugin is and what it adds to OpenRAGE.

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The plugin API version this OpenRAGE implements (`api` in plugin.toml).
/// Before 1.0 every minor version may break plugins, so it must match exactly.
pub const API_VERSION: &str = "0.2";

/// The manifest's file name in a plugin folder.
pub const FILE_NAME: &str = "plugin.toml";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Manifest {
    /// Unique name: lowercase letters, digits and `-`, e.g. `strings`.
    pub id: String,
    /// Display name.
    pub name: String,
    pub version: String,
    /// The plugin API version it was built for.
    pub api: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    /// SPDX licence expression of the plugin, e.g. `GPL-3.0-or-later`.
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub homepage: String,
    /// The compiled component, relative to the plugin folder.
    #[serde(default = "default_wasm")]
    pub wasm: String,
    /// The games it works with; empty means all.
    #[serde(default)]
    pub games: Vec<GameId>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    /// File types it adds (names and icons in the file list).
    #[serde(default)]
    pub file_types: Vec<FileType>,
    /// File types it can preview.
    #[serde(default)]
    pub preview: Vec<PreviewSpec>,
    /// Commands it adds (context menu, `openrage plugin run`).
    #[serde(default)]
    pub actions: Vec<Action>,
}

fn default_wasm() -> String {
    "plugin.wasm".to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameId {
    Gta5,
    Rdr2,
    Gta4,
}

impl GameId {
    pub fn as_str(self) -> &'static str {
        match self {
            GameId::Gta5 => "gta5",
            GameId::Rdr2 => "rdr2",
            GameId::Gta4 => "gta4",
        }
    }
}

/// What a plugin may do beyond looking at the files the user hands it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Read any file of the game (`read-file`, `list-dir`).
    ReadGame,
    /// Change files of the game through the `mods` folder (`write-mod-file`).
    WriteMods,
}

impl Permission {
    pub const ALL: [Permission; 2] = [Permission::ReadGame, Permission::WriteMods];

    pub fn as_str(self) -> &'static str {
        match self {
            Permission::ReadGame => "read-game",
            Permission::WriteMods => "write-mods",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == s)
    }

    /// What granting it means, for the user.
    pub fn description(self) -> &'static str {
        match self {
            Permission::ReadGame => "read every file of the game, not only the ones you open with it",
            Permission::WriteMods => {
                "change game files: its changes go into the mods folder and can be undone in History"
            }
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FileType {
    /// Extensions without the dot, e.g. `["xyz"]`.
    pub extensions: Vec<String>,
    /// What the type is called in the file list, e.g. "Mission script".
    pub name: String,
    /// Icon group: one of [`CATEGORIES`].
    #[serde(default = "default_category")]
    pub category: String,
}

/// The icon groups of the file list.
pub const CATEGORIES: &[&str] =
    &["file", "texture", "model", "map", "text", "lang", "audio", "anim", "script", "package", "data"];

fn default_category() -> String {
    "file".to_owned()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PreviewSpec {
    /// Extensions without the dot; `*` makes it a fallback for files nothing
    /// else can show.
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Action {
    /// Passed to `run-action`; unique within the plugin.
    pub id: String,
    /// Menu text, e.g. "Export as OBJ…".
    pub title: String,
    #[serde(default)]
    pub kind: ActionKind,
    /// The files it applies to; `*` for every file.
    pub extensions: Vec<String>,
    /// Also offered on folders and archives: runs once for every matching
    /// file inside (with progress, cancellable).
    #[serde(default)]
    pub batch: bool,
    /// Asks the user for files from disk (these extensions) and hands them
    /// to the action too, e.g. a patch to apply.
    #[serde(default)]
    pub pick: Vec<String>,
    /// Title of the file dialog for `pick`.
    #[serde(default)]
    pub pick_title: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionKind {
    /// Turns the picked files into new files the user saves.
    #[default]
    Export,
    /// Anything else that produces files (reports, lists, …).
    Tool,
    /// Changes game files through the `mods` folder (`write-mod-file`);
    /// needs the `write-mods` permission.
    Modify,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(text)?;
        m.validate()?;
        Ok(m)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("in {}", path.display()))
    }

    fn validate(&self) -> Result<()> {
        if !valid_id(&self.id) {
            bail!("id {:?}: use lowercase letters, digits and '-' (at most 64)", self.id);
        }
        if self.name.trim().is_empty() {
            bail!("name is empty");
        }
        if self.api != API_VERSION {
            bail!("built for plugin API {}, this OpenRAGE has {API_VERSION}", self.api);
        }
        if safe_relative(&self.wasm).is_none() || !self.wasm.ends_with(".wasm") {
            bail!("wasm {:?}: must be a .wasm file inside the plugin folder", self.wasm);
        }
        for t in &self.file_types {
            if !CATEGORIES.contains(&t.category.as_str()) {
                bail!("file type {:?}: unknown category {:?} (one of {})", t.name, t.category, CATEGORIES.join(", "));
            }
            check_extensions(&t.extensions, false)?;
        }
        for p in &self.preview {
            check_extensions(&p.extensions, true)?;
        }
        for (i, a) in self.actions.iter().enumerate() {
            if !valid_id(&a.id) {
                bail!("action id {:?}: use lowercase letters, digits and '-'", a.id);
            }
            if self.actions[..i].iter().any(|b| b.id == a.id) {
                bail!("action id {:?} is used twice", a.id);
            }
            check_extensions(&a.extensions, true)?;
            if !a.pick.is_empty() {
                check_extensions(&a.pick, false)?;
            }
            if a.kind == ActionKind::Modify && !self.permissions.contains(&Permission::WriteMods) {
                bail!("action {:?} is of kind modify, which needs the write-mods permission", a.id);
            }
        }
        Ok(())
    }

    /// Whether it works with `game` (`None`: no game open).
    pub fn supports(&self, game: Option<GameId>) -> bool {
        self.games.is_empty() || game.is_some_and(|g| self.games.contains(&g))
    }

    pub fn action(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.id == id)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn check_extensions(exts: &[String], wildcard: bool) -> Result<()> {
    if exts.is_empty() {
        bail!("extensions is empty");
    }
    for e in exts {
        let ok = (wildcard && e == "*") || (!e.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        if !ok {
            bail!("extension {e:?}: give it without the dot, e.g. \"xyz\"");
        }
    }
    Ok(())
}

/// A relative path that stays inside its folder.
pub(crate) fn safe_relative(rel: &str) -> Option<std::path::PathBuf> {
    let mut out = std::path::PathBuf::new();
    for part in rel.split(['/', '\\']) {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') || part.chars().any(char::is_control) {
            return None;
        }
        out.push(part);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// The lowercase extension of a file name ("" if it has none).
pub fn extension(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Whether `exts` (from a manifest) covers a file name. `fallback` decides
/// whether `*` counts.
pub fn matches(exts: &[String], name: &str, fallback: bool) -> bool {
    let ext = extension(name);
    exts.iter().any(|e| (fallback && e == "*") || (!ext.is_empty() && e.eq_ignore_ascii_case(&ext)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        id = "demo"
        name = "Demo"
        version = "1.0.0"
        api = "0.2"
    "#;

    #[test]
    fn parses_a_full_manifest() {
        let m = Manifest::parse(
            r#"
            id = "obj-tools"
            name = "OBJ tools"
            version = "0.2.0"
            api = "0.2"
            license = "MIT"
            games = ["gta5", "rdr2"]
            permissions = ["read-game"]

            [[file-types]]
            extensions = ["obj"]
            name = "Wavefront model"
            category = "model"

            [[preview]]
            extensions = ["obj"]

            [[actions]]
            id = "to-obj"
            title = "Export as OBJ…"
            extensions = ["ydr", "*"]
            "#,
        )
        .unwrap();
        assert_eq!(m.wasm, "plugin.wasm");
        assert_eq!(m.permissions, [Permission::ReadGame]);
        assert!(m.supports(Some(GameId::Gta5)));
        assert!(!m.supports(Some(GameId::Gta4)));
        assert!(!m.supports(None));
        assert_eq!(m.action("to-obj").unwrap().kind, ActionKind::Export);
        assert!(Manifest::parse(MINIMAL).unwrap().supports(None));
    }

    #[test]
    fn rejects_bad_manifests() {
        let bad = [
            ("id = \"demo\"", "id = \"Demo\""),
            ("api = \"0.2\"", "api = \"9.0\""),
            ("version = \"1.0.0\"", "version = \"1.0.0\"\nwasm = \"../x.wasm\""),
            ("version = \"1.0.0\"", "version = \"1.0.0\"\npermissions = [\"network\"]"),
            ("version = \"1.0.0\"", "version = \"1.0.0\"\nunknown = 1"),
            ("api = \"0.2\"", "api = \"0.2\"\n[[preview]]\nextensions = [\".ydr\"]"),
            ("api = \"0.2\"", "api = \"0.2\"\n[[file-types]]\nextensions = [\"*\"]\nname = \"x\""),
            (
                "api = \"0.2\"",
                "api = \"0.2\"\n[[actions]]\nid = \"a\"\ntitle = \"A\"\nkind = \"modify\"\nextensions = [\"x\"]",
            ),
            (
                "api = \"0.2\"",
                "api = \"0.2\"\n[[actions]]\nid = \"a\"\ntitle = \"A\"\nextensions = [\"x\"]\npick = [\"*\"]",
            ),
        ];
        for (from, to) in bad {
            let text = MINIMAL.replace(from, to);
            assert!(Manifest::parse(&text).is_err(), "accepted:\n{text}");
        }
    }

    #[test]
    fn matches_extensions() {
        let exts = vec!["ydr".to_owned(), "*".to_owned()];
        assert!(matches(&exts, "x64a.rpf/a/B.YDR", false));
        assert!(!matches(&exts, "a.ytd", false));
        assert!(matches(&exts, "a.ytd", true));
        assert_eq!(extension("dir.d/file"), "");
    }
}
