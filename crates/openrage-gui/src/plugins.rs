//! Plugins in the explorer: type names and icons for the file list, previews,
//! context-menu actions and the list on the settings page.
//!
//! The registry is shared by the UI thread and the preview workers; plugins
//! are compiled once, on first use.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use openrage_app::config::Config;
use openrage_plugins::{Component, GameFiles, GameId, Host, Limits, LogLine, NoGame, Plugin, Registry, Status, View};
use rage_crypto::GtaVKeys;

struct State {
    registry: Registry,
    host: OnceLock<Result<Host, String>>,
    components: Mutex<HashMap<String, Component>>,
}

static STATE: RwLock<Option<Arc<State>>> = RwLock::new(None);
/// The open game: plugins can be limited to some games.
static GAME: RwLock<Option<GameId>> = RwLock::new(None);

/// Tells the plugins which game is open.
pub fn set_game(dir: Option<&Path>) {
    if let Ok(mut g) = GAME.write() {
        *g = dir.map(openrage_app::plugins::game_id);
    }
}

fn open_game() -> Option<GameId> {
    GAME.read().ok().and_then(|g| *g)
}

fn state() -> Option<Arc<State>> {
    STATE.read().ok()?.clone()
}

/// (Re)reads the plugin folders and the user's choices.
pub fn reload(cfg: &Config) {
    let state = State {
        registry: openrage_app::plugins::registry(cfg),
        host: OnceLock::new(),
        components: Mutex::new(HashMap::new()),
    };
    if let Ok(mut s) = STATE.write() {
        *s = Some(Arc::new(state));
    }
}

/// What the plugins need to know about the open game.
#[derive(Clone)]
pub struct GameContext {
    pub dir: Option<PathBuf>,
    pub keys: Option<Arc<GtaVKeys>>,
}

impl GameContext {
    fn files(&self) -> Arc<dyn GameFiles> {
        match &self.dir {
            Some(dir) => Arc::new(openrage_app::plugins::GameDir::new(dir, self.keys.clone())),
            None => Arc::new(NoGame),
        }
    }
}

/// The type name and icon group a plugin gives a file the explorer does not
/// know itself.
pub fn file_type(name: &str) -> Option<(String, String)> {
    let state = state()?;
    state.registry.file_type(name, open_game()).map(|(n, c)| (n.to_owned(), c.to_owned()))
}

/// A context-menu entry of a plugin.
#[derive(Clone)]
pub struct ActionRef {
    pub plugin_id: String,
    pub plugin_name: String,
    pub action_id: String,
    pub title: String,
    /// Changes game files (needs editing unlocked).
    pub modify: bool,
    /// Extensions of files to ask the user for.
    pub pick: Vec<String>,
    pub pick_title: String,
}

/// The actions plugins offer for a file, or for a folder or archive (batch
/// actions only).
fn actions(name: &str, folder: bool) -> Vec<ActionRef> {
    let Some(state) = state() else { return Vec::new() };
    let game = open_game();
    let found: Vec<(&Plugin, &openrage_plugins::Action)> = if folder {
        state
            .registry
            .plugins
            .iter()
            .filter(|p| p.enabled() && p.manifest.supports(game))
            .flat_map(|p| p.manifest.actions.iter().filter(|a| a.batch).map(move |a| (p, a)))
            .collect()
    } else {
        state.registry.actions_for(name, game)
    };
    found
        .into_iter()
        .map(|(p, a)| ActionRef {
            plugin_id: p.manifest.id.clone(),
            plugin_name: p.manifest.name.clone(),
            action_id: a.id.clone(),
            title: a.title.clone(),
            modify: a.kind == openrage_plugins::ActionKind::Modify,
            pick: a.pick.clone(),
            pick_title: a.pick_title.clone(),
        })
        .collect()
}

/// Titles of the actions for a file or folder, in the order [`action_at`]
/// takes their index.
pub fn action_titles(name: &str, folder: bool) -> Vec<String> {
    actions(name, folder).into_iter().map(|a| a.title).collect()
}

pub fn action_at(name: &str, folder: bool, index: usize) -> Option<ActionRef> {
    actions(name, folder).into_iter().nth(index)
}

