//! Changing archives: import, replace, delete, rename, undo, and editing textures.

use crate::*;

/// Exports the selected texture, or all textures of the open dictionary, as DDS or PNG.
pub(crate) fn export_textures(ui: &AppWindow, png: bool) {
    let Some((dict, selected, dict_name)) = with_app(|a| {
        let l = a.listing.as_ref()?;
        let Location::Textures { dict, .. } = &l.location else { return None };
        let name = l.vpath.rsplit('/').next().unwrap_or("textures").trim_end_matches(".ytd").to_owned();
        Some((Arc::clone(dict), a.selected, name))
    }) else {
        return;
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let title = if png { "Save PNG to folder" } else { "Export DDS to folder" };
        let Some(out) = rfd::FileDialog::new().set_title(title).pick_folder() else { return };
        // Exporting a whole dictionary goes into its own subfolder, like OpenIV.
        let (out, picked): (PathBuf, Vec<usize>) = match selected {
            Some(i) => (out, vec![i]),
            None => (out.join(&dict_name), (0..dict.textures.len()).collect()),
        };
        let mut written = 0;
        let mut problems = Vec::new();
        if let Err(e) = std::fs::create_dir_all(&out) {
            problems.push(e.to_string());
        }
        for i in picked {
            let t = &dict.textures[i];
            let Some(name) = vfs::safe_relative_path(&t.name) else {
                problems.push(format!("{}: unsafe name", t.name));
                continue;
            };
            let result = if png {
                match t.to_rgba() {
                    Some(rgba) => {
                        rage_formats::png(t.width, t.height, &rgba).map_err(|e| e.to_string()).and_then(|b| {
                            std::fs::write(out.join(&name).with_extension("png"), b).map_err(|e| e.to_string())
                        })
                    }
                    None => Err(format!("no PNG decoder for {}", t.format.name())),
                }
            } else {
                std::fs::write(out.join(&name).with_extension("dds"), rage_formats::dds::to_dds(t))
                    .map_err(|e| e.to_string())
            };
            match result {
                Ok(()) => written += 1,
                Err(e) => problems.push(format!("{}: {e}", t.name)),
            }
        }
        let mut msg = format!("Exported {written} texture(s) to {}", out.display());
        if let Some(first) = problems.first() {
            msg.push_str(&format!(" — {} problem(s), first: {first}", problems.len()));
        }
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

// ---------------------------------------------------------------------------
// Editing archives (always in the mods folder)

#[derive(Debug, Clone)]
pub(crate) enum Prompt {
    Rename { vpath: String },
    NewFolder { dir: String },
}

/// Virtual path and name of the selected entry in an archive listing.
pub(crate) fn selected_entry() -> Option<(String, String)> {
    with_app(|a| {
        let l = a.listing.as_ref()?;
        let item = &l.items[a.selected?];
        match l.location {
            Location::Archive(..) => {
                item.entry?;
            }
            // GTA IV archives: every listed entry (removed ones are shown greyed out).
            Location::Gta4(..) if item.mark != Some(Mark::Removed) => {}
            _ => return None,
        }
        Some((browser::join(&l.vpath, &item.name), item.name.clone()))
    })
}

/// The overlay folder of the current game: `mods`, or `update` for GTA IV.
pub(crate) fn overlay_name() -> &'static str {
    with_app(|a| a.game_dir.as_deref().map(rage_mods::overlay::folder_name)).unwrap_or("mods")
}

/// Whether changes go into the game's own files (GTA IV, when chosen).
pub(crate) fn writes_in_game() -> bool {
    with_app(|a| a.game_dir.clone()).is_some_and(|d| install::write_target(&d) == install::WriteTarget::Game)
}

/// Where changes go, for status texts: the overlay folder's name, or `game`.
pub(crate) fn write_folder() -> &'static str {
    if writes_in_game() { "game" } else { overlay_name() }
}

/// The FusionFix note, when changes go into the update folder without it.
pub(crate) fn overlay_note() -> Option<&'static str> {
    if writes_in_game() {
        return None;
    }
    with_app(|a| a.game_dir.clone()).and_then(|d| openrage_app::gta4::update_folder_note(&d))
}

