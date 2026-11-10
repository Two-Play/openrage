//! OpenRAGE graphical explorer.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod badges;
mod browser;
mod preview;

use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use openrage_app::config::Config;
use openrage_app::extract::{extract_entry, human_size};
use openrage_app::setup::{self, KeyBlobRequired, KeySource};
use openrage_app::{detect, game_language_from_locale, index_path, load_keys};
use rage_crypto::GtaVKeys;
use rage_index::{EntryTag, HitReason, Index, LANGUAGES};
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryId, EntryKind, Rpf7, RpfError};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use browser::{History, Item, ItemKind, Listing, Location, Tree};

slint::include_modules!();

/// UI-thread state. Lives in a thread-local so that closures posted back from
/// worker threads (which must be `Send`) can reach it.
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
            index_running: false,
        })
    });

    ui.set_mono_font(mono_font().into());
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
    ui.run()?;
    Ok(())
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
                    a.tree.toggle(i as usize, &dir, a.keys.clone());
                }
            });
            refresh_tree(ui, None);
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
            let target = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(i as usize)?];
                if let Some(vpath) = &item.vpath {
                    return Some(vpath.clone()); // search hit: jump to its location
                }
                item.kind.is_container().then(|| browser::join(&listing.vpath, &item.name))
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
    ui.on_extract_selected(move || with_ui(&w, start_extract));
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
    let title = with_app(|a| {
        let dir = a.game_dir.clone().expect("game dir");
        a.tree = Tree::build(&dir, a.keys.clone());
        a.history = History::default();
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("OpenRAGE — {name}")
    });
    ui.set_window_title(title.into());
    start_index_update(ui);
    navigate(ui, "", true);
    // Optional start path, e.g. `openrage-gui x64e.rpf/levels/gta5/vehicles.rpf/adder.yft`.
    if let Some(path) = std::env::args().nth(1) {
        navigate(ui, &path, true);
    }
}

fn navigate(ui: &AppWindow, path: &str, record: bool) {
    if let Some(query) = browser::search_query(path) {
        start_search(ui, query.to_owned(), record);
        return;
    }
    let Some((dir, keys)) = with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone()))) else { return };
    match browser::list(&dir, path, keys.clone()) {
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
            ui.set_address(vpath.clone().into());
            let tree_idx = with_app(|a| a.tree.reveal(&vpath, &dir, keys));
            refresh_tree(ui, tree_idx);
            refresh_files(ui, select.as_deref());
            if select.is_none() {
                show_folder_info(ui);
            }
            ui.set_status(SharedString::new());
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
                let (badge, tint) = badges::badge(&n.name, n.kind);
                TreeRow {
                    name: n.name.clone().into(),
                    path: n.vpath.clone().into(),
                    depth: n.depth as i32,
                    expandable: n.expandable,
                    expanded: n.expanded,
                    badge: badge.into(),
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
        let order = if matches!(listing.location, Location::Search { .. }) {
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
        matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Archive(..))) || a.selected.is_some()
    }));
    if let (Some(row), Some(_)) = (selected_row, select) {
        select_row(ui, row);
    }
}

fn file_row(item: &Item) -> FileRow {
    let (badge, tint) = badges::badge(&item.name, item.kind);
    FileRow {
        name: item.name.clone().into(),
        badge: badge.into(),
        tint,
        kind: match &item.vpath {
            Some(vpath) => browser::short_location(&browser::parent(vpath).unwrap_or_default(), 40).into(),
            None => badges::type_name(&item.name, item.kind).into(),
        },
        size: if item.kind.is_container() && item.size == 0 {
            SharedString::new()
        } else {
            human_size(item.size).into()
        },
        info: item.info.clone().into(),
    }
}

