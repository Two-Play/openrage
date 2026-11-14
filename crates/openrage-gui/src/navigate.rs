//! Browsing: the folder tree, the file list, the address bar and the info panel.

use crate::*;

pub(crate) fn start_browsing(ui: &AppWindow) {
    plugins::set_game(with_app(|a| a.game_dir.clone()).as_deref());
    let label = with_app(|a| a.game_dir.as_deref().and_then(detect::game_name)).unwrap_or("Game");
    GAME_LABEL.with(|l| *l.borrow_mut() = label.to_owned());
    ui.set_settings_game(label.into());
    ui.set_settings_game_dir(
        with_app(|a| a.game_dir.as_ref().map(|d| d.display().to_string())).unwrap_or_default().into(),
    );
    ui.set_overlay_folder(overlay_name().into());
    show_write_target(ui);
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
        } else if arg.to_ascii_lowercase().ends_with(".orp") && Path::new(&arg).is_file() {
            // `openrage-gui plugin.orp`: the plugin install dialog.
            offer_plugin_package(ui, PathBuf::from(arg));
        } else {
            navigate(ui, &arg, true);
        }
    }
}

pub(crate) fn rebuild_tree(ui: &AppWindow) {
    with_app(|a| {
        if let Some(dir) = a.game_dir.clone() {
            a.tree = Tree::build(&dir, a.keys.clone(), a.overlay);
        }
    });
    refresh_tree(ui, None);
}

/// Opens a folder (or archive) in the file list. Listing it can take a while
/// (a nested archive, a texture dictionary or an audio container is decoded),
/// so that runs off the UI thread: the window stays responsive, a slow folder
/// shows "Opening …", and a newer navigation drops the result of an older one.
pub(crate) fn navigate(ui: &AppWindow, path: &str, record: bool) {
    navigate_then(ui, path, record, |_| {});
}

/// [`navigate`], then `then` once the folder is shown (not when listing it
/// failed or a newer navigation took its place).
pub(crate) fn navigate_then(ui: &AppWindow, path: &str, record: bool, then: impl FnOnce(&AppWindow) + Send + 'static) {
    if navigate_special(ui, path, record) {
        then(ui);
        return;
    }
    let Some((dir, keys, overlay, counter)) =
        with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone(), a.overlay, Arc::clone(&a.navigate_generation))))
    else {
        return;
    };
    let generation = counter.fetch_add(1, Ordering::Relaxed) + 1;
    let done = Arc::new(AtomicBool::new(false));
    {
        let (counter, done, weak) = (Arc::clone(&counter), Arc::clone(&done), ui.as_weak());
        let label = browser::normalize(path);
        let label = if label.is_empty() { "the game folder".to_owned() } else { label };
        slint::Timer::single_shot(Duration::from_millis(150), move || {
            if !done.load(Ordering::Relaxed)
                && counter.load(Ordering::Relaxed) == generation
                && let Some(ui) = weak.upgrade()
            {
                ui.set_status(format!("Opening {label}…").into());
            }
        });
    }
    let weak = ui.as_weak();
    let path = path.to_owned();
    std::thread::spawn(move || {
        let result = browser::list(&dir, &path, keys.clone(), overlay);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            done.store(true, Ordering::Relaxed);
            if counter.load(Ordering::Relaxed) != generation {
                return;
            }
            if show_listing(&ui, result, &dir, keys, overlay, record) {
                then(&ui);
            }
        });
    });
}

/// [`navigate`] on the UI thread, for code that reads the new listing right
/// away (after writing into an archive, after undo).
pub(crate) fn navigate_now(ui: &AppWindow, path: &str, record: bool) {
    if navigate_special(ui, path, record) {
        return;
    }
    let Some((dir, keys, overlay)) = with_app(|a| {
        // Drops the result of a navigation still running.
        a.navigate_generation.fetch_add(1, Ordering::Relaxed);
        a.game_dir.clone().map(|d| (d, a.keys.clone(), a.overlay))
    }) else {
        return;
    };
    let result = browser::list(&dir, path, keys.clone(), overlay);
    show_listing(ui, result, &dir, keys, overlay, record);
}