/// The plugin that previews this file, and whether it names the file's
/// extension (`false`: it is a fallback for every file, `*`).
pub fn previews(name: &str) -> Option<(String, bool)> {
    let state = state()?;
    let plugin = state.registry.preview_for(name, open_game())?;
    let specific =
        plugin.manifest.preview.iter().any(|p| openrage_plugins::manifest::matches(&p.extensions, name, false));
    Some((plugin.manifest.id.clone(), specific))
}

fn with_plugin<T>(id: &str, f: impl FnOnce(&Host, &Plugin, &Component) -> Result<T, String>) -> Result<T, String> {
    let state = state().ok_or("plugins are not loaded")?;
    let plugin = state.registry.get(id).ok_or_else(|| format!("plugin {id} is gone"))?;
    let host = state
        .host
        .get_or_init(|| Host::new(Limits::default()).map_err(|e| format!("cannot start plugins: {e:#}")))
        .as_ref()
        .map_err(Clone::clone)?;
    let component = {
        let mut cache = state.components.lock().map_err(|_| "plugin cache poisoned")?;
        match cache.get(id) {
            Some(c) => c.clone(),
            None => {
                let c = host.compile(plugin).map_err(|e| format!("{e:#}"))?;
                cache.insert(id.to_owned(), c.clone());
                c
            }
        }
    };
    f(host, plugin, &component)
}

/// A plugin's preview, ready for the preview pane: info lines, text and an
/// optional image.
pub struct PreviewResult {
    pub info: Vec<(String, String)>,
    pub text: String,
    pub image: Option<(u32, u32, Vec<u8>)>,
}

/// Runs a plugin's preview (call it from a worker thread).
pub fn preview(id: &str, game: &GameContext, vpath: &str, data: &[u8]) -> PreviewResult {
    let result = with_plugin(id, |host, plugin, component| {
        let outcome = host.preview(plugin, component, game.files(), vpath, data);
        let mut info = vec![("Plugin".to_owned(), plugin.manifest.name.clone())];
        match outcome.value {
            Ok(p) => {
                info.extend(p.info.into_iter().map(|i| (i.key, i.value)));
                info.extend(log_lines(&outcome.log));
                let (text, image) = match p.view {
                    None => (String::new(), None),
                    Some(View::Text(t)) => (t, None),
                    Some(View::Tree(nodes)) => (tree_text(&nodes), None),
                    Some(View::Table(t)) => (table_text(&t.columns, &t.rows), None),
                    Some(View::Image(img)) => (String::new(), Some((img.width, img.height, img.rgba))),
                };
                Ok(PreviewResult { info, text, image })
            }
            Err(e) => {
                info.push(("Plugin error".to_owned(), format!("{e:#}")));
                info.extend(log_lines(&outcome.log));
                Ok(PreviewResult { info, text: String::new(), image: None })
            }
        }
    });
    result.unwrap_or_else(|e| PreviewResult {
        info: vec![("Plugin error".to_owned(), e)],
        text: String::new(),
        image: None,
    })
}

/// The last lines a plugin logged, as notes for the info panel.
fn log_lines(log: &[LogLine]) -> Vec<(String, String)> {
    let skip = log.len().saturating_sub(5);
    log[skip..]
        .iter()
        .map(|l| (String::new(), format!("{}: {}", openrage_plugins::level_name(l.level), l.message)))
        .collect()
}

/// A tree as indented lines.
fn tree_text(nodes: &[openrage_plugins::TreeNode]) -> String {
    let mut depth = vec![0usize; nodes.len()];
    let mut out = String::new();
    for (i, n) in nodes.iter().enumerate() {
        depth[i] = n.parent.map_or(0, |p| depth.get(p as usize).copied().unwrap_or(0) + 1);
        out.push_str(&"    ".repeat(depth[i]));
        out.push_str(&n.label);
        if !n.value.is_empty() {
            out.push_str(": ");
            out.push_str(&n.value);
        }
        out.push('\n');
    }
    out
}

/// A table as aligned columns (the preview uses a monospaced font).
fn table_text(columns: &[String], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count()).min(60);
        }
    }
    let line = |cells: &[String]| {
        let mut s = String::new();
        for (cell, w) in cells.iter().zip(&widths) {
            let cell: String = cell.chars().take(60).collect();
            s.push_str(&format!("{cell:<w$}  "));
        }
        s.trim_end().to_owned() + "\n"
    };
    let mut out = line(columns);
    out.push_str(&line(&widths.iter().map(|w| "─".repeat(*w)).collect::<Vec<_>>()));
    for row in rows {
        out.push_str(&line(row));
    }
    out
}