fn show_folder_info(ui: &AppWindow) {
    let text = with_app(|a| {
        let l = a.listing.as_ref()?;
        let place = match &l.location {
            Location::Search { summary, .. } => return Some(summary.clone()),
            Location::Disk(p) => format!("Folder on disk\n{}", p.display()),
            Location::Archive(rpf, _) => {
                format!(
                    "Inside archive {}\nEncryption: {:?}, {} entries",
                    rpf.name(),
                    rpf.encryption(),
                    rpf.entries().len()
                )
            }
        };
        let shown = if l.vpath.is_empty() { "(game folder)".to_owned() } else { l.vpath.clone() };
        Some(format!("{shown}\n{place}"))
    });
    ui.set_info_text(text.unwrap_or_default().into());
    ui.set_preview_text(SharedString::new());
    ui.set_preview_loading(false);
}

// ---------------------------------------------------------------------------
// Selection + preview

/// What the preview worker needs to load an item off the UI thread.
enum PreviewSource {
    DiskFile(PathBuf),
    DiskArchive(PathBuf),
    Entry(Arc<Rpf7>, EntryId),
    /// A search hit; resolved in the worker. `query` is set for text hits to show the matching line.
    Virtual {
        game_dir: PathBuf,
        vpath: String,
        query: Option<String>,
    },
}

fn select_row(ui: &AppWindow, row: usize) {
    let Some((item, vpath, source, keys, generation)) = with_app(|a| {
        let listing = a.listing.as_ref()?;
        let idx = *a.visible.get(row)?;
        a.selected = Some(idx);
        let item = listing.items[idx].clone();
        let source = match (&listing.location, item.entry) {
            (Location::Search { query, .. }, _) => {
                let vpath = item.vpath.clone().unwrap_or_default();
                let query = (item.info == "text").then(|| match rage_index::parse_hash(query) {
                    Some(h) => format!("0x{h:08X}"),
                    None => query.clone(),
                });
                a.game_dir.clone().map(|game_dir| PreviewSource::Virtual { game_dir, vpath, query })
            }
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
    ui.set_can_extract(true);

    let mut info =
        format!("{}\n{}\n{} · {}", item.name, vpath, badges::type_name(&item.name, item.kind), human_size(item.size));
    if !item.info.is_empty() {
        info.push_str(&format!(" · {}", item.info));
    }
    ui.set_info_text(info.clone().into());
    ui.set_preview_text(SharedString::new());
    let Some(source) = source else {
        ui.set_preview_loading(false);
        return;
    };
    ui.set_preview_loading(true);

    let counter = with_app(|a| Arc::clone(&a.preview_generation));
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let (extra, body) = load_preview(source, keys);
        if counter.load(Ordering::SeqCst) != generation {
            return; // a newer selection superseded this one
        }
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if counter.load(Ordering::SeqCst) != generation {
                return;
            }
            if !extra.is_empty() {
                ui.set_info_text(format!("{info}\n{extra}").into());
            }
            ui.set_preview_text(body.into());
            ui.set_preview_loading(false);
        });
    });
}

