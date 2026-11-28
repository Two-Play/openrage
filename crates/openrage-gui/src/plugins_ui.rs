//! Plugins in the UI: installing packages, previews and actions.

use crate::*;

/// Checks a plugin package (in the background) and shows the install dialog.
pub(crate) fn offer_plugin_package(ui: &AppWindow, path: PathBuf) {
    ui.set_status(format!("Checking {}…", path.display()).into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let offer = plugins::open_package(&path);
        let _ = weak.upgrade_in_event_loop(move |ui| match offer {
            Ok(o) => {
                ui.set_plugin_pkg_title(o.title.into());
                ui.set_plugin_pkg_subtitle(o.subtitle.into());
                ui.set_plugin_pkg_details(o.details.into());
                with_app(|a| a.plugin_package = Some(Arc::new(o.package)));
                ui.set_plugin_pkg_visible(true);
                ui.set_status(SharedString::new());
            }
            Err(e) => show_error(&ui, format!("Cannot install {}: {e}", path.display())),
        });
    });
}

/// Shows the installed plugins on the settings page.
pub(crate) fn refresh_plugin_rows(ui: &AppWindow) {
    let rows: Vec<PluginRow> = plugins::rows()
        .into_iter()
        .map(|r| PluginRow {
            id: r.id.into(),
            title: r.title.into(),
            description: r.description.into(),
            status: r.status.into(),
            permissions: r.permissions.into(),
            enabled: r.enabled,
            broken: r.broken,
            removable: r.removable,
        })
        .collect();
    ui.set_plugins(ModelRc::new(VecModel::from(rows)));
    let folder = plugins::user_folder().map(|d| format!("Plugin folder: {}", d.display())).unwrap_or_default();
    ui.set_plugin_folder(folder.into());
}

/// Plugins were turned on or off or reloaded: their type names, menu entries
/// and previews change.
pub(crate) fn after_plugins_change(ui: &AppWindow) {
    refresh_plugin_rows(ui);
    refresh_tool_menu(ui);
    if with_app(|a| a.listing.is_some()) {
        refresh_files(ui, None);
    }
}

/// Larger files are not handed to plugins for a preview.
const PREVIEW_MAX: u64 = 256 << 20;

/// A plugin's preview of a file, if it has one. A fallback plugin (`*`) is
/// only asked about binary files: text is shown as text.
pub(crate) fn plugin_preview(
    id: &str,
    specific: bool,
    game: &plugins::GameContext,
    vpath: &str,
) -> Option<plugins::PreviewResult> {
    let dir = game.dir.as_deref()?;
    let node = rage_mods::resolve(dir, vpath, game.keys.clone()).ok()?.0;
    let data = preview_bytes(node)?;
    if !specific && rage_core::text::decode_text(&data, 4096).is_some() {
        return None;
    }
    Some(plugins::preview(id, game, vpath, &data))
}

/// The file's bytes, unless it is too large to hand to a plugin.
fn preview_bytes(node: Node) -> Option<Vec<u8>> {
    match node {
        Node::PlainFile(p) => read_plain_for_preview(&p),
        Node::ArchiveFile(rpf, id) if rpf.entry(id).size() <= PREVIEW_MAX => rpf.read(id).ok(),
        Node::Rpf8File(rpf, i) => read_rpf8_for_preview(&rpf, i),
        Node::Gta4File(a, id) if a.entry(id).size() <= PREVIEW_MAX => a.read(id).ok(),
        Node::Rpf9File(rpf, i) => read_rpf9_for_preview(&rpf, i),
        _ => None,
    }
}

fn read_plain_for_preview(path: &Path) -> Option<Vec<u8>> {
    if std::fs::metadata(path).ok()?.len() > PREVIEW_MAX {
        return None;
    }
    std::fs::read(path).ok()
}

fn read_rpf8_for_preview(rpf: &rage_rpf::Rpf8, i: usize) -> Option<Vec<u8>> {
    let entry = rpf.entries()?.get(i)?;
    if entry.size() > PREVIEW_MAX {
        return None;
    }
    rpf.read(entry).ok()
}

fn read_rpf9_for_preview(rpf: &rage_rpf::Rpf9, i: usize) -> Option<Vec<u8>> {
    if rpf.entries().ok()?.get(i)?.size > PREVIEW_MAX {
        return None;
    }
    rpf.read_at(i).ok()
}