/// Shows the current write target in every window.
pub(crate) fn show_write_target(ui: &AppWindow) {
    let game = with_app(|a| a.game_dir.clone());
    ui.set_can_write_in_game(game.as_deref().is_some_and(openrage_app::gta4::is_gta4_dir));
    ui.set_write_in_game(writes_in_game());
    editor::set_write_in_game(writes_in_game());
    history::refresh(ui);
}

/// Game files changed in place whose originals are kept.
pub(crate) fn kept_originals() -> Vec<String> {
    with_app(|a| a.game_dir.clone()).and_then(|d| install::originals(&d).ok()).unwrap_or_default()
}

/// Puts every kept original back (History → Restore game files).
pub(crate) fn restore_originals(ui: &AppWindow) {
    let Some(game_dir) = with_app(|a| a.game_dir.clone()) else { return };
    let mut done = 0;
    let mut failed = Vec::new();
    for rel in kept_originals() {
        match install::restore_original(&game_dir, &rel) {
            Ok(_) => done += 1,
            Err(e) => failed.push(format!("{rel}: {e:#}")),
        }
    }
    let current = with_app(|a| a.listing.as_ref().map(|l| l.vpath.clone())).unwrap_or_default();
    after_mods_change(ui, &current);
    let msg = if failed.is_empty() {
        format!("Restored {done} game files to their originals")
    } else {
        format!("Restored {done} game files; failed: {}", failed.join("; "))
    };
    ui.set_status(msg.into());
    history::refresh(ui);
}

pub(crate) fn open_prompt(ui: &AppWindow, prompt: Prompt, title: &str, value: &str) {
    with_app(|a| a.prompt = Some(prompt));
    ui.set_prompt_title(title.into());
    ui.set_prompt_value(value.into());
    ui.set_prompt_visible(true);
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', ':']) && name != "." && name != ".."
}

pub(crate) fn prompt_accepted(ui: &AppWindow, text: &str) {
    if !valid_name(text) {
        ui.set_status(format!("\"{text}\" is not a valid name.").into());
        return;
    }
    ui.set_prompt_visible(false);
    match with_app(|a| a.prompt.take()) {
        Some(Prompt::Rename { vpath }) => {
            let to = browser::join(&browser::parent(&vpath).unwrap_or_default(), text);
            let what = format!("Renamed {} to {text}", vpath.rsplit('/').next().unwrap_or(&vpath));
            run_archive_ops(ui, vec![(vpath, OpTarget::Rename(to))], what, Some(text.to_owned()));
        }
        Some(Prompt::NewFolder { dir }) => {
            let path = browser::join(&dir, text);
            run_archive_ops(
                ui,
                vec![(path, OpTarget::Op(Op::CreateDir))],
                format!("Created folder {text}"),
                Some(text.to_owned()),
            );
        }
        None => {}
    }
}

/// An operation on a virtual path; renames carry the full new virtual path.
pub(crate) enum OpTarget {
    Op(Op),
    Rename(String),
}

