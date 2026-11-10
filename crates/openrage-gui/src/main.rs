//! OpenRAGE graphical explorer.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod badges;
mod browser;
mod preview;
mod viewer;

use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use openrage_app::config::Config;
use openrage_app::extract::{extract_entry, human_size};
use openrage_app::install::{self, Op};
use openrage_app::setup::{self, KeyBlobRequired, KeySource};
use openrage_app::{detect, game_language_from_locale, index_path, load_keys};
use rage_crypto::GtaVKeys;
use rage_index::{EntryTag, HitReason, Index, LANGUAGES};
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryId, EntryKind, Rpf7, RpfError};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use browser::{History, Item, ItemKind, Listing, Location, Mark, Tree};
use rage_mods::{ChangeKind, CompareMode, Origin};

slint::include_modules!();

/// UI-thread state. Lives in a thread-local so that closures posted back from
/// worker threads (which must be `Send`) can reach it.
/// A loaded model ready to be rendered.
struct ModelPreview {
    loaded: openrage_app::models::LoadedModel,
    materials: openrage_app::render::Materials,
    name: String,
}

struct App {
    cfg: Config,
    game_dir: Option<PathBuf>,
    keys: Option<Arc<GtaVKeys>>,
    listing: Option<Listing>,
    /// Indices into `listing.items` in display order (sorted + filtered).
    visible: Vec<usize>,
    selected: Option<usize>,
    sort_column: i32,
    sort_descending: bool,
    filter: String,
    tree: Tree,
    history: History,
    preview_generation: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    busy: bool,
    /// Text language for search hits in localized files (None = all).
    language: Option<String>,
    search_generation: Arc<AtomicU64>,
    /// Show the game as it sees itself with the OpenIV `mods` folder applied.
    overlay: bool,
    diff_generation: Arc<AtomicU64>,
    /// What the open text prompt is for.
    prompt: Option<Prompt>,
    /// Package shown in the install dialog.
    package: Option<Arc<openrage_app::oiv::Package>>,
    /// Model shown in the preview, with its camera.
    model: Option<Arc<ModelPreview>>,
    model_view: openrage_app::render::View,
    /// Open a viewer window as soon as the current preview has loaded.
    pending_viewer: bool,
    index_running: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(a.borrow_mut().as_mut().expect("app initialized")))
}

fn main() -> anyhow::Result<()> {
    let ui = AppWindow::new()?;
    let cfg = Config::load();
    let keys = load_keys().unwrap_or(None);
    let game_dir = cfg.game_dir.clone().filter(|d| detect::is_gta5_dir(d));

    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            cfg,
            game_dir,
            keys,
            listing: None,
            visible: Vec::new(),
            selected: None,
            sort_column: 0,
            sort_descending: false,
            filter: String::new(),
            tree: Tree::default(),
            history: History::default(),
            preview_generation: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
            busy: false,
            language: game_language_from_locale().map(str::to_owned),
            search_generation: Arc::new(AtomicU64::new(0)),
            overlay: false,
            diff_generation: Arc::new(AtomicU64::new(0)),
            prompt: None,
            package: None,
            model: None,
            model_view: openrage_app::render::View::default(),
            pending_viewer: false,
            index_running: false,
        })
    });

    ui.set_mono_font(mono_font().into());
    ui.set_theme_mode(with_app(|a| theme_mode(a.cfg.theme.as_deref())));
    let languages: Vec<SharedString> =
        std::iter::once("All languages".into()).chain(LANGUAGES.iter().map(|l| capitalize(l).into())).collect();
    ui.set_languages(ModelRc::new(VecModel::from(languages)));
    ui.set_language_index(with_app(|a| {
        a.language.as_deref().and_then(|l| LANGUAGES.iter().position(|x| *x == l)).map_or(0, |i| i as i32 + 1)
    }));
    wire_callbacks(&ui);

    let ready = with_app(|app| app.game_dir.is_some() && app.keys.is_some());
    if ready {
        start_browsing(&ui);
    } else {
        show_setup(&ui, false);
    }
    #[cfg(debug_assertions)]
    debug_snapshot(&ui);
    ui.run()?;
    Ok(())
}

/// Development aid (debug builds only): `OPENRAGE_SNAPSHOT=out.png` saves an
/// image of this window after a few seconds and quits; `OPENRAGE_THEME`
/// (`light`/`dark`), `OPENRAGE_VIEWER`, `OPENRAGE_EDIT` and `OPENRAGE_UNLOCK`
/// set up the state to capture.
#[cfg(debug_assertions)]
fn debug_snapshot(ui: &AppWindow) {
    if let Ok(theme) = std::env::var("OPENRAGE_THEME") {
        ui.set_theme_mode(theme_mode(Some(&theme)));
    }
    let Ok(path) = std::env::var("OPENRAGE_SNAPSHOT") else { return };
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_secs(5), move || {
        let Some(ui) = weak.upgrade() else { return };
        if std::env::var("OPENRAGE_VIEWER").is_ok() {
            open_viewer(&ui);
        }
        if std::env::var("OPENRAGE_EDIT").is_ok() {
            ui.set_edit_mode(true);
        }
        if std::env::var("OPENRAGE_UNLOCK").is_ok() {
            ui.set_unlock_visible(true);
        }
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_secs(2), move || {
            let Some(ui) = weak.upgrade() else { return };
            let snap = match viewer::newest() {
                Some(v) => v.window().take_snapshot(),
                None => ui.window().take_snapshot(),
            };
            match snap {
                Ok(img) => {
                    let _ = rage_formats::png(img.width(), img.height(), img.as_bytes())
                        .map(|png| std::fs::write(&path, png));
                }
                Err(e) => eprintln!("snapshot failed: {e}"),
            }
            let _ = slint::quit_event_loop();
        });
    });
}