/// Search results, changes and history are not folders; true when `path` was one.
fn navigate_special(ui: &AppWindow, path: &str, record: bool) -> bool {
    if let Some(query) = browser::search_query(path) {
        start_search(ui, query.to_owned(), record);
    } else if path.trim() == browser::CHANGES_PATH {
        start_changes(ui, record);
    } else if path.trim() == browser::HISTORY_PATH {
        show_history(ui, record);
    } else {
        return false;
    }
    true
}

/// Shows a listed folder (or why it could not be listed); true when shown.
fn show_listing(
    ui: &AppWindow,
    result: Result<(Listing, Option<String>), RpfError>,
    dir: &Path,
    keys: Option<Arc<GtaVKeys>>,
    overlay: bool,
    record: bool,
) -> bool {
    let shown = match result {
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
            let tree_idx = with_app(|a| a.tree.reveal(&vpath, dir, keys, overlay));
            refresh_tree(ui, tree_idx);
            ui.set_location_title(with_app(|a| a.listing.as_ref().map(location_title)).unwrap_or_default().into());
            refresh_files(ui, select.as_deref());
            if select.is_none() {
                show_folder_info(ui);
            }
            ui.set_status(SharedString::new());
            start_folder_diff(ui);
            true
        }
        Err(RpfError::KeysRequired(_)) => {
            show_setup(ui, true);
            false
        }
        Err(e) => {
            show_error(ui, format!("{e}"));
            false
        }
    };
    update_nav_buttons(ui);
    shown
}

pub(crate) fn update_nav_buttons(ui: &AppWindow) {
    with_app(|a| {
        ui.set_can_back(a.history.can_back());
        ui.set_can_forward(a.history.can_forward());
        ui.set_can_up(a.listing.as_ref().is_some_and(|l| browser::parent(&l.vpath).is_some()));
    });
}

pub(crate) fn refresh_tree(ui: &AppWindow, selected: Option<usize>) {
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
pub(crate) fn refresh_files(ui: &AppWindow, select: Option<&str>) {
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
    ui.set_textures_locked(with_app(|a| {
        matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Textures { read_only: true, .. }))
    }));
    ui.set_textures_game(
        with_app(|a| {
            let textures = matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Textures { .. }));
            match a.game_dir.as_deref() {
                Some(d) if textures && is_rdr2_dir(d) => "Red Dead Redemption 2",
                Some(d) if textures && is_gta4_dir(d) => "GTA IV",
                _ => "",
            }
        })
        .into(),
    );
    ui.set_texture_selected(texture_mode && selected_row.is_some());
    let history_mode = with_app(|a| matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::History)));
    ui.set_history_mode(history_mode);
    if !history_mode {
        ui.set_history_group(SharedString::new());
    }
    let archive_mode = with_app(|a| {
        matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Archive(..) | Location::Gta4(..)))
    });
    ui.set_audio_mode_folder(with_app(|a| {
        matches!(a.listing.as_ref().map(|l| &l.location), Some(Location::Audio { .. }))
    }));
    ui.set_in_mods(with_app(|a| a.listing.as_ref().is_some_and(|l| l.origin != Origin::Base)));
    ui.set_archive_mode(archive_mode);
    ui.set_entry_selected(archive_mode && selected_row.is_some());
    if let (Some(row), Some(_)) = (selected_row, select) {
        select_row(ui, row);
    }
}

/// Mod status as the UI's mark index (see `Theme.mark-color`).
pub(crate) fn mark_index(mark: Option<Mark>) -> i32 {
    match mark {
        None => 0,
        Some(Mark::Added | Mark::New) => 1,
        Some(Mark::Changed | Mark::Replaced) => 2,
        Some(Mark::ContainsMods) => 3,
        Some(Mark::Removed) => 4,
    }
}

pub(crate) fn is_model_item(item: &Item) -> bool {
    (item.kind == ItemKind::Ped || (item.kind == ItemKind::Resource && openrage_app::models::is_model(&item.name)))
        && item.mark != Some(Mark::Removed)
}

/// Items with a large view: models, textures and images.
pub(crate) fn is_viewable(item: &Item) -> bool {
    let lower = item.name.to_ascii_lowercase();
    is_model_item(item) || item.kind == ItemKind::Texture || [".ytd", ".dds", ".png"].iter().any(|e| lower.ends_with(e))
}

