//! Settings → ASI mods: GTA V's ASI mods, their loader and the Proton setting.

use anyhow::Context;
use openrage_app::asi;

use crate::*;

/// Fills the ASI section of the settings from the open game (hidden for other games).
pub(crate) fn refresh_asi(ui: &AppWindow) {
    let game = with_app(|a| a.game_dir.clone()).filter(|d| detect::is_gta5_dir(d));
    let status = game.as_deref().map(asi::status);
    let s = match status {
        Some(Ok(s)) => s,
        Some(Err(e)) => {
            ui.set_asi_available(true);
            ui.set_asi_summary(SharedString::new());
            ui.set_asi_problem(format!("Cannot read the game folder: {e:#}").into());
            ui.set_asi_mods(ModelRc::new(VecModel::from(Vec::<AsiRow>::new())));
            return;
        }
        None => {
            ui.set_asi_available(false);
            return;
        }
    };
    let mut summary = Vec::new();
    match s.loader() {
        Some(l) => summary.push(format!("Loader: {} ({})", l.file, l.description)),
        None => summary.push("Loader: none for this edition of the game".to_owned()),
    }
    summary.push(format!("Script Hook V: {}", if s.script_hook { "installed" } else { "not installed" }));
    if let (Some(p), Some(l)) = (&s.proton, s.loader()) {
        summary.push(if s.loader_runs() {
            format!("Proton (app {}): loads the game's {}", p.app_id, l.file)
        } else {
            format!("Proton (app {}): uses its own {}", p.app_id, l.file)
        });
    }
    ui.set_asi_available(true);
    ui.set_asi_summary(summary.join("\n").into());
    ui.set_asi_problem(s.problems().join("\n").into());
    let proton_ready = s.proton.is_some() && s.loader().is_some();
    ui.set_asi_can_fix_proton(proton_ready && !s.loader_runs());
    ui.set_asi_proton_set(proton_ready && s.loader_runs());
    let rows: Vec<AsiRow> = s
        .mods
        .iter()
        .map(|m| AsiRow { name: m.name.clone().into(), size: human_size(m.size).into(), enabled: m.enabled })
        .collect();
    ui.set_asi_mods(ModelRc::new(VecModel::from(rows)));
}

pub(crate) fn wire_asi(ui: &AppWindow) {
    let w = ui.as_weak();
    ui.on_asi_refresh(move || with_ui(&w, refresh_asi));
    let w = ui.as_weak();
    ui.on_asi_toggled(move |name, on| {
        with_ui(&w, |ui| {
            let Some(game) = with_app(|a| a.game_dir.clone()) else { return };
            match asi::set_enabled(&game, &name, on) {
                Ok(()) => ui.set_status(format!("{name} {}", if on { "enabled" } else { "disabled" }).into()),
                Err(e) => show_error(ui, format!("Cannot change {name}: {e:#}")),
            }
            refresh_asi(ui);
        })
    });
    let w = ui.as_weak();
    ui.on_asi_install(move || {
        let w = w.clone();
        std::thread::spawn(move || {
            let Some(file) =
                rfd::FileDialog::new().set_title("Install an ASI mod").add_filter("ASI mod", &["asi"]).pick_file()
            else {
                return;
            };
            let _ = w.upgrade_in_event_loop(move |ui| {
                let Some(game) = with_app(|a| a.game_dir.clone()) else { return };
                match asi::install(&game, &file) {
                    Ok(path) => {
                        ui.set_status(format!("Installed {} (undo removes it)", path.display()).into());
                        refresh_undo(&ui);
                    }
                    Err(e) => show_error(&ui, format!("Cannot install the ASI mod: {e:#}")),
                }
                refresh_asi(&ui);
            });
        });
    });
    let w = ui.as_weak();
    ui.on_asi_proton(move |on| {
        with_ui(&w, |ui| {
            let Some(game) = with_app(|a| a.game_dir.clone()) else { return };
            let result = asi::status(&game).and_then(|s| {
                let proton = s.proton.clone().context("the game does not run through Proton here")?;
                let loader = s.loader().context("no ASI loader for this edition of the game")?.clone();
                asi::set_proton_override(&proton, &loader.dll(), on)?;
                Ok(loader.file)
            });
            match result {
                Ok(file) if on => ui.set_status(format!("Proton now loads the game's {file}").into()),
                Ok(file) => ui.set_status(format!("Proton uses its own {file} again").into()),
                Err(e) => show_error(ui, format!("Cannot change the Proton setting: {e:#}")),
            }
            refresh_asi(ui);
        })
    });
}