fn wire_callbacks(ui: &AppWindow) {
    let w = ui.as_weak();
    ui.on_navigate(move |path| with_ui(&w, |ui| navigate(ui, &path, true)));
    let w = ui.as_weak();
    ui.on_go_back(move || {
        with_ui(&w, |ui| {
            if let Some(p) = with_app(|a| a.history.back()) {
                navigate(ui, &p, false);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_go_forward(move || {
        with_ui(&w, |ui| {
            if let Some(p) = with_app(|a| a.history.forward()) {
                navigate(ui, &p, false);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_go_up(move || {
        with_ui(&w, |ui| {
            let parent = with_app(|a| a.listing.as_ref().and_then(|l| browser::parent(&l.vpath)));
            if let Some(p) = parent {
                navigate(ui, &p, true);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_tree_toggle(move |i| {
        with_ui(&w, |ui| {
            with_app(|a| {
                if let Some(dir) = a.game_dir.clone() {
                    a.tree.toggle(i as usize, &dir, a.keys.clone(), a.overlay);
                }
            });
            refresh_tree(ui, None);
        })
    });
    let w = ui.as_weak();
    ui.on_tree_action(move |i, action| {
        with_ui(&w, |ui| {
            let Some(vpath) = with_app(|a| a.tree.nodes.get(i as usize).map(|n| n.vpath.clone())) else { return };
            match action.as_str() {
                "extract" => {
                    navigate(ui, &vpath, true);
                    start_extract(ui);
                }
                "copy-path" => {
                    let msg = match copy_to_clipboard(&vpath) {
                        Ok(()) => format!("Copied {vpath}"),
                        Err(e) => format!("Cannot copy to the clipboard: {e}"),
                    };
                    ui.set_status(msg.into());
                }
                _ => {}
            }
        })
    });
    let w = ui.as_weak();
    ui.on_tree_activate(move |i| {
        with_ui(&w, |ui| {
            if let Some(p) = with_app(|a| a.tree.nodes.get(i as usize).map(|n| n.vpath.clone())) {
                navigate(ui, &p, true);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_file_select(move |i| with_ui(&w, |ui| select_row(ui, i as usize)));
    let w = ui.as_weak();
    ui.on_file_open(move |i| {
        with_ui(&w, |ui| {
            let viewable = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(i as usize)?];
                Some(is_viewable(item) && !is_openable(item))
            });
            if viewable == Some(true) {
                if ui.get_file_selected() != i {
                    select_row(ui, i as usize);
                }
                open_viewer(ui);
                return;
            }
            let target = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(i as usize)?];
                if matches!(listing.location, Location::History) {
                    return None;
                }
                if let Some(vpath) = &item.vpath {
                    // Search hit or change: jump to its location (removed entries: their folder).
                    return if item.mark == Some(Mark::Removed) { browser::parent(vpath) } else { Some(vpath.clone()) };
                }
                let opens = item.kind.is_container()
                    || (item.kind == ItemKind::Resource && browser::is_texture_dict(&item.name));
                opens.then(|| browser::join(&listing.vpath, &item.name))
            });
            if let Some(p) = target {
                navigate(ui, &p, true);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_sort_by(move |col| {
        with_ui(&w, |ui| {
            with_app(|a| {
                if a.sort_column == col {
                    a.sort_descending = !a.sort_descending;
                } else {
                    a.sort_column = col;
                    a.sort_descending = false;
                }
            });
            refresh_files(ui, None);
        })
    });
    let w = ui.as_weak();
    ui.on_filter_edited(move |text| {
        with_ui(&w, |ui| {
            with_app(|a| a.filter = text.to_string());
            refresh_files(ui, None);
        })
    });
    let w = ui.as_weak();
    ui.on_search(move |text| {
        with_ui(&w, |ui| {
            let q = text.trim();
            if !q.is_empty() {
                navigate(ui, &format!("?{q}"), true);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_language_picked(move |i| {
        with_ui(&w, |ui| {
            let query = with_app(|a| {
                a.language = (i > 0).then(|| LANGUAGES[i as usize - 1].to_owned());
                match a.listing.as_ref().map(|l| &l.location) {
                    Some(Location::Search { query, .. }) => Some(query.clone()),
                    _ => None,
                }
            });
            if let Some(q) = query {
                navigate(ui, &format!("?{q}"), false);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_overlay_toggled(move |on| {
        with_ui(&w, |ui| {
            with_app(|a| a.overlay = on);
            let current = with_app(|a| a.listing.as_ref().map(|l| l.vpath.clone())).unwrap_or_default();
            rebuild_tree(ui);
            navigate(ui, &current, false);
        })
    });
    let w = ui.as_weak();
    ui.on_show_changes(move || with_ui(&w, |ui| navigate(ui, browser::CHANGES_PATH, true)));
    let w = ui.as_weak();
    ui.on_extract_selected(move || with_ui(&w, start_extract));
    let w = ui.as_weak();
    ui.on_model_orbit(move |dx, dy| {
        with_ui(&w, |ui| {
            with_app(|a| {
                a.model_view.yaw -= dx * 0.01;
                a.model_view.pitch = (a.model_view.pitch + dy * 0.01).clamp(-1.5, 1.5);
            });
            render_model(ui);
        })
    });
    let w = ui.as_weak();
    ui.on_model_zoom(move |delta| {
        with_ui(&w, |ui| {
            with_app(|a| a.model_view.zoom = (a.model_view.zoom * 1.1f32.powf(delta / 120.0)).clamp(0.2, 20.0));
            render_model(ui);
        })
    });
    let w = ui.as_weak();
    ui.on_model_resized(move || with_ui(&w, render_model));
    let w = ui.as_weak();
    ui.on_export_gltf(move || with_ui(&w, export_gltf));
    let w = ui.as_weak();
    ui.on_open_viewer(move || with_ui(&w, open_viewer));
    let w = ui.as_weak();
    ui.on_row_action(move |i, action| with_ui(&w, |ui| row_action(ui, i as usize, &action)));
    ui.on_theme_changed(|mode| {
        with_app(|a| {
            a.cfg.theme = Some(theme_name(mode).to_owned());
            let _ = a.cfg.save();
        });
        viewer::set_theme(mode);
    });
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    let w = ui.as_weak();
    ui.on_save_png(move || with_ui(&w, |ui| export_textures(ui, true)));
    let w = ui.as_weak();
    ui.on_archive_import(move || with_ui(&w, |ui| archive_import(ui, None)));
    let w = ui.as_weak();
    ui.on_files_dropped(move |data| {
        let paths: Vec<PathBuf> = data.file_paths().map(|it| it.map(Path::to_path_buf).collect()).unwrap_or_default();
        with_ui(&w, |ui| {
            if paths.is_empty() {
                ui.set_status("Only files can be dropped here.".into());
            } else {
                archive_import(ui, Some(paths));
            }
        })
    });
    let w = ui.as_weak();
    ui.on_archive_replace(move || with_ui(&w, archive_replace));
    let w = ui.as_weak();
    ui.on_archive_delete(move || with_ui(&w, archive_delete));
    let w = ui.as_weak();
    ui.on_archive_rename(move || {
        with_ui(&w, |ui| {
            let Some((vpath, name)) = selected_entry() else { return };
            open_prompt(ui, Prompt::Rename { vpath }, "Rename to", &name);
        })
    });
    let w = ui.as_weak();
    ui.on_archive_new_folder(move || {
        with_ui(&w, |ui| {
            let Some(dir) = with_app(|a| a.listing.as_ref().map(|l| l.vpath.clone())) else { return };
            open_prompt(ui, Prompt::NewFolder { dir }, "New folder name", "new_folder");
        })
    });
    let w = ui.as_weak();
    ui.on_prompt_accepted(move |text| with_ui(&w, |ui| prompt_accepted(ui, text.trim())));
    let w = ui.as_weak();
    ui.on_undo(move || with_ui(&w, start_undo));
    let w = ui.as_weak();
    ui.on_show_history(move || with_ui(&w, |ui| navigate(ui, browser::HISTORY_PATH, true)));
    let w = ui.as_weak();
    ui.on_install_oiv(move || with_ui(&w, pick_package));
    let w = ui.as_weak();
    ui.on_oiv_confirm(move || with_ui(&w, install_package));
    let w = ui.as_weak();
    ui.on_uninstall_group(move || with_ui(&w, uninstall_selected_group));
    let w = ui.as_weak();
    ui.on_texture_replace(move || with_ui(&w, |ui| edit_texture(ui, TextureAction::Replace)));
    let w = ui.as_weak();
    ui.on_texture_add(move || with_ui(&w, |ui| edit_texture(ui, TextureAction::Add)));
    let w = ui.as_weak();
    ui.on_texture_remove(move || with_ui(&w, |ui| edit_texture(ui, TextureAction::Remove)));
    ui.on_cancel_task(|| with_app(|a| a.cancel.store(true, Ordering::Relaxed)));
    let w = ui.as_weak();
    ui.on_open_setup(move || with_ui(&w, |ui| show_setup(ui, true)));
    let w = ui.as_weak();
    ui.on_setup_pick_detected(move |value| with_ui(&w, |ui| ui.set_setup_game_dir(value)));
    let w = ui.as_weak();
    ui.on_setup_browse_game(move || {
        pick_path(w.clone(), false, "Select the GTA V folder", |ui, p| {
            ui.set_setup_game_dir(p.display().to_string().into())
        })
    });
    let w = ui.as_weak();
    ui.on_setup_browse_magic(move || {
        pick_path(w.clone(), true, "Select magic.dat", |ui, p| ui.set_setup_magic_path(p.display().to_string().into()))
    });
    let w = ui.as_weak();
    ui.on_setup_run(move || with_ui(&w, run_setup));
}

fn theme_name(mode: i32) -> &'static str {
    match mode {
        1 => "light",
        2 => "dark",
        _ => "system",
    }
}

fn theme_mode(name: Option<&str>) -> i32 {
    match name {
        Some("light") => 1,
        Some("dark") => 2,
        _ => 0,
    }
}

/// Context-menu action on a file row.
fn row_action(ui: &AppWindow, row: usize, action: &str) {
    if ui.get_file_selected() != row as i32 {
        select_row(ui, row);
    }
    match action {
        "open" => ui.invoke_file_open(row as i32),
        "view" => open_viewer(ui),
        "extract" => ui.invoke_extract_selected(),
        "gltf" => {
            if with_app(|a| a.model.is_some()) {
                ui.invoke_export_gltf();
            } else {
                ui.set_status("The model is still loading.".into());
            }
        }
        "png" => ui.invoke_save_png(),
        "replace" if ui.get_texture_mode() => ui.invoke_texture_replace(),
        "replace" => ui.invoke_archive_replace(),
        "rename" => ui.invoke_archive_rename(),
        "delete" if ui.get_texture_mode() => ui.invoke_texture_remove(),
        "delete" => ui.invoke_archive_delete(),
        "uninstall" => ui.invoke_uninstall_group(),
        "copy-name" | "copy-path" => {
            let text = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(row)?];
                Some(if action == "copy-name" {
                    item.name.clone()
                } else {
                    item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name))
                })
            });
            if let Some(text) = text {
                let msg = match copy_to_clipboard(&text) {
                    Ok(()) => format!("Copied {text}"),
                    Err(e) => format!("Cannot copy to the clipboard: {e}"),
                };
                ui.set_status(msg.into());
            }
        }
        _ => {}
    }
}

thread_local! {
    // Kept alive: on X11 the clipboard content disappears with its owner.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

fn copy_to_clipboard(text: &str) -> Result<(), arboard::Error> {
    CLIPBOARD.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(arboard::Clipboard::new()?);
        }
        c.as_mut().map_or(Ok(()), |c| c.set_text(text))
    })
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The platform's fixed-width font family.
fn mono_font() -> String {
    if cfg!(windows) {
        return "Consolas".into();
    }
    if cfg!(target_os = "macos") {
        return "Menlo".into();
    }
    std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}", "monospace"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "DejaVu Sans Mono".into())
}

fn with_ui(w: &Weak<AppWindow>, f: impl FnOnce(&AppWindow)) {
    if let Some(ui) = w.upgrade() {
        f(&ui);
    }
}

// ---------------------------------------------------------------------------
// Browsing

fn start_browsing(ui: &AppWindow) {
    let mods = with_app(|a| a.game_dir.as_deref().and_then(rage_mods::mods_dir).is_some());
    with_app(|a| a.overlay = mods);
    ui.set_mods_available(mods);
    ui.set_mods_overlay(mods);
    rebuild_tree(ui);
    let title = with_app(|a| {
        let dir = a.game_dir.clone().expect("game dir");
        a.history = History::default();
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("OpenRAGE — {name}")
    });
    ui.set_window_title(title.into());
    start_index_update(ui);
    refresh_undo(ui);
    navigate(ui, "", true);
    // Optional start path, e.g. `openrage-gui x64e.rpf/levels/gta5/vehicles.rpf/adder.yft`.
    if let Some(arg) = std::env::args().nth(1) {
        if arg.to_ascii_lowercase().ends_with(".oiv") && Path::new(&arg).is_file() {
            // `openrage-gui package.oiv` (e.g. opened from a file manager): show the install dialog.
            open_package(ui, PathBuf::from(arg));
        } else {
            navigate(ui, &arg, true);
        }
    }
}

fn rebuild_tree(ui: &AppWindow) {
    with_app(|a| {
        if let Some(dir) = a.game_dir.clone() {
            a.tree = Tree::build(&dir, a.keys.clone(), a.overlay);
        }
    });
    refresh_tree(ui, None);
}

fn navigate(ui: &AppWindow, path: &str, record: bool) {
    if let Some(query) = browser::search_query(path) {
        start_search(ui, query.to_owned(), record);
        return;
    }
    if path.trim() == browser::CHANGES_PATH {
        start_changes(ui, record);
        return;
    }
    if path.trim() == browser::HISTORY_PATH {
        show_history(ui, record);
        return;
    }
    let Some((dir, keys, overlay)) = with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone(), a.overlay))) else {
        return;
    };
    match browser::list(&dir, path, keys.clone(), overlay) {
        Ok((listing, select)) => {
            let vpath = listing.vpath.clone();
            with_app(|a| {
                if record {
                    a.history.push(&vpath);
                }
                a.listing = Some(listing);
                a.selected = None;
                a.filter.clear();
            });
            ui.set_filter(SharedString::new());
            ui.set_search_mode(false);
            set_location(ui, &vpath);
            let tree_idx = with_app(|a| a.tree.reveal(&vpath, &dir, keys, overlay));
            refresh_tree(ui, tree_idx);
            ui.set_location_title(with_app(|a| a.listing.as_ref().map(location_title)).unwrap_or_default().into());
            refresh_files(ui, select.as_deref());
            if select.is_none() {
                show_folder_info(ui);
            }
            ui.set_status(SharedString::new());
            start_folder_diff(ui);
        }
        Err(RpfError::KeysRequired(_)) => show_setup(ui, true),
        Err(e) => ui.set_status(format!("{e}").into()),
    }
    update_nav_buttons(ui);
}

fn update_nav_buttons(ui: &AppWindow) {
    with_app(|a| {
        ui.set_can_back(a.history.can_back());
        ui.set_can_forward(a.history.can_forward());
        ui.set_can_up(a.listing.as_ref().is_some_and(|l| browser::parent(&l.vpath).is_some()));
    });
}

fn refresh_tree(ui: &AppWindow, selected: Option<usize>) {
    let rows: Vec<TreeRow> = with_app(|a| {
        a.tree
            .nodes
            .iter()
            .map(|n| {
                let (icon, tint) = badges::icon(&n.name, n.kind);
                TreeRow {
                    name: n.name.clone().into(),
                    path: n.vpath.clone().into(),
                    depth: n.depth as i32,
                    expandable: n.expandable,
                    expanded: n.expanded,
                    icon: icon.into(),
                    tint,
                }
            })
            .collect()
    });
    ui.set_tree(ModelRc::new(VecModel::from(rows)));
    if let Some(i) = selected {
        ui.set_tree_selected(i as i32);
    } else {
        // Keep the highlight on the node matching the current folder.
        let idx = with_app(|a| {
            let current = a.listing.as_ref()?.vpath.clone();
            a.tree.nodes.iter().position(|n| n.vpath.eq_ignore_ascii_case(&current))
        });
        ui.set_tree_selected(idx.map_or(-1, |i| i as i32));
    }
}

/// Rebuilds the file list; optionally selects the item named `select`.
fn refresh_files(ui: &AppWindow, select: Option<&str>) {
    let (rows, total, selected_row) = with_app(|a| {
        let Some(listing) = &a.listing else { return (Vec::new(), 0, None) };
        let needle = a.filter.to_lowercase();
        let order = if matches!(listing.location, Location::Search { .. } | Location::History) {
            (0..listing.items.len()).collect() // keep relevance order
        } else {
            browser::sort_order(&listing.items, a.sort_column, a.sort_descending, |i| {
                badges::type_name(&i.name, i.kind)
            })
        };
        a.visible = order
            .into_iter()
            .filter(|&i| needle.is_empty() || listing.items[i].name.to_lowercase().contains(&needle))
            .collect();
        if let Some(name) = select {
            a.selected = listing.items.iter().position(|i| i.name.eq_ignore_ascii_case(name));
        }
        let rows: Vec<FileRow> = a.visible.iter().map(|&i| file_row(&listing.items[i])).collect();
        let selected_row = a.selected.and_then(|s| a.visible.iter().position(|&v| v == s));
        (rows, listing.items.len(), selected_row)
    });
    ui.set_files(ModelRc::new(VecModel::from(rows)));
    ui.set_total_count(total as i32);
    ui.set_sort_column(with_app(|a| a.sort_column));
    ui.set_sort_descending(with_app(|a| a.sort_descending));
    ui.set_file_selected(selected_row.map_or(-1, |r| r as i32));
    ui.set_can_extract(with_app(|a| {
        matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Archive(..) | Location::Textures { .. }))
            || a.selected.is_some()
    }));
    let texture_mode = with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Textures { .. })));
    ui.set_texture_mode(texture_mode);
    ui.set_texture_selected(texture_mode && selected_row.is_some());
    let history_mode = with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::History)));
    ui.set_history_mode(history_mode);
    if !history_mode {
        ui.set_history_group(SharedString::new());
    }
    let archive_mode = with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Archive(..))));
    ui.set_in_mods(with_app(|a| a.listing.as_ref().is_some_and(|l| l.origin != Origin::Base)));
    ui.set_archive_mode(archive_mode);
    ui.set_entry_selected(archive_mode && selected_row.is_some());
    if let (Some(row), Some(_)) = (selected_row, select) {
        select_row(ui, row);
    }
}

/// Mod status as the UI's mark index (see `Theme.mark-color`).
fn mark_index(mark: Option<Mark>) -> i32 {
    match mark {
        None => 0,
        Some(Mark::Added | Mark::New) => 1,
        Some(Mark::Changed | Mark::Replaced) => 2,
        Some(Mark::ContainsMods) => 3,
        Some(Mark::Removed) => 4,
    }
}

fn is_model_item(item: &Item) -> bool {
    item.kind == ItemKind::Resource && openrage_app::models::is_model(&item.name) && item.mark != Some(Mark::Removed)
}

/// Items with a large view: models, textures and images.
fn is_viewable(item: &Item) -> bool {
    let lower = item.name.to_ascii_lowercase();
    is_model_item(item) || item.kind == ItemKind::Texture || [".ytd", ".dds", ".png"].iter().any(|e| lower.ends_with(e))
}

/// Double-click navigates into it (or, for search hits and changes, to it).
fn is_openable(item: &Item) -> bool {
    item.vpath.is_some()
        || item.kind.is_container()
        || (item.kind == ItemKind::Resource && browser::is_texture_dict(&item.name))
}

fn file_row(item: &Item) -> FileRow {
    let (icon, tint) = badges::icon(&item.name, item.kind);
    let info = match (item.mark, item.info.is_empty()) {
        (Some(m), true) => m.label().to_owned(),
        (Some(m), false) if item.vpath.is_none() => format!("{} · {}", m.label(), item.info),
        _ => item.info.clone(),
    };
    FileRow {
        mark: mark_index(item.mark),
        dim: item.mark == Some(Mark::Removed),
        name: item.name.clone().into(),
        icon: icon.into(),
        tint,
        can_open: is_openable(item),
        can_view: is_viewable(item),
        is_model: is_model_item(item),
        kind: match &item.vpath {
            Some(_) if item.detail.is_some() => item.detail.clone().unwrap_or_default().into(),
            Some(vpath) => browser::short_location(&browser::parent(vpath).unwrap_or_default(), 40).into(),
            None => badges::type_name(&item.name, item.kind).into(),
        },
        size: if item.kind.is_container() && item.size == 0 {
            SharedString::new()
        } else {
            human_size(item.size).into()
        },
        info: info.into(),
    }
}

/// Shows `vpath` in the address bar, as text and as clickable breadcrumbs.
fn set_location(ui: &AppWindow, vpath: &str) {
    ui.set_address(vpath.into());
    ui.set_address_editing(false);
    ui.set_crumbs(ModelRc::new(VecModel::from(crumbs(vpath))));
}

fn crumbs(vpath: &str) -> Vec<Crumb> {
    let crumb = |label: &str, path: &str| Crumb { label: label.into(), path: path.into() };
    let mut out = vec![crumb("Game", "")];
    if let Some(q) = browser::search_query(vpath) {
        out.push(crumb(&format!("Search: {q}"), vpath));
        return out;
    }
    match vpath.trim() {
        browser::CHANGES_PATH => out.push(crumb("Mod changes", vpath)),
        browser::HISTORY_PATH => out.push(crumb("History", vpath)),
        _ => {
            let mut acc = String::new();
            for part in vpath.split('/').filter(|p| !p.is_empty()) {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(part);
                out.push(crumb(part, &acc));
            }
        }
    }
    // Deep paths: keep the root and the last levels; "…" goes to the hidden parent.
    const KEEP: usize = 5;
    if out.len() > KEEP + 2 {
        let hidden = out[out.len() - KEEP - 1].path.clone();
        let tail = out.split_off(out.len() - KEEP);
        out.truncate(1);
        out.push(crumb("…", &hidden));
        out.extend(tail);
    }
    out
}

/// Heading of the file list: the folder's name, or what the list shows.
fn location_title(l: &Listing) -> String {
    match &l.location {
        Location::Search { query, .. } => format!("Search: {query}"),
        Location::Changes { .. } => "Mod changes".into(),
        Location::History => "History".into(),
        _ => l.vpath.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("Game folder").to_owned(),
    }
}

/// Fills the info panel header and metadata table.
fn set_info(ui: &AppWindow, title: &str, path: &str, meta: &Meta, icon: (&str, slint::Color)) {
    ui.set_info_title(title.into());
    ui.set_info_path(path.into());
    set_meta(ui, meta);
    ui.set_info_icon(icon.0.into());
    ui.set_info_tint(icon.1);
}

fn set_meta(ui: &AppWindow, meta: &Meta) {
    let rows: Vec<InfoRow> = meta.iter().map(|(k, v)| InfoRow { key: k.into(), value: v.into() }).collect();
    ui.set_info_rows(ModelRc::new(VecModel::from(rows)));
}

/// `13379` → `13,379`.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn show_folder_info(ui: &AppWindow) {
    let info = with_app(|a| {
        let l = a.listing.as_ref()?;
        let name = location_title(l);
        let folder_icon = if l.vpath.to_ascii_lowercase().ends_with(".rpf") {
            badges::icon(&name, ItemKind::Archive)
        } else {
            badges::icon(&name, ItemKind::Dir)
        };
        let place = match &l.location {
            Location::Search { query, summary } => {
                return Some((
                    format!("Search: {query}"),
                    String::new(),
                    vec![note(summary)],
                    ("search", badges::NEUTRAL),
                ));
            }
            Location::Changes { summary } => {
                return Some((
                    "Mod changes".into(),
                    String::new(),
                    vec![note(summary)],
                    badges::icon("", ItemKind::Dir),
                ));
            }
            Location::History => {
                return Some((
                    "History".into(),
                    String::new(),
                    vec![note(
                        "Every change OpenRAGE made to the mods folder. Select a package entry to uninstall the whole package.",
                    )],
                    ("history", badges::NEUTRAL),
                ));
            }
            Location::Textures { dict, .. } => {
                return Some((
                    name.clone(),
                    l.vpath.clone(),
                    vec![
                        kv("Type", "Texture dictionary"),
                        kv("Textures", dict.textures.len()),
                        note("Extract… exports DDS files like OpenIV; Save PNG… exports images."),
                    ],
                    badges::icon(&name, ItemKind::Resource),
                ));
            }
            Location::Disk(p) => vec![kv("Type", "Folder"), kv("On disk", p.display())],
            Location::Archive(rpf, _) => vec![
                kv("Type", "Inside an archive"),
                kv("Archive", rpf.name()),
                kv("Encryption", format!("{:?}", rpf.encryption())),
                kv("Entries", grouped(rpf.entries().len())),
            ],
        };
        let mut meta = place;
        match l.origin {
            Origin::Mods => meta.push(kv("Source", "mods folder (replaces the game's version)")),
            Origin::Merged => meta.push(kv("Source", "game folder merged with its mods counterpart")),
            Origin::Base => {}
        }
        Some((name, l.vpath.clone(), meta, folder_icon))
    });
    let (title, path, meta, icon) =
        info.unwrap_or_else(|| (String::new(), String::new(), Meta::new(), ("folder", slint::Color::default())));
    ui.set_location_title(title.clone().into());
    set_info(ui, &title, &path, &meta, icon);
    ui.set_preview_text(SharedString::new());
    ui.set_has_image(false);
    ui.set_model_mode(false);
    ui.set_preview_loading(false);
}

// ---------------------------------------------------------------------------
// Selection + preview

/// What the preview worker needs to load an item off the UI thread.
enum PreviewSource {
    DiskFile(PathBuf),
    DiskArchive(PathBuf),
    Entry(Arc<Rpf7>, EntryId),
    /// A texture of an opened texture dictionary (index into its textures).
    Texture(Arc<rage_formats::ytd::TextureDictionary>, usize),
    /// A search hit; resolved in the worker. `query` is set for text hits to show the matching line.
    Virtual {
        game_dir: PathBuf,
        vpath: String,
        query: Option<String>,
        /// Resolve through the mods overlay.
        overlay: bool,
    },
}

fn select_row(ui: &AppWindow, row: usize) {
    let Some((item, vpath, source, keys, generation)) = with_app(|a| {
        let listing = a.listing.as_ref()?;
        let idx = *a.visible.get(row)?;
        a.selected = Some(idx);
        let item = listing.items[idx].clone();
        let source = match (&listing.location, item.entry) {
            (Location::History, _) => None,
            (_, None) if item.mark == Some(Mark::Removed) => {
                // Removed by the mod: show the original.
                let vpath = item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name));
                a.game_dir.clone().map(|game_dir| PreviewSource::Virtual {
                    game_dir,
                    vpath,
                    query: None,
                    overlay: false,
                })
            }
            (Location::Changes { .. }, _) => {
                let vpath = item.vpath.clone().unwrap_or_default();
                a.game_dir.clone().map(|game_dir| PreviewSource::Virtual {
                    game_dir,
                    vpath,
                    query: None,
                    overlay: true,
                })
            }
            (Location::Search { query, .. }, _) => {
                let vpath = item.vpath.clone().unwrap_or_default();
                let query = (item.info == "text").then(|| match rage_index::parse_hash(query) {
                    Some(h) => format!("0x{h:08X}"),
                    None => query.clone(),
                });
                a.game_dir.clone().map(|game_dir| PreviewSource::Virtual { game_dir, vpath, query, overlay: false })
            }
            (Location::Textures { dict, .. }, _) => Some(PreviewSource::Texture(Arc::clone(dict), idx)),
            (Location::Disk(dir), _) if item.kind == ItemKind::Archive => {
                Some(PreviewSource::DiskArchive(dir.join(&item.name)))
            }
            (Location::Disk(dir), _) if item.kind == ItemKind::File => {
                Some(PreviewSource::DiskFile(dir.join(&item.name)))
            }
            (Location::Archive(rpf, _), Some(id)) if item.kind != ItemKind::Dir => {
                Some(PreviewSource::Entry(Arc::clone(rpf), id))
            }
            _ => None,
        };
        let generation = a.preview_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let vpath = item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name));
        Some((item, vpath, source, a.keys.clone(), generation))
    }) else {
        return;
    };
    ui.set_file_selected(row as i32);
    with_app(|a| a.pending_viewer = false);
    ui.set_can_extract(true);
    ui.set_texture_selected(item.kind == ItemKind::Texture);
    if with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::History))) {
        ui.set_history_group(item.info.clone().into());
    }
    ui.set_entry_selected(item.entry.is_some());

    let mut info = vec![kv("Type", badges::type_name(&item.name, item.kind)), kv("Size", human_size(item.size))];
    if !item.info.is_empty() {
        // Resources show their version ("v171"); other items a short note.
        let is_version = item.info.strip_prefix('v').is_some_and(|v| v.chars().all(|c| c.is_ascii_digit()));
        info.push(kv(if is_version { "Version" } else { "Details" }, &item.info));
    }
    set_info(ui, &item.name, &vpath, &info, badges::icon(&item.name, item.kind));
    ui.set_preview_text(SharedString::new());
    ui.set_has_image(false);
    ui.set_model_mode(false);
    with_app(|a| a.model = None);
    if is_model_item(&item) {
        start_model_preview(ui, vpath.clone(), info, generation);
        return;
    }
    let Some(source) = source else {
        ui.set_preview_loading(false);
        return;
    };
    ui.set_preview_loading(true);

    let counter = with_app(|a| Arc::clone(&a.preview_generation));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let (extra, body, image) = load_preview(source, keys);
        if counter.load(Ordering::SeqCst) != generation {
            return; // a newer selection superseded this one
        }
        let image =
            image.map(|(w, h, rgba)| slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h));
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if counter.load(Ordering::SeqCst) != generation {
                return;
            }
            if !extra.is_empty() {
                set_meta(&ui, &info.iter().cloned().chain(extra).collect());
            }
            ui.set_preview_text(body.into());
            match image {
                Some(buffer) => {
                    ui.set_preview_image(slint::Image::from_rgba8(buffer));
                    ui.set_has_image(true);
                    if with_app(|a| std::mem::take(&mut a.pending_viewer)) {
                        open_viewer(&ui);
                    }
                }
                None => ui.set_has_image(false),
            }
            ui.set_preview_loading(false);
        });
    });
}