/// Applies operations (grouped per top-level archive) in the background, then refreshes the view.
pub(crate) fn run_archive_ops(
    ui: &AppWindow,
    ops: Vec<(String, OpTarget)>,
    description: String,
    select: Option<String>,
) {
    let Some((game_dir, keys, current)) =
        with_app(|a| Some((a.game_dir.clone()?, a.keys.clone(), a.listing.as_ref()?.vpath.clone())))
    else {
        return;
    };
    set_busy(ui, true, format!("Writing into the {} folder…", write_folder()));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = (|| -> anyhow::Result<install::Applied> {
            let mut grouped: Vec<(String, Vec<(String, Op)>)> = Vec::new();
            for (vpath, op) in ops {
                let t = install::split(&game_dir, &vpath)
                    .ok_or_else(|| anyhow::anyhow!("{vpath} is not inside an archive"))?;
                let op = match op {
                    OpTarget::Op(op) => op,
                    OpTarget::Rename(to) => {
                        let to =
                            install::split(&game_dir, &to).ok_or_else(|| anyhow::anyhow!("invalid target {to}"))?;
                        Op::Rename(to.inner)
                    }
                };
                match grouped.iter_mut().find(|(a, _)| a.eq_ignore_ascii_case(&t.archive)) {
                    Some((_, list)) => list.push((t.inner, op)),
                    None => grouped.push((t.archive, vec![(t.inner, op)])),
                }
            }
            let mut last = None;
            for (archive, list) in grouped {
                last = Some(install::apply(&game_dir, keys.clone(), &archive, &list, &description)?);
            }
            last.ok_or_else(|| anyhow::anyhow!("nothing to do"))
        })();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            set_busy(&ui, false, String::new());
            match result {
                Ok(applied) => {
                    with_app(|a| a.overlay = true);
                    ui.set_mods_available(true);
                    ui.set_mods_overlay(true);
                    rebuild_tree(&ui);
                    let target = match &select {
                        Some(name) => browser::join(&current, name),
                        None => current.clone(),
                    };
                    navigate_now(&ui, &target, false);
                    if select.is_some() && with_app(|a| a.selected.is_none()) {
                        navigate_now(&ui, &current, false);
                    }
                    refresh_undo(&ui);
                    let mut msg = format!("{description} — {:.2}s", started.elapsed().as_secs_f32());
                    if applied.created {
                        msg.push_str(&format!(" (archive copied into the {} folder first)", overlay_name()));
                    }
                    if let Some(note) = overlay_note() {
                        msg.push_str(&format!(" — {note}"));
                    }
                    ui.set_status(msg.into());
                }
                Err(e) => ui.set_status(format!("{description} failed: {e:#}").into()),
            }
        });
    });
}

pub(crate) fn archive_import(ui: &AppWindow, dropped: Option<Vec<PathBuf>>) {
    let Some(dir) = with_app(|a| {
        let l = a.listing.as_ref()?;
        matches!(l.location, Location::Archive(..) | Location::Gta4(..)).then(|| l.vpath.clone())
    }) else {
        ui.set_status("Open a folder inside an archive to import files.".into());
        return;
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let files = match dropped {
            Some(f) => f,
            None => match rfd::FileDialog::new().set_title("Import files into the archive").pick_files() {
                Some(f) => f,
                None => return,
            },
        };
        let mut ops = Vec::new();
        let mut problems = Vec::new();
        for f in files.iter().filter(|f| f.is_file()) {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read(f) {
                Ok(data) => ops.push((browser::join(&dir, &name), OpTarget::Op(Op::Set(data)))),
                Err(e) => problems.push(format!("{name}: {e}")),
            }
        }
        let description = match ops.len() {
            1 => format!("Imported {}", ops[0].0.rsplit('/').next().unwrap_or_default()),
            n => format!("Imported {n} files"),
        };
        let select = (ops.len() == 1).then(|| ops[0].0.rsplit('/').next().unwrap_or_default().to_owned());
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if let Some(p) = problems.first() {
                ui.set_status(format!("Could not read {p}").into());
            }
            if !ops.is_empty() {
                run_archive_ops(&ui, ops, description, select);
            }
        });
    });
}

pub(crate) fn archive_replace(ui: &AppWindow) {
    let Some((vpath, name)) = selected_entry() else { return };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(file) = rfd::FileDialog::new().set_title(format!("Replace {name}")).pick_file() else { return };
        let result = std::fs::read(&file);
        let _ = weak.upgrade_in_event_loop(move |ui| match result {
            Ok(data) => {
                run_archive_ops(&ui, vec![(vpath, OpTarget::Op(Op::Set(data)))], format!("Replaced {name}"), Some(name))
            }
            Err(e) => ui.set_status(format!("Could not read {}: {e}", file.display()).into()),
        });
    });
}

pub(crate) fn archive_delete(ui: &AppWindow) {
    let Some((vpath, name)) = selected_entry() else { return };
    run_archive_ops(ui, vec![(vpath, OpTarget::Op(Op::Remove))], format!("Deleted {name}"), None);
}

/// Shows or hides the Undo button with the newest journal entry.
pub(crate) fn refresh_undo(ui: &AppWindow) {
    history::refresh(ui);
    let newest = with_app(|a| a.game_dir.clone())
        .and_then(|d| rage_mods::mods_dir(&d))
        .and_then(|m| openrage_app::journal::Journal::new(&m).entries().ok())
        .and_then(|e| e.into_iter().next());
    ui.set_can_undo(newest.is_some());
    ui.set_undo_label(newest.map(|e| e.description).unwrap_or_default().into());
}