/// Returns (additional info lines, preview body).
fn load_preview(source: PreviewSource, keys: Option<Arc<GtaVKeys>>) -> (String, String) {
    match source {
        PreviewSource::Virtual { game_dir, vpath, query } => match vfs::resolve(&game_dir, &vpath, keys.clone()) {
            Ok(Node::ArchiveFile(rpf, id)) => {
                let (mut extra, body) = load_preview(PreviewSource::Entry(Arc::clone(&rpf), id), keys);
                if let Some(q) = query {
                    let name = rpf.entry(id).name.clone();
                    let line = rpf
                        .read(id)
                        .ok()
                        .and_then(|d| rage_index::content_for(&name, &d))
                        .and_then(|(text, _)| rage_index::snippet(&text, &q));
                    if let Some(line) = line {
                        extra = format!("Match: {line}\n{extra}");
                    }
                }
                (extra, body)
            }
            Ok(Node::ArchiveDir(rpf, id)) if id == Rpf7::ROOT => (archive_info(&rpf), String::new()),
            Ok(Node::ArchiveDir(..)) | Ok(Node::Dir(_)) => (String::new(), String::new()),
            Ok(Node::PlainFile(p)) => load_preview(PreviewSource::DiskFile(p), keys),
            Err(e) => (format!("Error: {e}"), String::new()),
        },
        PreviewSource::DiskFile(path) => {
            let mut data = Vec::new();
            match File::open(&path).and_then(|f| f.take(1 << 20).read_to_end(&mut data)) {
                Ok(_) => (String::new(), preview::render(&data)),
                Err(e) => (format!("Error: {e}"), String::new()),
            }
        }
        PreviewSource::DiskArchive(path) => match Rpf7::open(&path, keys) {
            Ok(rpf) => (archive_info(&rpf), String::new()),
            Err(e) => (format!("Error: {e}"), String::new()),
        },
        PreviewSource::Entry(rpf, id) => {
            let entry = rpf.entry(id).clone();
            if entry.is_archive() {
                return match rpf.open_child(id) {
                    Ok(child) => (archive_info(&child), String::new()),
                    Err(e) => (format!("Error: {e}"), String::new()),
                };
            }
            match entry.kind {
                EntryKind::Resource { size, .. } => {
                    let (sys, gfx) = entry.resource_page_sizes().unwrap_or_default();
                    let info = format!(
                        "Resource version {} · stored {} · system pages {} · graphics pages {}",
                        entry.resource_version().unwrap_or_default(),
                        human_size(size as u64),
                        human_size(sys),
                        human_size(gfx)
                    );
                    match rpf.read_resource_pages(id) {
                        Ok(pages) => (info, preview::hex_dump(&pages)),
                        Err(e) => (format!("{info}\nError: {e}"), String::new()),
                    }
                }
                EntryKind::Binary { stored_size, size, encrypted, .. } => {
                    let mut info = Vec::new();
                    if stored_size > 0 {
                        info.push(format!(
                            "compressed {} → {}",
                            human_size(stored_size as u64),
                            human_size(size as u64)
                        ));
                    }
                    if encrypted {
                        info.push("encrypted".into());
                    }
                    match rpf.read(id) {
                        Ok(data) => (info.join(" · "), preview::render(&data)),
                        Err(e) => (format!("Error: {e}"), String::new()),
                    }
                }
                EntryKind::Directory { .. } => (String::new(), String::new()),
            }
        }
    }
}

fn archive_info(rpf: &Rpf7) -> String {
    let (mut dirs, mut files, mut resources, mut archives) = (0, 0, 0, 0);
    for e in rpf.entries() {
        match e.kind_label() {
            "dir" => dirs += 1,
            "rpf" => archives += 1,
            "resource" => resources += 1,
            _ => files += 1,
        }
    }
    format!(
        "RPF7 · encryption {:?}\n{dirs} folders, {files} files, {resources} resources, {archives} nested archives",
        rpf.encryption()
    )
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
            location: Location::Search { query: query.clone(), summary: summary.clone() },
            items,
        });
        a.selected = None;
        a.filter.clear();
    });
    ui.set_filter(SharedString::new());
    ui.set_address(vpath.into());
    ui.set_search_mode(true);
    ui.set_search_summary(summary.into());
    ui.set_tree_selected(-1);
    refresh_files(ui, None);
    show_folder_info(ui);
    ui.set_status(SharedString::new());
    update_nav_buttons(ui);
}

// ---------------------------------------------------------------------------
// Extraction

fn start_extract(ui: &AppWindow) {
    let target = with_app(|a| -> Result<(Arc<Rpf7>, EntryId), String> {
        let listing = a.listing.as_ref().ok_or("nothing to extract")?;
        let item = a.selected.map(|s| &listing.items[s]);
        match (&listing.location, item) {
            (Location::Search { .. }, Some(item)) => {
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