/// Decoded RGBA image: (width, height, pixels).
type Rgba = (u32, u32, Vec<u8>);

/// Metadata shown as a key/value table; an empty key is a note spanning the row.
type Meta = Vec<(String, String)>;

fn kv(key: &str, value: impl ToString) -> (String, String) {
    (key.to_owned(), value.to_string())
}

fn note(text: impl ToString) -> (String, String) {
    (String::new(), text.to_string())
}

fn texture_info(t: &rage_formats::texture::Texture) -> Meta {
    vec![
        kv("Dimensions", format!("{} × {}", t.width, t.height)),
        kv("Format", t.format.name()),
        kv("Mip levels", t.levels),
        kv("Usage", t.usage_name()),
        kv("Data", human_size(t.data.len() as u64)),
    ]
}

fn texture_image(t: &rage_formats::texture::Texture) -> Option<Rgba> {
    t.to_rgba().map(|rgba| (t.width, t.height, rgba))
}

/// Preview with an optional image: textures and texture dictionaries show a picture.
fn load_preview(source: PreviewSource, keys: Option<Arc<GtaVKeys>>) -> (Meta, String, Option<Rgba>) {
    match source {
        PreviewSource::Texture(dict, idx) => match dict.textures.get(idx) {
            Some(t) => {
                let image = texture_image(t);
                let mut meta = texture_info(t);
                if image.is_none() {
                    meta.push(note("No preview decoder for this format (DDS export works)."));
                }
                (meta, String::new(), image)
            }
            None => (Meta::new(), String::new(), None),
        },
        PreviewSource::Entry(rpf, id) if browser::is_texture_dict(&rpf.entry(id).name) => {
            match rpf
                .read_resource(id)
                .map_err(|e| e.to_string())
                .and_then(|r| rage_formats::ytd::parse(&r).map_err(|e| e.to_string()))
            {
                Ok(dict) => {
                    let names: Vec<&str> = dict.textures.iter().take(8).map(|t| t.name.as_str()).collect();
                    let more = if dict.textures.len() > 8 { ", …" } else { "" };
                    let meta = vec![
                        kv("Textures", dict.textures.len()),
                        kv("Contains", format!("{}{more}", names.join(", "))),
                        note("Double-click to open the dictionary."),
                    ];
                    (meta, String::new(), dict.textures.first().and_then(texture_image))
                }
                Err(e) => (vec![kv("Error", e)], String::new(), None),
            }
        }
        other => {
            let (meta, body) = load_preview_text(other, keys);
            (meta, body, None)
        }
    }
}