/// Runs an action on a file, folder or archive (call it from a worker
/// thread). `progress` gets (done, total, path) for batch runs and cancels
/// by returning `false`.
pub fn run(
    action: &ActionRef,
    game: &GameContext,
    vpath: &str,
    picked: &[openrage_app::plugins::Picked],
    progress: &mut dyn FnMut(usize, usize, &str) -> bool,
) -> Result<openrage_app::plugins::RunResult, String> {
    let files = game.files();
    with_plugin(&action.plugin_id, |host, plugin, component| {
        openrage_app::plugins::run_action(
            host,
            plugin,
            component,
            files,
            &action.action_id,
            &[vpath.to_owned()],
            picked,
            progress,
        )
        .map_err(|e| format!("{e:#}"))
    })
}

/// One plugin as the settings page lists it.
pub struct Row {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub permissions: String,
    pub enabled: bool,
    pub broken: bool,
}

/// Every installed plugin, and the folders that could not be read.
pub fn rows() -> Vec<Row> {
    let Some(state) = state() else { return Vec::new() };
    let mut rows: Vec<Row> = state
        .registry
        .plugins
        .iter()
        .map(|p| {
            let m = &p.manifest;
            let permissions = if m.permissions.is_empty() {
                "Needs no permissions: sees only the files you open with it.".to_owned()
            } else {
                let list: Vec<String> = m.permissions.iter().map(|p| format!("{p} ({})", p.description())).collect();
                format!("Asks to {}. Enabling it grants this.", list.join("; "))
            };
            let mut description = m.description.clone();
            let games: Vec<_> = m.games.iter().map(|g| g.as_str()).collect();
            let mut meta = Vec::new();
            if !m.authors.is_empty() {
                meta.push(format!("by {}", m.authors.join(", ")));
            }
            if !m.license.is_empty() {
                meta.push(m.license.clone());
            }
            if !games.is_empty() {
                meta.push(format!("for {}", games.join(", ")));
            }
            if !meta.is_empty() {
                if !description.is_empty() {
                    description.push('\n');
                }
                description.push_str(&meta.join(" · "));
            }
            Row {
                id: m.id.clone(),
                title: format!("{} {}", m.name, m.version),
                description,
                status: match &p.status {
                    Status::Enabled => "On".to_owned(),
                    Status::Disabled => "Off".to_owned(),
                    Status::NeedsApproval(missing) => {
                        let names: Vec<_> = missing.iter().map(|p| p.as_str()).collect();
                        format!("Off: this version also asks for {}", names.join(", "))
                    }
                },
                permissions,
                enabled: p.enabled(),
                broken: false,
            }
        })
        .collect();
    rows.extend(state.registry.broken.iter().map(|b| Row {
        id: String::new(),
        title: b.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        description: b.dir.display().to_string(),
        status: "Cannot be loaded".to_owned(),
        permissions: b.error.clone(),
        enabled: false,
        broken: true,
    }));
    rows
}

/// Turns a plugin on (granting what it asks for) or off, and saves the choice.
pub fn set_enabled(cfg: &mut Config, id: &str, on: bool) -> anyhow::Result<()> {
    if on {
        let permissions = state()
            .and_then(|s| s.registry.get(id).map(|p| p.manifest.permissions.clone()))
            .ok_or_else(|| anyhow::anyhow!("plugin {id} is not installed"))?;
        openrage_app::plugins::enable(cfg, id, &permissions);
    } else {
        openrage_app::plugins::disable(cfg, id);
    }
    cfg.save()?;
    reload(cfg);
    Ok(())
}

/// The user's plugin folder.
pub fn user_folder() -> Option<PathBuf> {
    openrage_plugins::user_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_trees_and_tables_as_text() {
        let node = |parent, label: &str, value: &str| openrage_plugins::TreeNode {
            parent,
            label: label.into(),
            value: value.into(),
        };
        let tree = tree_text(&[node(None, "root", ""), node(Some(0), "a", "1"), node(Some(1), "b", "")]);
        assert_eq!(tree, "root\n    a: 1\n        b\n");
        let table = table_text(&["Name".into(), "Size".into()], &[vec!["x.ydr".into(), "12".into()]]);
        assert_eq!(table, "Name   Size\n─────  ────\nx.ydr  12\n");
    }
}
