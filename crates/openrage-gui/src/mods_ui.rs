//! Mods: the history of changes and installing OpenIV packages.

use crate::*;

/// The journal of the mods folder, newest first.
pub(crate) fn journal_entries() -> Vec<openrage_app::journal::Entry> {
    with_app(|a| a.game_dir.clone())
        .and_then(|d| rage_mods::mods_dir(&d))
        .and_then(|m| openrage_app::journal::Journal::new(&m).entries().ok())
        .unwrap_or_default()
}

/// Shows the history in the file list (the alternative to its own window).
pub(crate) fn show_history_in_list(ui: &AppWindow) {
    show_history(ui, true);
}

pub(crate) fn show_history(ui: &AppWindow, record: bool) {
    let entries = journal_entries();
    let items: Vec<Item> = entries
        .iter()
        .map(|e| {
            use openrage_app::journal::Kind;
            let place = match e.kind {
                Kind::FileWritten { in_game: true, .. } | Kind::Edited { in_game: true, .. } => {
                    format!("game folder: {}", e.archive)
                }
                _ => format!("{}/{}", overlay_name(), e.archive),
            };
            Item {
                name: e.description.clone(),
                kind: if e.archive.to_ascii_lowercase().ends_with(".rpf") { ItemKind::Archive } else { ItemKind::File },
                size: 0,
                info: e.group.clone().unwrap_or_default(),
                entry: None,
                vpath: Some(e.archive.clone()),
                mark: None,
                detail: Some(place),
            }
        })
        .collect();
    let summary = format!("{} recorded changes", items.len());
    let vpath = browser::HISTORY_PATH.to_owned();
    with_app(|a| {
        if record {
            a.history.push(&vpath);
        }
        a.listing = Some(Listing { vpath: vpath.clone(), origin: Origin::Base, location: Location::History, items });
        a.selected = None;
        a.filter.clear();
    });
    ui.set_filter(SharedString::new());
    set_location(ui, &vpath);
    ui.set_search_mode(true);
    ui.set_show_language(false);
    ui.set_search_summary(summary.into());
    ui.set_tree_selected(-1);
    refresh_files(ui, None);
    show_folder_info(ui);
    update_nav_buttons(ui);
}

pub(crate) fn after_mods_change(ui: &AppWindow, current: &str) {
    with_app(|a| a.overlay = true);
    ui.set_mods_available(true);
    ui.set_mods_overlay(true);
    rebuild_tree(ui);
    navigate(ui, current, false);
    refresh_undo(ui);
}

pub(crate) fn pick_package(ui: &AppWindow) {
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(file) = rfd::FileDialog::new()
            .set_title("Install OpenIV package")
            .add_filter("OpenIV package", &["oiv"])
            .pick_file()
        else {
            return;
        };
        let _ = weak.upgrade_in_event_loop(move |ui| open_package(&ui, file));
    });
}

/// Reads a package, previews it and shows the install dialog.
pub(crate) fn open_package(ui: &AppWindow, file: PathBuf) {
    let Some((game_dir, keys)) = with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone()))) else { return };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let _ = weak.upgrade_in_event_loop(|ui| ui.set_status("Reading package…".into()));
        let result = openrage_app::oiv::Package::open(&file)
            .and_then(|pkg| openrage_app::oiv::install::preview(&game_dir, keys, &pkg).map(|r| (pkg, r)));
        let _ = weak.upgrade_in_event_loop(move |ui| match result {
            Ok((pkg, report)) => {
                let m = &pkg.metadata;
                ui.set_oiv_title(format!("{} {}", m.name, m.version).trim().to_owned().into());
                let mut sub = format!("by {}", if m.author.is_empty() { "unknown author" } else { &m.author });
                if !m.web.is_empty() {
                    sub.push_str(&format!(" · {}", m.web));
                }
                ui.set_oiv_subtitle(sub.into());
                ui.set_oiv_description(m.description.clone().into());
                let changes: Vec<String> = report
                    .steps
                    .iter()
                    .map(|s| {
                        let detail = if s.detail.is_empty() { String::new() } else { format!("  ({})", s.detail) };
                        format!("{:<8} {}{detail}", s.action, s.target)
                    })
                    .collect();
                ui.set_oiv_changes(changes.join("\n").into());
                let mut problems: Vec<String> =
                    report.blockers.iter().map(|b| format!("Cannot install: {b}")).collect();
                problems.extend(report.warnings.iter().map(|w| format!("Note: {w}")));
                ui.set_oiv_problems(problems.join("\n").into());
                ui.set_oiv_can_install(report.blockers.is_empty());
                ui.set_oiv_running(false);
                ui.set_oiv_visible(true);
                ui.set_status(SharedString::new());
                with_app(|a| a.package = Some(Arc::new(pkg)));
            }
            Err(e) => show_error(&ui, format!("Could not read the package: {e:#}")),
        });
    });
}

pub(crate) fn install_package(ui: &AppWindow) {
    let Some((game_dir, keys, pkg, current)) = with_app(|a| {
        Some((
            a.game_dir.clone()?,
            a.keys.clone(),
            a.package.clone()?,
            a.listing.as_ref().map(|l| l.vpath.clone()).unwrap_or_default(),
        ))
    }) else {
        return;
    };
    ui.set_oiv_running(true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = openrage_app::oiv::install::install(&game_dir, keys, &pkg);
        let name = openrage_app::oiv::install::group_name(&pkg);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            ui.set_oiv_running(false);
            match result {
                Ok(_) => {
                    ui.set_oiv_visible(false);
                    with_app(|a| a.package = None);
                    after_mods_change(&ui, &current);
                    ui.set_status(
                        format!(
                            "Installed \"{name}\" in {:.1}s — uninstall it from History.",
                            started.elapsed().as_secs_f32()
                        )
                        .into(),
                    );
                }
                Err(e) => ui.set_oiv_problems(format!("Installation failed: {e:#}").into()),
            }
        });
    });
}

pub(crate) fn uninstall_selected_group(ui: &AppWindow) {
    let group = ui.get_history_group().to_string();
    let Some(game_dir) = with_app(|a| a.game_dir.clone()) else { return };
    if group.is_empty() {
        return;
    }
    set_busy(ui, true, format!("Uninstalling \"{group}\"…"));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = openrage_app::oiv::install::uninstall(&game_dir, &group);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            set_busy(&ui, false, String::new());
            after_mods_change(&ui, browser::HISTORY_PATH);
            ui.set_status(
                match result {
                    Ok(done) => format!("Uninstalled \"{group}\" ({} changes undone).", done.len()),
                    Err(e) => format!("Could not uninstall \"{group}\": {e:#}"),
                }
                .into(),
            );
        });
    });
}