/// Returns (metadata, preview body).
fn load_preview_text(source: PreviewSource, keys: Option<Arc<GtaVKeys>>) -> (Meta, String) {
    match source {
        PreviewSource::Virtual { game_dir, vpath, query, overlay } => match if overlay {
            rage_mods::resolve(&game_dir, &vpath, keys.clone()).map(|(n, _)| n)
        } else {
            vfs::resolve(&game_dir, &vpath, keys.clone())
        } {
            Ok(Node::ArchiveFile(rpf, id)) => {
                let (mut meta, body) = load_preview_text(PreviewSource::Entry(Arc::clone(&rpf), id), keys);
                if let Some(q) = query {
                    let name = rpf.entry(id).name.clone();
                    let line = rpf
                        .read(id)
                        .ok()
                        .and_then(|d| rage_index::content_for(&name, &d))
                        .and_then(|(text, _)| rage_index::snippet(&text, &q));
                    if let Some(line) = line {
                        meta.insert(0, kv("Match", line));
                    }
                }
                (meta, body)
            }
            Ok(Node::ArchiveDir(rpf, id)) if id == Rpf7::ROOT => (archive_info(&rpf), String::new()),
            Ok(Node::ArchiveDir(..)) | Ok(Node::Dir(_)) => (Meta::new(), String::new()),
            Ok(Node::PlainFile(p)) => load_preview_text(PreviewSource::DiskFile(p), keys),
            Err(e) => (vec![kv("Error", e)], String::new()),
        },
        PreviewSource::Texture(..) => (Meta::new(), String::new()),
        PreviewSource::DiskFile(path) => {
            let mut data = Vec::new();
            match File::open(&path).and_then(|f| f.take(1 << 20).read_to_end(&mut data)) {
                Ok(_) => (Meta::new(), preview::render(&data)),
                Err(e) => (vec![kv("Error", e)], String::new()),
            }
        }
        PreviewSource::DiskArchive(path) => match Rpf7::open(&path, keys) {
            Ok(rpf) => (archive_info(&rpf), String::new()),
            Err(e) => (vec![kv("Error", e)], String::new()),
        },
        PreviewSource::Entry(rpf, id) => {
            let entry = rpf.entry(id).clone();
            if entry.is_archive() {
                return match rpf.open_child(id) {
                    Ok(child) => (archive_info(&child), String::new()),
                    Err(e) => (vec![kv("Error", e)], String::new()),
                };
            }
            match entry.kind {
                EntryKind::Resource { size, .. } => {
                    let (sys, gfx) = entry.resource_page_sizes().unwrap_or_default();
                    let mut meta = vec![
                        kv("Resource version", entry.resource_version().unwrap_or_default()),
                        kv("Stored", human_size(size as u64)),
                        kv("System pages", human_size(sys)),
                        kv("Graphics pages", human_size(gfx)),
                    ];
                    match rpf.read_resource_pages(id) {
                        Ok(pages) => (meta, preview::hex_dump(&pages)),
                        Err(e) => {
                            meta.push(kv("Error", e));
                            (meta, String::new())
                        }
                    }
                }
                EntryKind::Binary { stored_size, size, encrypted, .. } => {
                    let mut meta = Meta::new();
                    if stored_size > 0 {
                        meta.push(kv(
                            "Compressed",
                            format!("{} → {}", human_size(stored_size as u64), human_size(size as u64)),
                        ));
                    }
                    if encrypted {
                        meta.push(kv("Encrypted", "yes"));
                    }
                    match rpf.read(id) {
                        Ok(data) => (meta, preview::render(&data)),
                        Err(e) => (vec![kv("Error", e)], String::new()),
                    }
                }
                EntryKind::Directory { .. } => (Meta::new(), String::new()),
            }
        }
    }
}