pub(crate) fn start_undo(ui: &AppWindow) {
    let Some((game_dir, current)) =
        with_app(|a| Some((a.game_dir.clone()?, a.listing.as_ref().map(|l| l.vpath.clone()).unwrap_or_default())))
    else {
        return;
    };
    set_busy(ui, true, "Undoing…".into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = install::undo_last(&game_dir);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            set_busy(&ui, false, String::new());
            let msg = match result {
                Ok(Some(e)) => format!("Undone: {}", e.description),
                Ok(None) => "Nothing to undo.".into(),
                Err(e) => format!("Undo failed: {e:#}"),
            };
            rebuild_tree(&ui);
            // The current folder may no longer exist after undoing.
            let mut target = current.clone();
            loop {
                navigate_now(&ui, &target, false);
                let ok = with_app(|a| a.listing.as_ref().is_some_and(|l| l.vpath.eq_ignore_ascii_case(&target)));
                match browser::parent(&target) {
                    Some(p) if !ok => target = p,
                    _ => break,
                }
            }
            refresh_undo(&ui);
            ui.set_status(msg.into());
        });
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextureAction {
    Replace,
    Add,
    Remove,
}

/// Replaces, adds or removes a texture of the open dictionary and writes the
/// result into the mods folder (in the background).
pub(crate) fn edit_texture(ui: &AppWindow, action: TextureAction) {
    if with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Textures { read_only: true, .. })))
    {
        ui.set_status(
            "Textures inside particle dictionaries (.ypt) can be viewed and extracted, not changed yet.".into(),
        );
        return;
    }
    let Some((game_dir, keys, ytd, selected)) = with_app(|a| {
        let l = a.listing.as_ref()?;
        if !matches!(l.location, Location::Textures { .. }) {
            return None;
        }
        let selected = a.selected.map(|s| l.items[s].name.clone());
        Some((a.game_dir.clone()?, a.keys.clone(), l.vpath.clone(), selected))
    }) else {
        return;
    };
    if action != TextureAction::Add && selected.is_none() {
        ui.set_status("Select a texture first.".into());
        return;
    }
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        use rage_formats::ytd_write::Edit;
        let edit = match action {
            TextureAction::Remove => Edit::Remove(selected.clone().unwrap_or_default()),
            TextureAction::Replace | TextureAction::Add => {
                let title =
                    if action == TextureAction::Replace { "Replace texture with DDS" } else { "Add texture from DDS" };
                let Some(file) =
                    rfd::FileDialog::new().set_title(title).add_filter("DDS texture", &["dds"]).pick_file()
                else {
                    return;
                };
                let name = match action {
                    TextureAction::Replace => selected.clone().unwrap_or_default(),
                    _ => file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                };
                match openrage_app::textures::texture_from_dds(&name, &file) {
                    Ok(t) => Edit::Set(t),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
                        return;
                    }
                }
            }
        };
        let _ = weak.upgrade_in_event_loop(|ui| {
            set_busy(&ui, true, format!("Writing the changed dictionary into the {} folder…", write_folder()))
        });
        let started = Instant::now();
        let result = openrage_app::textures::edit_textures(&game_dir, keys, browser::texture_owner(&ytd), &[edit]);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let msg = match &result {
                Ok((r, path)) => format!(
                    "{} — written to {} in {:.1}s",
                    if r.in_place { "Texture replaced in place" } else { "Dictionary rebuilt" },
                    path.display(),
                    started.elapsed().as_secs_f32()
                ),
                Err(e) => format!("Could not change the dictionary: {e:#}"),
            };
            set_busy(&ui, false, String::new());
            if result.is_ok() {
                // The change lives in the mods folder: show the game as it sees itself.
                with_app(|a| a.overlay = true);
                ui.set_mods_available(true);
                ui.set_mods_overlay(true);
                rebuild_tree(&ui);
                navigate(&ui, &ytd, false);
                refresh_undo(&ui);
            }
            ui.set_status(msg.into());
        });
    });
}
