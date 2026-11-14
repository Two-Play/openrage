//! Connects the window's callbacks to the code that handles them.

use crate::*;

pub(crate) fn wire_callbacks(ui: &AppWindow) {
    wire_navigation(ui);
    wire_files(ui);
    wire_list(ui);
    wire_model(ui);
    wire_plugins(ui);
    wire_settings(ui);
    wire_sound(ui);
    wire_archive(ui);
    wire_mods(ui);
    wire_about(ui);
    wire_setup(ui);
}

/// The address bar, back/forward/up and the folder tree.
fn wire_navigation(ui: &AppWindow) {
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
                "extract" => navigate_then(ui, &vpath, true, start_extract),
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
}

/// Selecting, opening and acting on files in the list.
fn wire_files(ui: &AppWindow) {
    let w = ui.as_weak();
    ui.on_file_select(move |i| {
        with_ui(&w, |ui| {
            // A click on the selected row (e.g. the first click of a
            // double-click) must not reload its preview.
            if ui.get_file_selected() == i && with_app(|a| a.selected.is_some()) {
                return;
            }
            select_row(ui, i as usize)
        })
    });
    let w = ui.as_weak();
    ui.on_file_open(move |i| {
        with_ui(&w, |ui| {
            let is_sound = with_app(|a| {
                let listing = a.listing.as_ref()?;
                Some(listing.items[*a.visible.get(i as usize)?].kind == ItemKind::Sound)
            });
            if is_sound == Some(true) {
                if ui.get_file_selected() != i {
                    with_app(|a| a.play_when_ready = true);
                    select_row(ui, i as usize);
                } else {
                    stop_sound(ui);
                    toggle_sound(ui);
                }
                return;
            }
            // GTA IV placement files open as a 3D scene (before the text check:
            // they can also be edited as text, through the context menu).
            let placement = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(i as usize)?];
                (item.kind == ItemKind::File && item.name.to_ascii_lowercase().ends_with(".wpl"))
                    .then(|| item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = placement {
                scene::open_map(ui, vpath);
                return;
            }
            let text = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(i as usize)?];
                is_text_file(item)
                    .then(|| item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = text {
                open_text_file(ui, vpath);
                return;
            }
            let meta = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(i as usize)?];
                is_meta_file(item)
                    .then(|| item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = meta {
                open_xml_view(ui, vpath);
                return;
            }
            let script = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(i as usize)?];
                let name = item.name.to_ascii_lowercase();
                ((item.kind == ItemKind::Resource && name.ends_with(".ysc"))
                    || (item.kind == ItemKind::File && name.ends_with(".sco")))
                .then(|| item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = script {
                open_script_view(ui, vpath);
                return;
            }
            let video = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = &listing.items[*a.visible.get(i as usize)?];
                item.name
                    .to_ascii_lowercase()
                    .ends_with(".bik")
                    .then(|| item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = video {
                // Video players built on FFmpeg (VLC, mpv, …) play Bink.
                open_with_system(ui, vpath);
                return;
            }
            let is_ycd = with_app(|a| {
                let listing = a.listing.as_ref()?;
                Some(is_clip_dictionary(&listing.items[*a.visible.get(i as usize)?]))
            });
            if is_ycd == Some(true) {
                if ui.get_file_selected() != i {
                    select_row(ui, i as usize);
                }
                if let Some((model, ycd)) = with_app(|a| Some((a.suggestions.first()?.clone(), a.suggest_ycd.clone())))
                {
                    open_model_with_animation(ui, model, ycd);
                }
                return;
            }
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
                    || (item.kind == ItemKind::Resource && browser::is_texture_dict(&item.name))
                    || (item.kind == ItemKind::File && browser::is_audio(&item.name));
                opens.then(|| browser::join(&listing.vpath, &item.name))
            });
            if let Some(p) = target {
                navigate(ui, &p, true);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_row_action(move |i, action| with_ui(&w, |ui| row_action(ui, i as usize, &action)));
}

/// Sorting, filtering, search, language, the mods overlay and extracting.
fn wire_list(ui: &AppWindow) {
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
}

/// The model preview: camera, suggestions, export and the viewer window.
fn wire_model(ui: &AppWindow) {
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
    ui.on_suggestion_open(move |i| {
        with_ui(&w, |ui| {
            let pick = with_app(|a| {
                Some((a.suggestions.get(i as usize)?.clone(), a.suggest_ycd.clone(), a.suggest_awc.clone()))
            });
            match pick {
                // A sound of the selected audio container: open it and play.
                Some((sound, _, awc)) if !awc.is_empty() => {
                    with_app(|a| a.play_when_ready = true);
                    navigate(ui, &browser::join(&awc, &sound), true);
                }
                Some((model, ycd, _)) => open_model_with_animation(ui, model, ycd),
                None => {}
            }
        })
    });
    let w = ui.as_weak();
    ui.on_model_resized(move || with_ui(&w, render_model));
    let w = ui.as_weak();
    ui.on_model_axis(move |axis| {
        with_ui(&w, |ui| {
            with_app(|a| a.model_view = axis_view(axis, a.model_view));
            render_model(ui);
        })
    });
    let w = ui.as_weak();
    ui.on_export_gltf(move || with_ui(&w, export_gltf));
    let w = ui.as_weak();
    ui.on_open_viewer(move || with_ui(&w, open_viewer));
    let w = ui.as_weak();
    ui.on_open_model_textures(move || {
        with_ui(&w, |ui| {
            if let Some(model) = with_app(|a| a.model_textures_path.clone()) {
                navigate(ui, &format!("{model}/{}", browser::MODEL_TEXTURES), true);
            }
        })
    });
}

/// The plugin list and plugin packages.
fn wire_plugins(ui: &AppWindow) {
    refresh_plugin_rows(ui);
    let w = ui.as_weak();
    ui.on_plugin_toggled(move |id, on| {
        with_ui(&w, |ui| {
            let result = with_app(|a| plugins::set_enabled(&mut a.cfg, &id, on));
            if let Err(e) = result {
                show_error(ui, format!("Cannot change plugin {id}: {e:#}"));
            }
            after_plugins_change(ui);
        });
    });
    let w = ui.as_weak();
    ui.on_reload_plugins(move || {
        with_ui(&w, |ui| {
            with_app(|a| plugins::reload(&a.cfg));
            after_plugins_change(ui);
            ui.set_status("Plugins reloaded".into());
        });
    });
    let w = ui.as_weak();
    ui.on_pick_plugin_package(move || {
        let w = w.clone();
        std::thread::spawn(move || {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Install a plugin")
                .add_filter("OpenRAGE plugin", &["orp"])
                .pick_file()
            else {
                return;
            };
            let _ = w.upgrade_in_event_loop(move |ui| offer_plugin_package(&ui, path));
        });
    });
    let w = ui.as_weak();
    ui.on_plugin_pkg_confirm(move || {
        with_ui(&w, |ui| {
            ui.set_plugin_pkg_visible(false);
            let Some(package) = with_app(|a| a.plugin_package.take()) else { return };
            let name = format!("{} {}", package.manifest.name, package.manifest.version);
            let result = with_app(|a| plugins::install(&a.cfg, &package));
            after_plugins_change(ui);
            ui.set_status(
                match result {
                    Ok(_) if plugins::is_enabled(&package.manifest.id) => {
                        format!("Installed {name}; it is on.")
                    }
                    Ok(_) => format!("Installed {name}. Turn it on in Settings → Plugins."),
                    Err(e) => format!("Installing {name} failed: {e:#}"),
                }
                .into(),
            );
        });
    });
    let w = ui.as_weak();
    ui.on_uninstall_plugin(move |id| {
        with_ui(&w, |ui| {
            let result = with_app(|a| plugins::uninstall(&mut a.cfg, &id));
            after_plugins_change(ui);
            ui.set_status(
                match result {
                    Ok(()) => format!("Uninstalled {id}."),
                    Err(e) => format!("Cannot uninstall {id}: {e:#}"),
                }
                .into(),
            );
        });
    });
    let w = ui.as_weak();
    ui.on_open_plugin_folder(move || {
        with_ui(&w, |ui| {
            let Some(dir) = plugins::user_folder() else { return };
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return show_error(ui, format!("Cannot create {}: {e}", dir.display()));
            }
            let opened = if cfg!(target_os = "windows") {
                std::process::Command::new("explorer").arg(&dir).spawn()
            } else if cfg!(target_os = "macos") {
                std::process::Command::new("open").arg(&dir).spawn()
            } else {
                std::process::Command::new("xdg-open").arg(&dir).spawn()
            };
            if let Err(e) = opened {
                show_error(ui, format!("Cannot open {}: {e}", dir.display()));
            }
        });
    });
}

/// Settings, appearance, quitting and cancelling work.
fn wire_settings(ui: &AppWindow) {
    let w = ui.as_weak();
    ui.on_ask_game_changed(move |ask| {
        with_ui(&w, |ui| {
            let saved = with_app(|a| {
                a.cfg.default_game = if ask { None } else { a.game_dir.clone() };
                a.cfg.save()
            });
            refresh_games(ui);
            if let Err(e) = saved {
                show_error(ui, format!("Could not save the settings: {e}"));
            }
        })
    });
    let w = ui.as_weak();
    ui.on_accent_chosen(move |mode, color| {
        accent::set(mode, color);
        with_app(|a| a.cfg.accent = Some(accent::to_setting(mode, color)));
        if let Some(ui) = w.upgrade() {
            save_settings(&ui);
            accent::apply(&ui);
        }
        viewer::set_accent();
        editor::set_accent();
        xmlview::set_accent();
        history::set_accent();
    });
    ui.on_color_hex(accent::hex_string);
    ui.on_parse_hex(|s| accent::parse_hex(&s).unwrap_or(slint::Color::from_argb_u8(0, 0, 0, 0)));
    let w = ui.as_weak();
    ui.on_theme_changed(move |mode| {
        with_app(|a| a.cfg.theme = Some(theme_name(mode).to_owned()));
        with_ui(&w, save_settings);
        viewer::set_theme(mode);
        editor::set_theme(mode);
        xmlview::set_theme(mode);
        history::set_theme(mode);
    });
    let w = ui.as_weak();
    ui.on_edit_mode_changed(move |on| {
        editor::set_editable(on);
        if let Some(ui) = w.upgrade() {
            history::refresh(&ui);
        }
    });
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    ui.on_cancel_task(|| with_app(|a| a.cancel.store(true, Ordering::Relaxed)));
}

/// Sound playback.
fn wire_sound(ui: &AppWindow) {
    let w = ui.as_weak();
    ui.on_audio_toggle(move || with_ui(&w, toggle_sound));
    let w = ui.as_weak();
    ui.on_audio_stop(move || with_ui(&w, stop_sound));
    let w = ui.as_weak();
    ui.on_audio_seek(move |f| with_ui(&w, |ui| seek_sound(ui, f)));
    let w = ui.as_weak();
    ui.on_audio_save_wav(move || with_ui(&w, save_wav));
    let w = ui.as_weak();
    ui.on_audio_step(move |d| {
        with_ui(&w, |ui| {
            step_sound(ui, d, true);
        })
    });
    let w = ui.as_weak();
    ui.on_audio_skip(move |secs| {
        with_ui(&w, |ui| {
            let Some(pcm) = with_app(|a| a.sound.clone()) else { return };
            let total = pcm.duration_secs().max(0.001);
            let now = with_app(|a| match &a.output {
                Some(o) if Arc::ptr_eq(o.pcm(), &pcm) && !o.finished() => o.position().as_secs_f32(),
                _ => a.sound_start,
            });
            seek_sound(ui, ((now + secs) / total).clamp(0.0, 1.0));
        })
    });
}

/// Changing archives and textures, undo and where changes are written.
fn wire_archive(ui: &AppWindow) {
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
    ui.on_write_target_changed(move |on| {
        with_ui(&w, |ui| {
            let Some(game_dir) = with_app(|a| a.game_dir.clone()) else { return };
            let target = if on { install::WriteTarget::Game } else { install::WriteTarget::Overlay };
            match install::set_write_target(&game_dir, target) {
                Ok(()) => ui.set_status(
                    if on {
                        "Changes now go into the game files; originals are kept and can be restored in History."
                            .to_owned()
                    } else {
                        format!("Changes now go into the {} folder.", overlay_name())
                    }
                    .into(),
                ),
                Err(e) => show_error(ui, format!("{e:#}")),
            }
            show_write_target(ui);
        })
    });
    let w = ui.as_weak();
    ui.on_texture_replace(move || with_ui(&w, |ui| edit_texture(ui, TextureAction::Replace)));
    let w = ui.as_weak();
    ui.on_texture_add(move || with_ui(&w, |ui| edit_texture(ui, TextureAction::Add)));
    let w = ui.as_weak();
    ui.on_texture_remove(move || with_ui(&w, |ui| edit_texture(ui, TextureAction::Remove)));
}

/// The history of changes and OpenIV packages.
fn wire_mods(ui: &AppWindow) {
    let w = ui.as_weak();
    ui.on_show_history(move || {
        with_ui(&w, |ui| {
            if ui.get_history_in_window() {
                history::open(ui);
            } else {
                navigate(ui, browser::HISTORY_PATH, true);
            }
        })
    });
    let w = ui.as_weak();
    ui.on_history_window_changed(move |on| {
        with_app(|a| a.cfg.history_window = Some(on));
        with_ui(&w, save_settings);
    });
    let w = ui.as_weak();
    ui.on_install_oiv(move || with_ui(&w, pick_package));
    let w = ui.as_weak();
    ui.on_oiv_confirm(move || with_ui(&w, install_package));
    let w = ui.as_weak();
    ui.on_uninstall_group(move || with_ui(&w, uninstall_selected_group));
}

/// The About dialog and loading native names.
fn wire_about(ui: &AppWindow) {
    register_about(ui);
    let w = ui.as_weak();
    ui.on_load_natives(move || {
        let w = w.clone();
        std::thread::spawn(move || {
            let Some(file) = rfd::FileDialog::new()
                .set_title("Load native names (natives.json in NativeDB format)")
                .add_filter("JSON", &["json"])
                .pick_file()
            else {
                return;
            };
            let msg = match openrage_app::natives::import(&file) {
                Ok(n) => format!("Loaded {n} native names; script disassemblies show them from now on."),
                Err(e) => format!("Cannot load native names: {e:#}"),
            };
            let _ = w.upgrade_in_event_loop(move |ui| ui.set_status(msg.into()));
        });
    });
}

/// Choosing a game and the setup dialog.
fn wire_setup(ui: &AppWindow) {
    let w = ui.as_weak();
    ui.on_game_chosen(move |i, remember| with_ui(&w, |ui| open_game(ui, i as usize, remember)));
    let w = ui.as_weak();
    ui.on_open_setup(move || with_ui(&w, |ui| show_setup(ui, true)));
    let w = ui.as_weak();
    ui.on_setup_pick_detected(move |value| {
        with_ui(&w, |ui| {
            ui.set_setup_game_dir(value);
            show_folder_hint(ui);
        })
    });
    let w = ui.as_weak();
    ui.on_setup_game_dir_edited(move || with_ui(&w, show_folder_hint));
    let w = ui.as_weak();
    ui.on_setup_browse_game(move || {
        pick_path(w.clone(), false, "Select the game folder", |ui, p| {
            ui.set_setup_game_dir(p.display().to_string().into());
            show_folder_hint(ui);
        })
    });
    let w = ui.as_weak();
    ui.on_setup_browse_magic(move || {
        pick_path(w.clone(), true, "Select magic.dat", |ui, p| ui.set_setup_magic_path(p.display().to_string().into()))
    });
    let w = ui.as_weak();
    ui.on_setup_run(move || with_ui(&w, run_setup));
}

pub(crate) fn theme_name(mode: i32) -> &'static str {
    match mode {
        1 => "light",
        2 => "dark",
        _ => "system",
    }
}

pub(crate) fn theme_mode(name: Option<&str>) -> i32 {
    match name {
        Some("light") => 1,
        Some("dark") => 2,
        _ => 0,
    }
}

/// Context-menu action on a file row.
pub(crate) fn row_action(ui: &AppWindow, row: usize, action: &str) {
    if ui.get_file_selected() != row as i32 {
        select_row(ui, row);
    }
    if let Some(index) = action.strip_prefix("plugin:").and_then(|i| i.parse::<usize>().ok()) {
        let target = with_app(|a| {
            let listing = a.listing.as_ref()?;
            let item = listing.items.get(*a.visible.get(row)?)?;
            let vpath = item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name));
            Some((item.name.clone(), vpath, plugin_target(item)?))
        });
        if let Some((name, vpath, folder)) = target {
            match plugins::action_at(&name, folder, index) {
                Some(action) => run_plugin_action(ui, action, vpath),
                None => ui.set_status("This plugin action is no longer available.".into()),
            }
        }
        return;
    }
    match action {
        "open" => ui.invoke_file_open(row as i32),
        "view" => open_viewer(ui),
        "edit" | "system" => {
            let vpath = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = listing.items.get(*a.visible.get(row)?)?;
                Some(item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = vpath {
                if action == "edit" {
                    open_editor(ui, vpath);
                } else {
                    open_with_system(ui, vpath);
                }
            }
        }
        "swf" => {
            let vpath = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = listing.items.get(*a.visible.get(row)?)?;
                Some(item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = vpath {
                export_swf(ui, vpath);
            }
        }
        "text-export" => {
            let vpath = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = listing.items.get(*a.visible.get(row)?)?;
                Some(item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = vpath {
                export_text_table(ui, vpath);
            }
        }
        "xml-view" | "xml" => {
            let vpath = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = listing.items.get(*a.visible.get(row)?)?;
                Some(item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name)))
            });
            if let Some(vpath) = vpath {
                if action == "xml" {
                    export_xml(ui, vpath);
                } else {
                    open_xml_view(ui, vpath);
                }
            }
        }
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
        "copy-hash" | "search-name" | "show-in-folder" => {
            let item = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = listing.items.get(*a.visible.get(row)?)?;
                Some((item.name.clone(), item.vpath.clone()))
            });
            let Some((name, vpath)) = item else { return };
            let stem = name.split('.').next().unwrap_or(&name).to_owned();
            match action {
                "copy-hash" => {
                    let text = format!("0x{:08X}", rage_core::hash::joaat(&stem.to_ascii_lowercase()));
                    let msg = match copy_to_clipboard(&text) {
                        Ok(()) => format!("Copied {text} (hash of {stem})"),
                        Err(e) => format!("Cannot copy to the clipboard: {e}"),
                    };
                    ui.set_status(msg.into());
                }
                "search-name" => start_search(ui, stem, true),
                _ => {
                    if let Some(v) = vpath {
                        let folder = browser::parent(&v).unwrap_or_default();
                        let file = v.rsplit('/').next().unwrap_or(&v).to_owned();
                        navigate_then(ui, &folder, true, move |ui| {
                            let row = with_app(|a| {
                                let l = a.listing.as_ref()?;
                                a.visible
                                    .iter()
                                    .position(|&i| l.items.get(i).is_some_and(|it| it.name.eq_ignore_ascii_case(&file)))
                            });
                            if let Some(row) = row {
                                ui.invoke_select_row(row as i32);
                            }
                        });
                    }
                }
            }
        }
        "copy-name" | "copy-path" => {
            let text = with_app(|a| {
                let listing = a.listing.as_ref()?;
                let item = listing.items.get(*a.visible.get(row)?)?;
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