fn archive_info(rpf: &Rpf7) -> Meta {
    let (mut dirs, mut files, mut resources, mut archives) = (0, 0, 0, 0);
    for e in rpf.entries() {
        match e.kind_label() {
            "dir" => dirs += 1,
            "rpf" => archives += 1,
            "resource" => resources += 1,
            _ => files += 1,
        }
    }
    vec![
        kv("Format", "RPF7"),
        kv("Encryption", format!("{:?}", rpf.encryption())),
        kv("Folders", dirs),
        kv("Files", files),
        kv("Resources", resources),
        kv("Nested archives", archives),
    ]
}

// ---------------------------------------------------------------------------
// Search index

fn start_index_update(ui: &AppWindow) {
    let Some((game_dir, keys)) = with_app(|a| {
        if a.index_running {
            return None;
        }
        a.index_running = true;
        a.game_dir.clone().map(|d| (d, a.keys.clone()))
    }) else {
        return;
    };
    let weak = ui.as_weak();
    ui.set_index_status("Index: checking…".into());
    std::thread::spawn(move || {
        let status = |text: String| {
            let _ = weak.upgrade_in_event_loop(move |ui| ui.set_index_status(text.into()));
        };
        let result = (|| -> Result<rage_index::IndexStats, rage_index::IndexError> {
            let path = index_path(&game_dir).ok_or_else(|| std::io::Error::other("no cache directory"))?;
            let mut index = Index::open(&path)?;
            let mut last = Instant::now();
            index.update(&game_dir, keys, &AtomicBool::new(false), &mut |p| {
                if last.elapsed() > Duration::from_millis(200) {
                    last = Instant::now();
                    status(format!("Indexing {}/{} archives…", p.archives_done, p.archives_total));
                }
            })?;
            index.stats()
        })();
        let text = match result {
            Ok(s) => format!("Index: {} entries", s.entries),
            Err(e) => format!("Index error: {e}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            with_app(|a| a.index_running = false);
            ui.set_index_status(text.into());
        });
    });
}

