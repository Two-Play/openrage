//! Mods → ASI mods…: GTA V's ASI mods, their loader and the Proton setting, in
//! their own window.

use std::cell::RefCell;

use anyhow::Context;
use openrage_app::asi;

use crate::*;

thread_local! {
    static WINDOW: RefCell<Option<AsiWindow>> = const { RefCell::new(None) };
}

/// The ASI window, if it was opened (for debug snapshots).
#[cfg(debug_assertions)]
pub(crate) fn asi_window() -> Option<AsiWindow> {
    window()
}

fn window() -> Option<AsiWindow> {
    WINDOW.with(|w| w.borrow().as_ref().map(ComponentHandle::clone_strong))
}

/// The open game if it is GTA V (the only game with ASI mods).
fn gta5_dir() -> Option<PathBuf> {
    with_app(|a| a.game_dir.clone()).filter(|d| detect::is_gta5_dir(d))
}

/// Enables Mods → ASI mods… for GTA V, shows or hides the hint above the
/// file list, and refreshes an open ASI window (after the game changed).
pub(crate) fn refresh_asi(ui: &AppWindow) {
    let game = gta5_dir();
    let available = game.is_some();
    ui.set_asi_available(available);
    ui.set_asi_hint(game.as_deref().and_then(proton_hint).unwrap_or_default().into());
    if let Some(win) = window() {
        if available {
            fill(&win);
        } else {
            let _ = win.hide();
        }
    }
}

/// The hint for a game whose ASI loader Proton does not load (unless the
/// user dismissed it for this game).
fn proton_hint(game: &Path) -> Option<String> {
    if with_app(|a| a.cfg.asi_hint_off.iter().any(|g| g == game)) {
        return None;
    }
    let s = asi::status(game).ok()?;
    let loader = s.loader()?;
    (s.proton.is_some() && !s.loader_runs()).then(|| {
        format!(
            "ASI mods will not load: Proton uses its own {} instead of the ASI loader in the game folder.",
            loader.file
        )
    })
}

/// Lets Proton load the game's ASI loader (or its own DLL again): what was
/// done, or why not.
fn fix_proton(game: &Path, on: bool) -> Result<String, String> {
    let result = asi::status(game).and_then(|s| {
        let proton = s.proton.clone().context("the game does not run through Proton here")?;
        let loader = s.loader().context("no ASI loader for this edition of the game")?.clone();
        asi::set_proton_override(&proton, &loader.dll(), on)?;
        Ok(loader.file)
    });
    match result {
        Ok(file) if on => Ok(format!("Proton now loads the game's {file}")),
        Ok(file) => Ok(format!("Proton uses its own {file} again")),
        Err(e) => Err(format!("Cannot change the Proton setting: {e:#}")),
    }
}

/// Shows the ASI window (creating it on first use).
pub(crate) fn open_asi_window(ui: &AppWindow) {
    let win = match window() {
        Some(w) => w,
        None => {
            let Ok(win) = AsiWindow::new() else { return };
            wire(&win, ui);
            WINDOW.with(|w| *w.borrow_mut() = Some(win.clone_strong()));
            win
        }
    };
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(&win);
    fill(&win);
    let _ = win.show();
}

/// Fills the window from the game folder (and its Proton prefix).
fn fill(win: &AsiWindow) {
    let Some(game) = gta5_dir() else { return };
    win.set_game(game.display().to_string().into());
    let s = match asi::status(&game) {
        Ok(s) => s,
        Err(e) => {
            win.set_summary(SharedString::new());
            win.set_problem(format!("Cannot read the game folder: {e:#}").into());
            win.set_mods(ModelRc::new(VecModel::from(Vec::<AsiRow>::new())));
            return;
        }
    };
    let mut summary = vec![format!("Game: GTA V {}", if s.enhanced { "Enhanced" } else { "Legacy" })];
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
    win.set_summary(summary.join("\n").into());
    win.set_problem(s.problems().join("\n").into());
    let proton_ready = s.proton.is_some() && s.loader().is_some();
    win.set_can_fix_proton(proton_ready && !s.loader_runs());
    win.set_proton_set(proton_ready && s.loader_runs());
    let rows: Vec<AsiRow> = s
        .mods
        .iter()
        .map(|m| AsiRow { name: m.name.clone().into(), size: human_size(m.size).into(), enabled: m.enabled })
        .collect();
    win.set_mods(ModelRc::new(VecModel::from(rows)));
}

fn wire(win: &AsiWindow, ui: &AppWindow) {
    let w = win.as_weak();
    win.on_refresh(move || {
        if let Some(win) = w.upgrade() {
            fill(&win);
            win.set_status(SharedString::new());
        }
    });
    let w = win.as_weak();
    win.on_toggled(move |name, on| {
        let Some(win) = w.upgrade() else { return };
        let Some(game) = gta5_dir() else { return };
        win.set_status(
            match asi::set_enabled(&game, &name, on) {
                Ok(()) => format!("{name} {}", if on { "enabled" } else { "disabled (put aside, not deleted)" }),
                Err(e) => format!("Cannot change {name}: {e:#}"),
            }
            .into(),
        );
        fill(&win);
    });
    let (w, main) = (win.as_weak(), ui.as_weak());
    win.on_install(move || {
        let (w, main) = (w.clone(), main.clone());
        std::thread::spawn(move || {
            let Some(file) =
                rfd::FileDialog::new().set_title("Install an ASI mod").add_filter("ASI mod", &["asi"]).pick_file()
            else {
                return;
            };
            let _ = w.upgrade_in_event_loop(move |win| {
                let Some(game) = gta5_dir() else { return };
                win.set_status(
                    match asi::install(&game, &file) {
                        Ok(path) => {
                            // The main window's undo button and History know it now.
                            if let Some(ui) = main.upgrade() {
                                refresh_undo(&ui);
                            }
                            format!("Installed {} (undo removes it)", path.display())
                        }
                        Err(e) => format!("Cannot install the ASI mod: {e:#}"),
                    }
                    .into(),
                );
                fill(&win);
            });
        });
    });
    let (w, main) = (win.as_weak(), ui.as_weak());
    win.on_proton(move |on| {
        let Some(win) = w.upgrade() else { return };
        let Some(game) = gta5_dir() else { return };
        win.set_status(fix_proton(&game, on).unwrap_or_else(|e| e).into());
        fill(&win);
        // The hint above the main window's file list follows.
        if let Some(ui) = main.upgrade() {
            refresh_asi(&ui);
        }
    });
}

pub(crate) fn wire_asi(ui: &AppWindow) {
    let w = ui.as_weak();
    ui.on_open_asi_window(move || with_ui(&w, open_asi_window));
    let w = ui.as_weak();
    ui.on_asi_hint_fix(move || {
        with_ui(&w, |ui| {
            let Some(game) = gta5_dir() else { return };
            match fix_proton(&game, true) {
                Ok(done) => ui.set_status(done.into()),
                Err(e) => show_error(ui, e),
            }
            refresh_asi(ui);
        })
    });
    let w = ui.as_weak();
    ui.on_asi_hint_dismiss(move || {
        with_ui(&w, |ui| {
            let Some(game) = gta5_dir() else { return };
            with_app(|a| {
                if !a.cfg.asi_hint_off.contains(&game) {
                    a.cfg.asi_hint_off.push(game);
                }
            });
            save_settings(ui);
            refresh_asi(ui);
        })
    });
}
