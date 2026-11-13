//! The search index, search results, and the changes made by mods.

use crate::*;

pub(crate) fn start_index_update(ui: &AppWindow) {
    let Some((game_dir, keys)) = with_app(|a| {
        // The search index covers GTA V's and GTA IV's archives; RDR2 is browsed without it for now.
        if a.index_running || a.game_dir.as_deref().is_some_and(is_rdr2_dir) {
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
            // Names of models, textures, … so hashes in metadata read as names.
            if let Ok(names) = index.known_names() {
                rage_formats::names::register(names);
            }
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

pub(crate) fn start_search(ui: &AppWindow, query: String, record: bool) {
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
            Err(e) => show_error(&ui, format!("Search failed: {e}")),
        });
    });
}

pub(crate) fn show_search_results(
    ui: &AppWindow,
    query: String,
    results: rage_index::SearchResults,
    ms: u128,
    record: bool,
) {
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
pub(crate) fn start_folder_diff(ui: &AppWindow) {
    // GTA IV archives in the update folder: compared with the game's copy.
    let gta4 = with_app(|a| {
        let l = a.listing.as_ref()?;
        let Location::Gta4(arc, dir) = &l.location else { return None };
        (a.overlay && l.origin == Origin::Mods).then(|| (l.vpath.clone(), Arc::clone(arc), *dir, a.game_dir.clone()))
    });
    if let Some((vpath, arc, dir, Some(game_dir))) = gta4 {
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let diff = match vfs::resolve(&game_dir, &vpath, None) {
                Ok(Node::Gta4Dir(base, bdir)) => Some(rage_mods::compare_gta4_folders(&arc, dir, &base, bdir)),
                _ => None,
            };
            let _ = weak.upgrade_in_event_loop(move |ui| apply_folder_diff(&ui, &vpath, diff));
        });
        return;
    }
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
            apply_folder_diff(&ui, &vpath, diff);
        });
    });
}

/// Marks the items of the listing at `vpath` as added, changed or removed
/// (`None`: the whole folder is new), keeping the selection.
pub(crate) fn apply_folder_diff(ui: &AppWindow, vpath: &str, diff: Option<rage_mods::FolderDiff>) {
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
        refresh_files(ui, None);
        if let Some(name) = selected {
            let row = with_app(|a| {
                let l = a.listing.as_ref()?;
                a.visible.iter().position(|&i| l.items[i].name == name)
            });
            ui.set_file_selected(row.map_or(-1, |r| r as i32));
        }
    }
}

pub(crate) fn start_changes(ui: &AppWindow, record: bool) {
    let Some((game_dir, keys)) = with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone()))) else { return };
    if rage_mods::mods_dir(&game_dir).is_none() {
        ui.set_status(format!("This game has no {} folder.", overlay_name()).into());
        return;
    }
    ui.set_status("Comparing mods with the original files…".into());
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut last = Instant::now();
        let mut changes = rage_mods::list_changes(&game_dir, keys, CompareMode::Quick, &mut |i, n, p| {
            if last.elapsed() > Duration::from_millis(150) {
                last = Instant::now();
                let msg = format!("Comparing {}/{n}: {p}", i + 1);
                let _ = weak.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
            }
        });
        // GTA IV: changes made to the game's own files, marked by a leading `!`.
        changes.extend(rage_mods::list_game_changes(&game_dir).into_iter().map(|mut c| {
            c.vpath.insert(0, '!');
            c
        }));
        let ms = started.elapsed().as_millis();
        let _ = weak.upgrade_in_event_loop(move |ui| show_changes(&ui, changes, ms, record));
    });
}

pub(crate) fn show_changes(ui: &AppWindow, changes: Vec<rage_mods::Change>, ms: u128, record: bool) {
    let files = with_app(|a| a.game_dir.as_deref().map(rage_mods::scan).unwrap_or_default());
    let items: Vec<Item> = changes
        .iter()
        .map(|c| {
            let in_game = c.vpath.strip_prefix('!');
            (c, in_game.unwrap_or(&c.vpath), in_game.is_some())
        })
        .map(|(c, vpath, in_game)| Item {
            name: vpath.rsplit('/').next().unwrap_or_default().to_owned(),
            kind: if c.is_dir {
                ItemKind::Dir
            } else if vpath.to_ascii_lowercase().ends_with(".rpf") {
                ItemKind::Archive
            } else {
                ItemKind::File
            },
            size: c.size,
            // The mark shows the kind; the info says where the change is.
            info: if in_game { "in the game".into() } else { c.kind.label().into() },
            entry: None,
            vpath: Some(vpath.to_owned()),
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
    let in_game = changes.iter().filter(|c| c.vpath.starts_with('!')).count();
    let mut summary = format!(
        "{} changes from {} files in the {} folder · {ms} ms",
        items.len() - in_game,
        files.len(),
        overlay_name()
    );
    if in_game > 0 {
        summary.push_str(&format!(" · {in_game} in the game files"));
    }
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