fn start_search(ui: &AppWindow, query: String, record: bool) {
    let Some((game_dir, language, generation, counter)) = with_app(|a| {
        let generation = a.search_generation.fetch_add(1, Ordering::SeqCst) + 1;
        a.game_dir.clone().map(|d| (d, a.language.clone(), generation, Arc::clone(&a.search_generation)))
    }) else {
        return;
    };
    ui.set_status(format!("Searching for \"{query}\"…").into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = index_path(&game_dir)
            .ok_or_else(|| "no cache directory".to_owned())
            .and_then(|p| Index::open(&p).map_err(|e| e.to_string()))
            .and_then(|idx| match idx.stats() {
                Ok(s) if s.updated_at.is_none() => {
                    Err("the search index is still being built — please try again in a moment".to_owned())
                }
                _ => idx.search(&query, 1000, language.as_deref()).map_err(|e| e.to_string()),
            });
        let ms = started.elapsed().as_millis();
        if counter.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |ui| match result {
            Ok(results) => show_search_results(&ui, query, results, ms, record),
            Err(e) => ui.set_status(format!("Search failed: {e}").into()),
        });
    });
}

fn show_search_results(ui: &AppWindow, query: String, results: rage_index::SearchResults, ms: u128, record: bool) {
    let items: Vec<Item> = results
        .hits
        .iter()
        .map(|h| Item {
            name: h.name.clone(),
            kind: match h.kind {
                EntryTag::Dir => ItemKind::Dir,
                EntryTag::Archive => ItemKind::Archive,
                EntryTag::File => ItemKind::File,
                EntryTag::Resource => ItemKind::Resource,
            },
            size: h.size,
            info: match h.reason {
                HitReason::Name => "name",
                HitReason::NameHash => "hash",
                HitReason::Content => "text",
            }
            .into(),
            entry: None,
            vpath: Some(h.vpath.clone()),
            mark: None,
            detail: None,
        })
        .collect();
    let mut summary = match results.hash {
        Some(hash) => {
            let names = if results.dictionary.is_empty() {
                "no known name".to_owned()
            } else {
                results.dictionary.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ")
            };
            format!("Hash 0x{hash:08X} ({hash} / {}) = {names}", hash as i32)
        }
        None => format!("\"{query}\" · joaat 0x{:08X} ({})", results.query_hash, results.query_hash as i32),
    };
    summary.push_str(&format!(
        " · {} hits{} in {ms} ms",
        items.len(),
        if results.limited { " (first 1000)" } else { "" }
    ));
    let vpath = format!("?{query}");
    with_app(|a| {
        if record {
            a.history.push(&vpath);
        }
        a.listing = Some(Listing {
            vpath: vpath.clone(),
            origin: Origin::Base,
            location: Location::Search { query: query.clone(), summary: summary.clone() },
            items,
        });
        a.selected = None;
        a.filter.clear();
    });
    ui.set_filter(SharedString::new());
    set_location(ui, &vpath);
    ui.set_search_mode(true);
    ui.set_show_language(true);
    ui.set_search_summary(summary.into());
    ui.set_tree_selected(-1);
    refresh_files(ui, None);
    show_folder_info(ui);
    ui.set_status(SharedString::new());
    update_nav_buttons(ui);
}

// ---------------------------------------------------------------------------
// Mods

/// Marks the entries of a folder inside a modded archive (added / changed /
/// removed relative to the original). Runs in the background.
fn start_folder_diff(ui: &AppWindow) {
    let Some((vpath, rpf, dir, game_dir, keys, generation, counter)) = with_app(|a| {
        let l = a.listing.as_ref()?;
        let Location::Archive(rpf, dir) = &l.location else { return None };
        if !a.overlay || l.origin != Origin::Mods {
            return None;
        }
        let generation = a.diff_generation.fetch_add(1, Ordering::SeqCst) + 1;
        Some((
            l.vpath.clone(),
            Arc::clone(rpf),
            *dir,
            a.game_dir.clone()?,
            a.keys.clone(),
            generation,
            Arc::clone(&a.diff_generation),
        ))
    }) else {
        return;
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let diff = match vfs::resolve(&game_dir, &vpath, keys) {
            Ok(Node::ArchiveDir(base, bdir)) => {
                Some(rage_mods::compare_folders(&rpf, dir, &base, bdir, CompareMode::Quick))
            }
            _ => None, // not in the game at all: everything here is new
        };
        if counter.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if counter.load(Ordering::SeqCst) != generation {
                return;
            }
            let applied = with_app(|a| {
                let l = a.listing.as_mut().filter(|l| l.vpath == vpath)?;
                match &diff {
                    Some(d) => {
                        for item in &mut l.items {
                            item.mark = match d.marks.get(&item.name.to_ascii_lowercase()) {
                                Some(rage_mods::Mark::Added) => Some(Mark::Added),
                                Some(rage_mods::Mark::Changed) => Some(Mark::Changed),
                                _ => None,
                            };
                        }
                        for r in &d.removed {
                            l.items.push(Item {
                                name: r.name.clone(),
                                kind: if r.is_archive {
                                    ItemKind::Archive
                                } else if r.is_dir {
                                    ItemKind::Dir
                                } else {
                                    ItemKind::File
                                },
                                size: r.size,
                                info: String::new(),
                                entry: None,
                                vpath: None,
                                mark: Some(Mark::Removed),
                                detail: None,
                            });
                        }
                    }
                    None => l.items.iter_mut().for_each(|i| i.mark = Some(Mark::New)),
                }
                Some(a.selected.map(|s| l.items[s].name.clone()))
            });
            if let Some(selected) = applied {
                refresh_files(&ui, None);
                if let Some(name) = selected {
                    let row = with_app(|a| {
                        let l = a.listing.as_ref()?;
                        a.visible.iter().position(|&i| l.items[i].name == name)
                    });
                    ui.set_file_selected(row.map_or(-1, |r| r as i32));
                }
            }
        });
    });
}

fn start_changes(ui: &AppWindow, record: bool) {
    let Some((game_dir, keys)) = with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone()))) else { return };
    if rage_mods::mods_dir(&game_dir).is_none() {
        ui.set_status("This game has no mods folder.".into());
        return;
    }
    ui.set_status("Comparing mods with the original files…".into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut last = Instant::now();
        let changes = rage_mods::list_changes(&game_dir, keys, CompareMode::Quick, &mut |i, n, p| {
            if last.elapsed() > Duration::from_millis(150) {
                last = Instant::now();
                let msg = format!("Comparing {}/{n}: {p}", i + 1);
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
            }
        });
        let ms = started.elapsed().as_millis();
        let _ = weak.upgrade_in_event_loop(move |ui| show_changes(&ui, changes, ms, record));
    });
}