/// Files that open in the text editor (by extension; binary contents are
/// detected when opening).
pub(crate) fn is_text_file(item: &Item) -> bool {
    const TEXT: [&str; 19] = [
        "xml",
        "meta",
        "txt",
        "ini",
        "cfg",
        "json",
        "ide",
        "ipl",
        "lua",
        "csv",
        "log",
        "dat",
        "gxt",
        "list",
        "ugc",
        "gxt2",
        "nametable",
        "yldb",
        "wpl",
    ];
    let ext = item.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(item.kind, ItemKind::File)
        && item.mark != Some(Mark::Removed)
        && (TEXT.contains(&ext.as_str()) || is_gta4_cutscene(item))
}

/// A GTA IV cutscene (`.cut`): plain text in sections, unlike GTA V's
/// binary metadata of the same extension.
pub(crate) fn is_gta4_cutscene(item: &Item) -> bool {
    item.name.to_ascii_lowercase().ends_with(".cut") && GAME_LABEL.with(|l| l.borrow().as_str() == "GTA IV")
}

/// Binary metadata files, shown as XML.
pub(crate) fn is_meta_file(item: &Item) -> bool {
    matches!(item.kind, ItemKind::File | ItemKind::Resource | ItemKind::Ped)
        && item.mark != Some(Mark::Removed)
        && openrage_app::metadata::is_metadata(&item.name)
        && !is_gta4_cutscene(item)
}

/// Double-click navigates into it (or, for search hits and changes, to it).
pub(crate) fn is_openable(item: &Item) -> bool {
    item.vpath.is_some()
        || item.kind.is_container()
        || (item.kind == ItemKind::Resource && browser::is_texture_dict(&item.name))
        || (item.kind == ItemKind::File && browser::is_audio(&item.name))
}

