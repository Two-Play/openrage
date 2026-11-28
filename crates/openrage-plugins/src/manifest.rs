//! `plugin.toml`: what a plugin is and what it adds to OpenRAGE.

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The plugin API version this OpenRAGE implements (`api` in plugin.toml).
/// Before 1.0 every minor version may break plugins, so it must match exactly.
pub const API_VERSION: &str = "0.3";

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
    /// Tools with a window of their own (fields the user fills in, a result
    /// the plugin computes), in the explorer's Tools menu.
    #[serde(default)]
    pub tools: Vec<Tool>,
}

/// A tool: OpenRAGE draws its window from the fields and shows what the
/// plugin's `preview` returns for `tool:<id>` with the fields' values.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Tool {
    /// Unique within the plugin; the plugin is asked for `tool:<id>`.
    pub id: String,
    /// Window title and menu text, e.g. "Number converter".
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub fields: Vec<Field>,
}

/// An input of a tool.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Field {
    /// The key the plugin receives the value under.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub kind: FieldKind,
    /// Starting value: text, one of `choices`, or `true`/`false` for a check box.
    #[serde(default)]
    pub default: String,
    /// The values of a `choice` field.
    #[serde(default)]
    pub choices: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldKind {
    /// A line of text.
    #[default]
    Text,
    /// One of `choices`.
    Choice,
    /// A check box: `true` or `false`.
    Check,
}

impl Field {
    /// The value the field starts with.
    pub fn initial(&self) -> String {
        match self.kind {
            FieldKind::Choice if self.default.is_empty() => self.choices.first().cloned().unwrap_or_default(),
            FieldKind::Check if self.default.is_empty() => "false".to_owned(),
            _ => self.default.clone(),
        }
    }
}

/// The `vpath` a tool is asked for in `preview`.
pub fn tool_vpath(tool: &str) -> String {
    format!("tool:{tool}")
}

/// The fields' values as the `data` of a tool's `preview` call: one
/// `id=value` line each, with `\` and line breaks escaped (`\\`, `\n`).
pub fn encode_tool_values(values: &[(String, String)]) -> Vec<u8> {
    let mut out = String::new();
    for (id, value) in values {
        let value = value.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "");
        out.push_str(&format!("{id}={value}\n"));
    }
    out.into_bytes()
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

/// Most fields one tool may declare.
const MAX_TOOL_FIELDS: usize = 32;

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

    fn check_identity(&self) -> Result<()> {
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
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        self.check_identity()?;
        for t in &self.file_types {
            check_file_type(t)?;
        }
        for p in &self.preview {
            check_extensions(&p.extensions, true)?;
        }
        for (i, a) in self.actions.iter().enumerate() {
            self.check_action(i, a)?;
        }
        for (i, t) in self.tools.iter().enumerate() {
            check_tool(&self.tools[..i], t)?;
        }
        Ok(())
    }

    fn check_action(&self, i: usize, a: &Action) -> Result<()> {
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
        Ok(())
    }

    pub fn tool(&self, id: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.id == id)
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

fn check_file_type(t: &FileType) -> Result<()> {
    if !CATEGORIES.contains(&t.category.as_str()) {
        bail!("file type {:?}: unknown category {:?} (one of {})", t.name, t.category, CATEGORIES.join(", "));
    }
    check_extensions(&t.extensions, false)
}

/// Checks a tool; `earlier` are the tools declared before it.
fn check_tool(earlier: &[Tool], t: &Tool) -> Result<()> {
    if !valid_id(&t.id) {
        bail!("tool id {:?}: use lowercase letters, digits and '-'", t.id);
    }
    if earlier.iter().any(|u| u.id == t.id) {
        bail!("tool id {:?} is used twice", t.id);
    }
    if t.title.trim().is_empty() {
        bail!("tool {:?} has no title", t.id);
    }
    if t.fields.len() > MAX_TOOL_FIELDS {
        bail!("tool {:?} has more than {MAX_TOOL_FIELDS} fields", t.id);
    }
    for (j, f) in t.fields.iter().enumerate() {
        check_field(&t.id, &t.fields[..j], f)?;
    }
    Ok(())
}

/// Checks a tool field; `earlier` are the fields declared before it.
fn check_field(tool_id: &str, earlier: &[Field], f: &Field) -> Result<()> {
    if !valid_id(&f.id) {
        bail!("tool {:?}: field id {:?}: use lowercase letters, digits and '-'", tool_id, f.id);
    }
    if earlier.iter().any(|g| g.id == f.id) {
        bail!("tool {:?}: field id {:?} is used twice", tool_id, f.id);
    }
    check_field_choices(tool_id, f)
}