fn show_changes(ui: &AppWindow, changes: Vec<rage_mods::Change>, ms: u128, record: bool) {
    let files = with_app(|a| a.game_dir.as_deref().map(rage_mods::scan).unwrap_or_default());
    let items: Vec<Item> = changes
        .iter()
        .map(|c| Item {
            name: c.vpath.rsplit('/').next().unwrap_or_default().to_owned(),
            kind: if c.is_dir {
                ItemKind::Dir
            } else if c.vpath.to_ascii_lowercase().ends_with(".rpf") {
                ItemKind::Archive
            } else {
                ItemKind::File
            },
            size: c.size,
            info: c.kind.label().into(),
            entry: None,
            vpath: Some(c.vpath.clone()),
            mark: Some(match c.kind {
                ChangeKind::Added => Mark::Added,
                ChangeKind::NewFile => Mark::New,
                ChangeKind::Removed => Mark::Removed,
                ChangeKind::Modified => Mark::Changed,
                ChangeKind::ReplacedFile | ChangeKind::IdenticalCopy => Mark::Replaced,
            }),
            detail: None,
        })
        .collect();
    let identical = changes.iter().filter(|c| c.kind == ChangeKind::IdenticalCopy).count();
    let mut summary = format!("{} changes from {} files in the mods folder · {ms} ms", items.len(), files.len());
    if identical > 0 {
        let noun = if identical == 1 { "copy" } else { "copies" };
        summary.push_str(&format!(" · {identical} unchanged {noun} of game files (can be deleted)"));
    }
    let vpath = browser::CHANGES_PATH.to_owned();
    with_app(|a| {
        if record {
            a.history.push(&vpath);
        }
        a.listing = Some(Listing {
            vpath: vpath.clone(),
            origin: Origin::Base,
            location: Location::Changes { summary: summary.clone() },
            items,
        });
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
    ui.set_status(SharedString::new());
    update_nav_buttons(ui);
}

// ---------------------------------------------------------------------------
// Extraction

/// Exports the selected texture, or all textures of the open dictionary, as DDS or PNG.
fn export_textures(ui: &AppWindow, png: bool) {
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
enum Prompt {
    Rename { vpath: String },
    NewFolder { dir: String },
}

/// Virtual path and name of the selected entry in an archive listing.
fn selected_entry() -> Option<(String, String)> {
    with_app(|a| {
        let l = a.listing.as_ref()?;
        if !matches!(l.location, Location::Archive(..)) {
            return None;
        }
        let item = &l.items[a.selected?];
        item.entry?;
        Some((browser::join(&l.vpath, &item.name), item.name.clone()))
    })
}

fn open_prompt(ui: &AppWindow, prompt: Prompt, title: &str, value: &str) {
    with_app(|a| a.prompt = Some(prompt));
    ui.set_prompt_title(title.into());
    ui.set_prompt_value(value.into());
    ui.set_prompt_visible(true);
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\', ':']) && name != "." && name != ".."
}

fn prompt_accepted(ui: &AppWindow, text: &str) {
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
enum OpTarget {
    Op(Op),
    Rename(String),
}

/// Applies operations (grouped per top-level archive) in the background, then refreshes the view.
fn run_archive_ops(ui: &AppWindow, ops: Vec<(String, OpTarget)>, description: String, select: Option<String>) {
    let Some((game_dir, keys, current)) =
        with_app(|a| Some((a.game_dir.clone()?, a.keys.clone(), a.listing.as_ref()?.vpath.clone())))
    else {
        return;
    };
    set_busy(ui, true, "Writing into the mods folder…".into());
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
                    navigate(&ui, &target, false);
                    if select.is_some() && with_app(|a| a.selected.is_none()) {
                        navigate(&ui, &current, false);
                    }
                    refresh_undo(&ui);
                    let mut msg = format!("{description} — {:.2}s", started.elapsed().as_secs_f32());
                    if applied.created {
                        msg.push_str(" (archive copied into the mods folder first)");
                    }
                    ui.set_status(msg.into());
                }
                Err(e) => ui.set_status(format!("{description} failed: {e:#}").into()),
            }
        });
    });
}

fn archive_import(ui: &AppWindow, dropped: Option<Vec<PathBuf>>) {
    let Some(dir) = with_app(|a| {
        let l = a.listing.as_ref()?;
        matches!(l.location, Location::Archive(..)).then(|| l.vpath.clone())
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

fn archive_replace(ui: &AppWindow) {
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

fn archive_delete(ui: &AppWindow) {
    let Some((vpath, name)) = selected_entry() else { return };
    run_archive_ops(ui, vec![(vpath, OpTarget::Op(Op::Remove))], format!("Deleted {name}"), None);
}

/// Shows or hides the Undo button with the newest journal entry.
fn refresh_undo(ui: &AppWindow) {
    let newest = with_app(|a| a.game_dir.clone())
        .and_then(|d| rage_mods::mods_dir(&d))
        .and_then(|m| openrage_app::journal::Journal::new(&m).entries().ok())
        .and_then(|e| e.into_iter().next());
    ui.set_can_undo(newest.is_some());
    ui.set_undo_label(newest.map(|e| e.description).unwrap_or_default().into());
}

fn start_undo(ui: &AppWindow) {
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
                navigate(&ui, &target, false);
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

// ---------------------------------------------------------------------------
// Model preview

fn start_model_preview(ui: &AppWindow, vpath: String, info: Meta, generation: u64) {
    let Some((game_dir, keys, counter)) =
        with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone(), Arc::clone(&a.preview_generation))))
    else {
        return;
    };
    ui.set_preview_loading(true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = openrage_app::models::load(&game_dir, keys, &vpath).map(|loaded| {
            let materials = openrage_app::render::Materials::new(&loaded);
            let name = loaded.drawables.first().map(|(n, _)| n.clone()).unwrap_or_default();
            ModelPreview { loaded, materials, name }
        });
        if counter.load(Ordering::SeqCst) != generation {
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if counter.load(Ordering::SeqCst) != generation {
                return;
            }
            ui.set_preview_loading(false);
            match result {
                Ok(m) => {
                    let mut meta = info.clone();
                    let l = &m.loaded;
                    let (mut meshes, mut shaders, mut textured) = (0, 0, 0);
                    for (_, d) in l.visible_parts() {
                        meshes += d.meshes.len();
                        shaders += d.shaders.len();
                        textured += d
                            .shaders
                            .iter()
                            .filter(|s| s.base_texture().is_some_and(|t| l.texture(d, t).is_some()))
                            .count();
                    }
                    let shown = l.parts.iter().filter(|p| p.visible).count();
                    if l.parts.len() > 1 {
                        meta.push(kv("Parts", format!("{shown} shown of {}", l.parts.len())));
                    }
                    meta.push(kv("Meshes", meshes));
                    meta.push(kv("Vertices", grouped(l.vertex_count())));
                    meta.push(kv("Triangles", grouped(l.triangle_count())));
                    meta.push(kv("Textured", format!("{textured} of {shaders} shaders")));
                    set_meta(&ui, &meta);
                    with_app(|a| {
                        a.model = Some(Arc::new(m));
                        a.model_view = openrage_app::render::View::default();
                    });
                    ui.set_model_mode(true);
                    render_model(&ui);
                    if with_app(|a| std::mem::take(&mut a.pending_viewer)) {
                        open_viewer(&ui);
                    }
                }
                Err(e) => {
                    let mut meta = info.clone();
                    meta.push(kv("Error", format!("Cannot read the model: {e:#}")));
                    set_meta(&ui, &meta);
                }
            }
        });
    });
}

/// Renders a model into an image of `w × h` pixels.
fn render_image(model: &ModelPreview, view: openrage_app::render::View, w: usize, h: usize) -> Option<slint::Image> {
    let rgba = openrage_app::render::render(&model.loaded, &model.materials, view, w, h);
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w as u32, h as u32);
    Some(slint::Image::from_rgba8(buffer))
}

/// Renders the current model at the size of the preview area.
fn render_model(ui: &AppWindow) {
    let Some((model, view)) = with_app(|a| a.model.clone().map(|m| (m, a.model_view))) else { return };
    let w = (ui.get_preview_area_width() - 16.0).max(64.0) as usize;
    let h = (ui.get_preview_area_height() - 16.0).max(64.0) as usize;
    if let Some(img) = render_image(&model, view, w, h) {
        ui.set_preview_image(img);
        ui.set_has_image(true);
    }
}

/// Opens the selected model or image in its own window (once it has loaded).
fn open_viewer(ui: &AppWindow) {
    if ui.get_model_mode() {
        if let Some(m) = with_app(|a| a.model.clone()) {
            viewer::open(ui, viewer::Content::Model(m));
            return;
        }
    } else if ui.get_has_image() {
        viewer::open(ui, viewer::Content::Image(ui.get_preview_image()));
        return;
    }
    if ui.get_preview_loading() {
        with_app(|a| a.pending_viewer = true);
        ui.set_status("The viewer opens when the preview has loaded…".into());
    }
}

fn export_gltf(ui: &AppWindow) {
    if let Some(model) = with_app(|a| a.model.clone()) {
        save_glb(model, ui.as_weak());
    }
}