/// Context-menu entries plugins add for a file.
pub(crate) fn plugin_actions(item: &Item) -> ModelRc<SharedString> {
    let titles = match plugin_target(item) {
        Some(folder) => plugins::action_titles(&item.name, folder),
        None => Vec::new(),
    };
    if titles.is_empty() {
        return ModelRc::default();
    }
    ModelRc::new(VecModel::from(titles.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

/// Whether plugin actions apply to a list item: `Some(false)` for a file,
/// `Some(true)` for a folder or archive (batch actions), `None` otherwise.
pub(crate) fn plugin_target(item: &Item) -> Option<bool> {
    if item.mark == Some(Mark::Removed) {
        return None;
    }
    match item.kind {
        ItemKind::File | ItemKind::Resource | ItemKind::Ped => Some(false),
        ItemKind::Dir | ItemKind::Archive => Some(true),
        _ => None,
    }
}

pub(crate) fn file_row(item: &Item) -> FileRow {
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
        can_edit_text: is_text_file(item),
        is_meta: is_meta_file(item),
        is_gfx: item.name.to_ascii_lowercase().ends_with(".gfx"),
        is_table: is_text_file(item) && openrage_app::texttable::is_text_table(&item.name),
        plugin_actions: plugin_actions(item),
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
pub(crate) fn set_location(ui: &AppWindow, vpath: &str) {
    ui.set_address(vpath.into());
    ui.set_address_editing(false);
    ui.set_crumbs(ModelRc::new(VecModel::from(crumbs(vpath))));
}

pub(crate) fn crumbs(vpath: &str) -> Vec<Crumb> {
    let crumb = |label: &str, path: &str| Crumb { label: label.into(), path: path.into() };
    let mut out = vec![crumb(&game_label(), "")];
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
pub(crate) fn location_title(l: &Listing) -> String {
    match &l.location {
        Location::Search { query, .. } => format!("Search: {query}"),
        Location::Changes { .. } => "Mod changes".into(),
        Location::History => "History".into(),
        _ => l.vpath.rsplit('/').next().filter(|n| !n.is_empty()).map_or_else(game_label, str::to_owned),
    }
}

thread_local! {
    /// The game's short name, set when browsing starts (read while `APP` may be borrowed).
    static GAME_LABEL: std::cell::RefCell<String> = std::cell::RefCell::new("Game".to_owned());
}

/// The game's short name ("GTA V Enhanced", "RDR2"), for the address bar and headings.
pub(crate) fn game_label() -> String {
    GAME_LABEL.with(|l| l.borrow().clone())
}

/// Fills the info panel header and metadata table.
pub(crate) fn set_info(ui: &AppWindow, title: &str, path: &str, meta: &Meta, icon: (&str, slint::Color)) {
    ui.set_info_title(title.into());
    ui.set_info_path(path.into());
    set_meta(ui, meta);
    ui.set_info_icon(icon.0.into());
    ui.set_info_tint(icon.1);
}

pub(crate) fn set_meta(ui: &AppWindow, meta: &Meta) {
    let rows: Vec<InfoRow> = meta.iter().map(|(k, v)| InfoRow { key: k.into(), value: v.into() }).collect();
    ui.set_info_rows(ModelRc::new(VecModel::from(rows)));
}

pub(crate) fn show_folder_info(ui: &AppWindow) {
    // Read before `with_app` below: it would borrow the app state twice.
    let overlay = overlay_name();
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
                    vec![note(format!(
                        "Every change OpenRAGE made to the {} folder. Select a package entry to uninstall the whole package.",
                        overlay
                    ))],
                    ("history", badges::NEUTRAL),
                ));
            }
            Location::Textures { dict, .. } => {
                // `…/x.wdr/textures`: the textures inside a GTA IV model.
                if let Some(model) = browser::model_of_textures(&l.vpath) {
                    let model_name = model.rsplit('/').next().unwrap_or(model);
                    return Some((
                        format!("{model_name} — textures"),
                        l.vpath.clone(),
                        vec![
                            kv("Type", "Textures inside a model"),
                            kv("Textures", dict.textures.len()),
                            note(if openrage_app::textures::is_gta4_model(model) {
                                "Replace a texture to change the model's look. Textures its shaders use \
                                 cannot be removed."
                            } else {
                                "Replace a texture to change the model's look. The same size (or smaller) \
                                 always fits; a larger one only if the model has room for it."
                            }),
                        ],
                        badges::icon(&name, ItemKind::Resource),
                    ));
                }
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
            Location::Audio { awc } => {
                let mut meta = vec![kv("Type", "Audio container")];
                match &awc.stream_format {
                    Some(f) => {
                        meta.push(kv("Content", format!("one stream with {} channels", f.channels)));
                        if let Some(s) = awc.sounds.first() {
                            meta.push(kv("Duration", browser::seconds(s.duration_secs())));
                        }
                    }
                    None => meta.push(kv("Sounds", awc.sounds.len())),
                }
                if awc.encrypted {
                    meta.push(kv("Encrypted", "yes"));
                }
                meta.push(note("Double-click a sound to play it; Extract… saves WAV files."));
                return Some((name.clone(), l.vpath.clone(), meta, ("audio", badges::NEUTRAL)));
            }
            Location::Disk(p) => vec![kv("Type", "Folder"), kv("On disk", p.display())],
            Location::Rpf8(rpf, _) => {
                let h = rpf.header();
                vec![
                    kv("Type", "Inside a Red Dead Redemption 2 archive"),
                    kv("Archive", rpf.display_name().rsplit('/').next().unwrap_or_default()),
                    kv("Entries", grouped(h.entry_count as usize)),
                    note("Read-only. Names come from `openrage rdr2-names`; unnamed files are under hash/."),
                ]
            }
            Location::Gta4(a, _) => {
                let mut meta = vec![kv("Type", "Inside a GTA IV archive")];
                meta.extend(gta4_archive_info(a));
                meta.push(note("Read-only. Extract… saves files as stored (resources as RSC5 files, like OpenIV)."));
                meta
            }
            Location::Archive(rpf, _) => vec![
                kv("Type", "Inside an archive"),
                kv("Archive", rpf.name()),
                kv("Encryption", format!("{:?}", rpf.encryption())),
                kv("Entries", grouped(rpf.entries().len())),
            ],
        };
        let mut meta = place;
        match l.origin {
            Origin::Mods => meta.push(kv("Source", format!("{} folder (replaces the game's version)", overlay))),
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
    ui.set_model_textures(0);
    ui.set_preview_loading(false);
}