/// Runs a plugin's action on a file, folder or archive: asks for the files
/// it wants from disk, shows progress (cancellable), saves what it produces
/// (one file with a save dialog, several into a folder) and applies its
/// changes to the mods folder as one step History can undo.
pub(crate) fn run_plugin_action(ui: &AppWindow, action: plugins::ActionRef, vpath: String) {
    if action.modify && !ui.get_edit_mode() {
        ui.set_status(
            format!(
                "{} changes game files: unlock editing first (Read-only button in the toolbar).",
                action.title.trim_end_matches('…')
            )
            .into(),
        );
        return;
    }
    if with_app(|a| a.busy) {
        return;
    }
    let (game_dir, keys) = with_app(|a| (a.game_dir.clone(), a.keys.clone()));
    let game = plugins::GameContext { dir: game_dir.clone(), keys: keys.clone() };
    let current = with_app(|a| a.listing.as_ref().map(|l| l.vpath.clone())).unwrap_or_default();
    let cancel = with_app(|a| {
        a.cancel.store(false, Ordering::Relaxed);
        Arc::clone(&a.cancel)
    });
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let title = action.title.trim_end_matches('…').to_owned();
        let Some(picked) = pick_plugin_files(&action, &title, &weak) else { return };
        let start = format!("{title}…");
        let _ = weak.upgrade_in_event_loop(move |ui| set_busy_cancellable(&ui, start));
        let mut last = Instant::now();
        let result = plugins::run(&action, &game, &vpath, &picked, &mut |done, total, path| {
            if last.elapsed() > Duration::from_millis(100) {
                last = Instant::now();
                let msg = format!("{title}: {done} of {total} — {path}");
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
            }
            !cancel.load(Ordering::Relaxed)
        });
        let (msg, mods_changed) = match result {
            Err(e) => (format!("{}: {e}", action.plugin_name), false),
            Ok(r) if r.cancelled => ("Cancelled; nothing was saved or changed.".to_owned(), false),
            Ok(r) => finish_plugin_run(&action, &title, &r, game_dir.as_deref(), &keys),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if mods_changed {
                after_mods_change(&ui, &current);
            }
            set_busy(&ui, false, msg);
        });
    });
}

/// Asks for the files the action wants from disk (none: an empty list); `None` when
/// the user cancelled the dialog or a file could not be read.
fn pick_plugin_files(
    action: &plugins::ActionRef,
    title: &str,
    weak: &Weak<AppWindow>,
) -> Option<Vec<openrage_app::plugins::Picked>> {
    let mut picked = Vec::new();
    if action.pick.is_empty() {
        return Some(picked);
    }
    let exts: Vec<&str> = action.pick.iter().map(String::as_str).collect();
    let dialog_title = if action.pick_title.is_empty() { title.to_owned() } else { action.pick_title.clone() };
    let paths = rfd::FileDialog::new().set_title(dialog_title).add_filter(exts.join(", "), &exts).pick_files()?;
    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::read(&path) {
            Ok(data) => picked.push(openrage_app::plugins::Picked { name, data }),
            Err(e) => {
                let msg = format!("Cannot read {}: {e}", path.display());
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
                return None;
            }
        }
    }
    Some(picked)
}

/// Saves the outputs and applies the writes of a finished run; returns the status
/// text and whether the mods folder changed.
fn finish_plugin_run(
    action: &plugins::ActionRef,
    title: &str,
    r: &openrage_app::plugins::RunResult,
    game_dir: Option<&Path>,
    keys: &Option<Arc<GtaVKeys>>,
) -> (String, bool) {
    let mut mods_changed = false;
    let mut parts = Vec::new();
    if !r.outputs.is_empty() {
        parts.push(save_plugin_outputs(title, &r.outputs));
    }
    if !r.writes.is_empty() {
        let description = format!("{}: {title}", action.plugin_name);
        let group = openrage_app::plugins::group_name(&description);
        match game_dir
            .map(|dir| openrage_app::plugins::apply_writes(dir, keys.clone(), &r.writes, &description, &group))
        {
            Some(Ok(done)) => {
                mods_changed = true;
                parts.push(format!("Changed {} (undo in History)", done.join(", ")));
            }
            Some(Err(e)) => parts.push(format!("Changing files failed: {e:#}")),
            None => parts.push("No game is open to change".to_owned()),
        }
    }
    if let Some((path, error)) = r.failures.first() {
        parts.push(format!("{} of {} files failed ({path}: {error})", r.failures.len(), r.files));
    }
    parts.retain(|p| !p.is_empty());
    if parts.is_empty() && r.outputs.is_empty() {
        let last = r.log.last().map(|l| format!(": {}", l.message)).unwrap_or_default();
        parts.push(format!("{}: nothing to save or change{last}", action.plugin_name));
    }
    (parts.join(" · "), mods_changed)
}

/// Saves the files an action produced; returns the status text ("" if the
/// user cancelled the dialog).
pub(crate) fn save_plugin_outputs(title: &str, outputs: &[openrage_plugins::OutputFile]) -> String {
    let write = |dir: &Path, o: &openrage_plugins::OutputFile| -> anyhow::Result<PathBuf> {
        // The host has checked that the names stay inside the folder.
        let rel = vfs::safe_relative_path(&o.name).ok_or_else(|| anyhow::anyhow!("bad file name {}", o.name))?;
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &o.data)?;
        Ok(path)
    };
    match outputs {
        [] => String::new(),
        [one] => {
            let name = one.name.rsplit('/').next().unwrap_or(&one.name);
            let Some(path) = rfd::FileDialog::new().set_title(title).set_file_name(name).save_file() else {
                return String::new();
            };
            match std::fs::write(&path, &one.data) {
                Ok(()) => format!("Saved {}", path.display()),
                Err(e) => format!("Saving failed: {e}"),
            }
        }
        many => {
            let dialog = format!("{title}: choose a folder for {} files", many.len());
            let Some(dir) = rfd::FileDialog::new().set_title(dialog).pick_folder() else { return String::new() };
            match many.iter().map(|o| write(&dir, o)).collect::<anyhow::Result<Vec<_>>>() {
                Ok(paths) => format!("Saved {} files in {}", paths.len(), dir.display()),
                Err(e) => format!("Saving failed: {e:#}"),
            }
        }
    }
}