/// Asks for a file name and writes the model as `.glb`; reports in the main window.
fn save_glb(model: Arc<ModelPreview>, main: Weak<AppWindow>) {
    std::thread::spawn(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export glTF")
            .set_file_name(format!("{}.glb", model.name))
            .add_filter("glTF binary", &["glb"])
            .save_file()
        else {
            return;
        };
        let result = openrage_app::models::to_glb(&model.loaded, &model.name)
            .ok_or_else(|| "nothing visible to export".to_owned())
            .and_then(|glb| std::fs::write(&path, glb).map_err(|e| e.to_string()));
        let msg = match result {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
        let _ = main.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

/// Asks for a file name and saves an image shown in a viewer as PNG.
fn save_png_image(img: &slint::Image, name: String, main: Weak<AppWindow>) {
    let Some(pixels) = img.to_rgba8() else { return };
    let (w, h) = (pixels.width(), pixels.height());
    let rgba = pixels.as_bytes().to_vec();
    std::thread::spawn(move || {
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s).to_owned();
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save PNG")
            .set_file_name(format!("{stem}.png"))
            .add_filter("PNG image", &["png"])
            .save_file()
        else {
            return;
        };
        let result = rage_formats::png(w, h, &rgba)
            .map_err(|e| e.to_string())
            .and_then(|png| std::fs::write(&path, png).map_err(|e| e.to_string()));
        let msg = match result {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Saving failed: {e}"),
        };
        let _ = main.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
    });
}

// ---------------------------------------------------------------------------
// History and OpenIV packages

fn show_history(ui: &AppWindow, record: bool) {
    let entries = with_app(|a| a.game_dir.clone())
        .and_then(|d| rage_mods::mods_dir(&d))
        .and_then(|m| openrage_app::journal::Journal::new(&m).entries().ok())
        .unwrap_or_default();
    let items: Vec<Item> = entries
        .iter()
        .map(|e| {
            use openrage_app::journal::Kind;
            let place = match e.kind {
                Kind::FileWritten { in_game: true, .. } => format!("game folder: {}", e.archive),
                _ => format!("mods/{}", e.archive),
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

fn after_mods_change(ui: &AppWindow, current: &str) {
    with_app(|a| a.overlay = true);
    ui.set_mods_available(true);
    ui.set_mods_overlay(true);
    rebuild_tree(ui);
    navigate(ui, current, false);
    refresh_undo(ui);
}

fn pick_package(ui: &AppWindow) {
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
fn open_package(ui: &AppWindow, file: PathBuf) {
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
            Err(e) => ui.set_status(format!("Could not read the package: {e:#}").into()),
        });
    });
}

fn install_package(ui: &AppWindow) {
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

fn uninstall_selected_group(ui: &AppWindow) {
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextureAction {
    Replace,
    Add,
    Remove,
}

/// Replaces, adds or removes a texture of the open dictionary and writes the
/// result into the mods folder (in the background).
fn edit_texture(ui: &AppWindow, action: TextureAction) {
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
            set_busy(&ui, true, "Writing the changed dictionary into the mods folder…".into())
        });
        let started = Instant::now();
        let result = openrage_app::textures::edit_textures(&game_dir, keys, &ytd, &[edit]);
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

fn start_extract(ui: &AppWindow) {
    if with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Textures { .. }))) {
        export_textures(ui, false);
        return;
    }
    let target = with_app(|a| -> Result<(Arc<Rpf7>, EntryId), String> {
        let listing = a.listing.as_ref().ok_or("nothing to extract")?;
        let item = a.selected.map(|s| &listing.items[s]);
        match (&listing.location, item) {
            (Location::Search { .. } | Location::Changes { .. }, Some(item)) if item.mark != Some(Mark::Removed) => {
                let dir = a.game_dir.clone().ok_or("no game directory")?;
                match vfs::resolve(&dir, item.vpath.as_deref().unwrap_or_default(), a.keys.clone()) {
                    Ok(Node::ArchiveFile(rpf, id) | Node::ArchiveDir(rpf, id)) => Ok((rpf, id)),
                    Ok(_) => Err("Only archive contents can be extracted.".into()),
                    Err(e) => Err(e.to_string()),
                }
            }
            (Location::Archive(rpf, _), Some(item)) => Ok((Arc::clone(rpf), item.entry.expect("archive item"))),
            (Location::Archive(rpf, dir), None) => Ok((Arc::clone(rpf), *dir)),
            (Location::Disk(dir), Some(item)) if item.kind == ItemKind::Archive => {
                let rpf = Rpf7::open(&dir.join(&item.name), a.keys.clone()).map_err(|e| e.to_string())?;
                Ok((Arc::new(rpf), Rpf7::ROOT))
            }
            _ => Err("Select an archive or something inside an archive to extract.".into()),
        }
    });
    let (rpf, id) = match target {
        Ok(t) => t,
        Err(msg) => {
            ui.set_status(msg.into());
            return;
        }
    };
    let cancel = with_app(|a| {
        a.cancel.store(false, Ordering::Relaxed);
        Arc::clone(&a.cancel)
    });
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let Some(out) = rfd::FileDialog::new().set_title("Extract to folder").pick_folder() else { return };
        let _ = weak.upgrade_in_event_loop(|ui| set_busy(&ui, true, "Extracting…".into()));
        let mut last = Instant::now();
        let result = extract_entry(&rpf, id, &out, false, &cancel, &mut |s, path| {
            if last.elapsed() > Duration::from_millis(100) {
                last = Instant::now();
                let msg = format!("Extracting… {} files, {} — {path}", s.files, human_size(s.bytes));
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
            }
        });
        let msg = match result {
            Ok(s) if s.cancelled => format!("Cancelled after {} files.", s.files),
            Ok(s) => {
                let mut m = format!("Extracted {} files ({}) to {}", s.files, human_size(s.bytes), s.target.display());
                if !s.failures.is_empty() {
                    m.push_str(&format!(" — {} failures, first: {}", s.failures.len(), s.failures[0]));
                }
                m
            }
            Err(e) => format!("Extraction failed: {e:#}"),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| set_busy(&ui, false, msg));
    });
}

fn set_busy(ui: &AppWindow, busy: bool, status: String) {
    with_app(|a| a.busy = busy);
    ui.set_busy(busy);
    ui.set_status(status.into());
}

// ---------------------------------------------------------------------------
// Setup

fn show_setup(ui: &AppWindow, can_cancel: bool) {
    let detected: Vec<String> = detect::find_gta5_installs().iter().map(|p| p.display().to_string()).collect();
    with_app(|a| {
        let game = a.game_dir.clone().or_else(|| a.cfg.game_dir.clone()).map(|p| p.display().to_string());
        let game = game.or_else(|| detected.first().cloned()).unwrap_or_default();
        ui.set_setup_game_dir(game.into());
        ui.set_setup_magic_path(a.cfg.magic_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default().into());
        ui.set_setup_can_cancel(can_cancel && a.keys.is_some() && a.game_dir.is_some());
    });
    let model: Vec<SharedString> = detected.into_iter().map(Into::into).collect();
    ui.set_setup_detected(ModelRc::new(VecModel::from(model)));
    ui.set_setup_log(SharedString::new());
    ui.set_setup_visible(true);
}

fn pick_path(w: Weak<AppWindow>, file: bool, title: &'static str, apply: fn(&AppWindow, PathBuf)) {
    std::thread::spawn(move || {
        let dialog = rfd::FileDialog::new().set_title(title);
        let picked = if file { dialog.add_filter("Key blob", &["dat"]).pick_file() } else { dialog.pick_folder() };
        if let Some(p) = picked {
            let _ = w.upgrade_in_event_loop(move |ui| apply(&ui, p));
        }
    });
}

fn run_setup(ui: &AppWindow) {
    let game_dir = PathBuf::from(ui.get_setup_game_dir().trim());
    let magic = Some(ui.get_setup_magic_path().trim().to_owned()).filter(|s| !s.is_empty()).map(PathBuf::from);
    if !detect::is_gta5_dir(&game_dir) {
        ui.set_setup_log(format!("{} does not contain GTA5_Enhanced.exe or GTA5.exe.", game_dir.display()).into());
        return;
    }
    ui.set_setup_running(true);
    ui.set_setup_log(SharedString::new());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let log = std::sync::Mutex::new(String::new());
        let push = |line: &str| {
            let text = {
                let mut l = log.lock().expect("log");
                l.push_str(line);
                l.push('\n');
                l.clone()
            };
            let _ = weak.upgrade_in_event_loop(move |ui| ui.set_setup_log(text.into()));
        };
        let result = setup::derive_keys(&game_dir, None, magic.as_deref(), &mut |m| push(m));
        match result {
            Ok((keys, report)) => {
                push(&format!(
                    "Keys found ({}) and verified{}.",
                    match report.source {
                        KeySource::Executable => "from the executable",
                        KeySource::KeyBlob => "from the key blob",
                    },
                    report.verified_entries.map(|n| format!(" — common.rpf has {n} entries")).unwrap_or_default()
                ));
                let _ = weak.upgrade_in_event_loop(move |ui| finish_setup(&ui, game_dir, magic, keys));
            }
            Err(e) => {
                if e.downcast_ref::<KeyBlobRequired>().is_some() {
                    push("This is GTA V Enhanced: its executable no longer contains the NG keys in plain form.");
                    push("Select a CodeWalker key blob (magic.dat) and try again.");
                } else {
                    push(&format!("Error: {e:#}"));
                }
                let _ = weak.upgrade_in_event_loop(|ui| ui.set_setup_running(false));
            }
        }
    });
}

fn finish_setup(ui: &AppWindow, game_dir: PathBuf, magic: Option<PathBuf>, keys: Arc<GtaVKeys>) {
    let saved = with_app(|a| {
        a.keys = Some(keys);
        a.game_dir = Some(game_dir.clone());
        a.cfg.game_dir = Some(game_dir);
        a.cfg.magic_path = magic;
        a.cfg.save()
    });
    ui.set_setup_running(false);
    ui.set_setup_visible(false);
    start_browsing(ui);
    if let Err(e) = saved {
        ui.set_status(format!("Could not save settings: {e:#}").into());
    }
}