fn check_field_choices(tool_id: &str, f: &Field) -> Result<()> {
    match f.kind {
        FieldKind::Choice if f.choices.is_empty() => {
            bail!("tool {:?}: choice field {:?} has no choices", tool_id, f.id)
        }
        FieldKind::Choice if !f.default.is_empty() && !f.choices.contains(&f.default) => {
            bail!("tool {:?}: field {:?}: default {:?} is not one of its choices", tool_id, f.id, f.default)
        }
        FieldKind::Check if !matches!(f.default.as_str(), "" | "true" | "false") => {
            bail!("tool {:?}: check box {:?}: default must be true or false", tool_id, f.id)
        }
        FieldKind::Text | FieldKind::Check if !f.choices.is_empty() => {
            bail!("tool {:?}: field {:?}: only choice fields have choices", tool_id, f.id)
        }
        _ => Ok(()),
    }
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
pub fn safe_relative(rel: &str) -> Option<std::path::PathBuf> {
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
        api = "0.3"
    "#;

    #[test]
    fn parses_a_full_manifest() {
        let m = Manifest::parse(
            r#"
            id = "obj-tools"
            name = "OBJ tools"
            version = "0.2.0"
            api = "0.3"
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
            ("api = \"0.3\"", "api = \"9.0\""),
            ("version = \"1.0.0\"", "version = \"1.0.0\"\nwasm = \"../x.wasm\""),
            ("version = \"1.0.0\"", "version = \"1.0.0\"\npermissions = [\"network\"]"),
            ("version = \"1.0.0\"", "version = \"1.0.0\"\nunknown = 1"),
            ("api = \"0.3\"", "api = \"0.3\"\n[[preview]]\nextensions = [\".ydr\"]"),
            ("api = \"0.3\"", "api = \"0.3\"\n[[file-types]]\nextensions = [\"*\"]\nname = \"x\""),
            (
                "api = \"0.3\"",
                "api = \"0.3\"\n[[actions]]\nid = \"a\"\ntitle = \"A\"\nkind = \"modify\"\nextensions = [\"x\"]",
            ),
            (
                "api = \"0.3\"",
                "api = \"0.3\"\n[[actions]]\nid = \"a\"\ntitle = \"A\"\nextensions = [\"x\"]\npick = [\"*\"]",
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

    #[test]
    fn tools_and_their_fields() {
        let tool = |fields: &str| {
            Manifest::parse(&format!("{MINIMAL}\n[[tools]]\nid = \"conv\"\ntitle = \"Converter\"\n{fields}"))
        };
        let m = tool(
            "[[tools.fields]]\nid = \"value\"\nlabel = \"Value\"\n\
             [[tools.fields]]\nid = \"as\"\nlabel = \"As\"\nkind = \"choice\"\nchoices = [\"a\", \"b\"]\n\
             [[tools.fields]]\nid = \"signed\"\nlabel = \"Signed\"\nkind = \"check\"",
        )
        .unwrap();
        let t = m.tool("conv").unwrap();
        let initial: Vec<String> = t.fields.iter().map(Field::initial).collect();
        assert_eq!(initial, ["", "a", "false"], "a choice starts at its first value, a check box off");
        for (bad, why) in [
            ("[[tools.fields]]\nid = \"as\"\nlabel = \"As\"\nkind = \"choice\"", "no choices"),
            (
                "[[tools.fields]]\nid = \"as\"\nlabel = \"As\"\nkind = \"choice\"\nchoices = [\"a\"]\ndefault = \"z\"",
                "not one of",
            ),
            ("[[tools.fields]]\nid = \"x\"\nlabel = \"X\"\nkind = \"check\"\ndefault = \"yes\"", "true or false"),
            ("[[tools.fields]]\nid = \"x\"\nlabel = \"X\"\n[[tools.fields]]\nid = \"x\"\nlabel = \"Y\"", "used twice"),
            ("[[tools.fields]]\nid = \"Bad Id\"\nlabel = \"X\"", "field id"),
        ] {
            let e = tool(bad).unwrap_err();
            assert!(format!("{e:#}").contains(why), "{why}: {e:#}");
        }
        // The values reach the plugin as id=value lines, line breaks escaped.
        let data = encode_tool_values(&[("value".into(), "a\nb\\c".into()), ("as".into(), "a".into())]);
        assert_eq!(String::from_utf8(data).unwrap(), "value=a\\nb\\\\c\nas=a\n");
    }
}
