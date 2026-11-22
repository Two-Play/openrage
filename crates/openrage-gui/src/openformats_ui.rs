//! Export openFormats… and Import openFormats… in the explorer's context menu.

use openrage_app::openformats::{self as ofs, Kind};
use rage_resource::Resource;

use crate::*;

/// A GTA V file inside an archive: its resource and page flags.
fn read_resource(vpath: &str) -> anyhow::Result<(Resource, (u32, u32))> {
    let (game_dir, keys) = with_app(|a| (a.game_dir.clone(), a.keys.clone()));
    let game_dir = game_dir.ok_or_else(|| anyhow::anyhow!("no game folder"))?;
    let Node::ArchiveFile(rpf, id) = rage_mods::resolve(&game_dir, vpath, keys)?.0 else {
        anyhow::bail!("openFormats are for GTA V files inside archives")
    };
    let rage_rpf::EntryKind::Resource { system_flags, graphics_flags, .. } = rpf.entry(id).kind else {
        anyhow::bail!("{vpath} is not a resource")
    };
    Ok((rpf.read_resource(id)?, (system_flags, graphics_flags)))
}

/// Asks for a folder and writes the file there as openFormats text files.
pub(crate) fn export(ui: &AppWindow, vpath: String) {
    let name = vpath.rsplit('/').next().unwrap_or(&vpath).to_owned();
    let res = match read_resource(&vpath) {
        Ok((r, _)) => r,
        Err(e) => return show_error(ui, format!("Cannot read {name}: {e:#}")),
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(dir) =
            rfd::FileDialog::new().set_title(format!("Export {name} as openFormats to folder")).pick_folder()
        else {
            return;
        };
        let msg = match ofs::export(&res, &name).and_then(|files| Ok((ofs::write_files(&dir, &files)?, files.len()))) {
            Ok((main, n)) => format!("Exported {n} files; open {} in OpenIV or a text editor", main.display()),
            Err(e) => format!("Cannot export {name} as openFormats: {e:#}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

/// Asks for an openFormats file and puts the selected file, rebuilt from it,
/// into the mods folder (undoable, like Replace…).
pub(crate) fn import(ui: &AppWindow) {
    let Some((vpath, name)) = selected_entry() else { return };
    let Some(kind) = Kind::of_game_file(&name).filter(|k| k.can_import()) else {
        return show_error(ui, format!("openFormats cannot be imported onto {name}"));
    };
    let template = match read_resource(&vpath) {
        Ok(t) => t,
        Err(e) => return show_error(ui, format!("Cannot read {name}: {e:#}")),
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let ext = kind.open_ext();
        let Some(file) = rfd::FileDialog::new()
            .set_title(format!("Import openFormats onto {name}"))
            .add_filter(format!("openFormats (.{ext})"), &[ext])
            .pick_file()
        else {
            return;
        };
        let result = ofs::import(&template.0, template.1, &name, &file);
        let file_name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |ui| match result {
            Ok(imported) => match imported.file {
                // What was taken over goes into the description (status and History).
                Some(data) => run_archive_ops(
                    &ui,
                    vec![(vpath, OpTarget::Op(Op::Set(data)))],
                    format!("Imported {file_name} into {name} ({})", imported.notes.join("; ")),
                    Some(name),
                ),
                None => ui.set_status(format!("{file_name} is the same as {name}; nothing changed").into()),
            },
            Err(e) => show_error(&ui, format!("Cannot import {file_name} onto {name}: {e:#}")),
        });
    });
}
