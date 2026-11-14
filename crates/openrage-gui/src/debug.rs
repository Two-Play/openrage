//! Debug snapshots of the window (`OPENRAGE_DEBUG_*`), for checking the UI without a screen.

use crate::*;

/// Development aid (debug builds only): `OPENRAGE_SNAPSHOT=out.png` saves an
/// image of this window after a few seconds and quits; `OPENRAGE_THEME`
/// (`light`/`dark`), `OPENRAGE_VIEWER`, `OPENRAGE_EDIT`, `OPENRAGE_UNLOCK` and
/// `OPENRAGE_ABOUT` set up the state to capture.
#[cfg(debug_assertions)]
pub(crate) fn debug_snapshot(ui: &AppWindow) {
    if let Ok(theme) = std::env::var("OPENRAGE_THEME") {
        ui.set_theme_mode(theme_mode(Some(&theme)));
    }
    let Ok(path) = std::env::var("OPENRAGE_SNAPSHOT") else { return };
    let weak = ui.as_weak();
    slint::Timer::single_shot(Duration::from_secs(5), move || {
        let Some(ui) = weak.upgrade() else { return };
        debug_viewer(&ui);
        if std::env::var("OPENRAGE_FLY").is_ok()
            && let Some(v) = viewer::newest()
        {
            v.invoke_toggle_fly();
        }
        if std::env::var("OPENRAGE_EDIT").is_ok() {
            ui.set_edit_mode(true);
        }
        if let Ok(i) = std::env::var("OPENRAGE_PICK_GAME") {
            ui.set_game_chooser_visible(false);
            ui.invoke_game_chosen(i.parse().unwrap_or(0), false);
        }
        // `OPENRAGE_CONFIRM_PLUGIN=1`: press Install in the plugin package dialog.
        if std::env::var("OPENRAGE_CONFIRM_PLUGIN").is_ok() && ui.get_plugin_pkg_visible() {
            ui.invoke_plugin_pkg_confirm();
        }
        if let Ok(page) = std::env::var("OPENRAGE_SETTINGS") {
            ui.set_settings_scroll_end(page == "plugins");
            ui.invoke_open_settings();
        }
        // `OPENRAGE_ABOUT=<tab>` opens the About dialog on that tab (0–2).
        if let Ok(tab) = std::env::var("OPENRAGE_ABOUT") {
            ui.invoke_about_open();
            ui.set_about_tab(tab.parse().unwrap_or(0));
        }
        if let Ok(name) = std::env::var("OPENRAGE_SELECT") {
            let row = with_app(|a| {
                let l = a.listing.as_ref()?;
                a.visible.iter().position(|&i| l.items[i].name.eq_ignore_ascii_case(&name))
            });
            if let Some(row) = row {
                select_row(&ui, row);
                // `OPENRAGE_PLUGIN_RUN=<n>` runs the selected file's n-th plugin action.
                if let Ok(i) = std::env::var("OPENRAGE_PLUGIN_RUN") {
                    row_action(&ui, row, &format!("plugin:{i}"));
                }
            }
        }
        // `OPENRAGE_CLICK=x,y[,right]` clicks there, e.g. to capture a context menu.
        debug_click(ui.window(), "OPENRAGE_CLICK");
        // Wait (up to a minute) until the models for a clip dictionary are found.
        let waiting = std::rc::Rc::new(slint::Timer::default());
        let started = Instant::now();
        let (weak, timer) = (ui.as_weak(), std::rc::Rc::clone(&waiting));
        waiting.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
            let loading =
                std::env::var("OPENRAGE_NO_WAIT").is_err() && weak.upgrade().is_some_and(|ui| ui.get_preview_loading());
            if (with_app(|a| a.suggest_pending || a.busy) || loading) && started.elapsed() < Duration::from_secs(60) {
                return;
            }
            timer.stop();
            if let Some(ui) = weak.upgrade() {
                debug_snapshot_rest(&ui, path.clone());
            }
        });
    });
}

/// The viewer part of [`debug_snapshot`]: `OPENRAGE_VIEWER` opens the selected
/// model, then the viewer hooks run.
#[cfg(debug_assertions)]
pub(crate) fn debug_viewer(ui: &AppWindow) {
    if std::env::var("OPENRAGE_VIEWER").is_ok() {
        open_viewer(ui);
    }
    if let Some(v) = viewer::newest() {
        if let Ok(tab) = std::env::var("OPENRAGE_TAB") {
            v.set_panel_tab(tab.parse().unwrap_or(0));
        }
        // `OPENRAGE_OUTFIT=<n>`: an RDR2 ped's outfit (loads in the background).
        if let Ok(n) = std::env::var("OPENRAGE_OUTFIT") {
            let n = n.parse().unwrap_or(0);
            v.set_outfit_index(n);
            v.invoke_outfit_selected(n);
        }
        // `OPENRAGE_ANIM_SEARCH=<text or .ycd path>` fills the clip dictionary results first.
        if let Ok(text) = std::env::var("OPENRAGE_ANIM_SEARCH") {
            v.invoke_anim_search(text.into());
        }
        if let Ok(spec) = std::env::var("OPENRAGE_ANIM") {
            use slint::Model;
            let (result, clip) = spec.split_once(':').unwrap_or(("0", "walk"));
            v.invoke_anim_open(result.parse().unwrap_or(0));
            let rows = v.get_anim_clips();
            if let Some(i) = (0..rows.row_count()).find(|&i| rows.row_data(i).is_some_and(|r| r.name == clip)) {
                v.invoke_anim_select(i as i32);
            }
        }
        if let Ok(hex) = std::env::var("OPENRAGE_PAINT") {
            use slint::Model;
            let rows = v.get_materials();
            for i in 0..rows.row_count() {
                if rows.row_data(i).is_some_and(|r| r.label == "Primary paint") {
                    v.invoke_material_hex(i as i32, hex.clone().into());
                }
            }
        }
    }
}

/// The part of [`debug_snapshot`] after selecting a file.
#[cfg(debug_assertions)]
pub(crate) fn debug_snapshot_rest(ui: &AppWindow, path: String) {
    // A file selected by `OPENRAGE_SELECT` opens in the viewer only now.
    if viewer::newest().is_none() && std::env::var("OPENRAGE_SELECT").is_ok() {
        debug_viewer(ui);
    }
    if let Ok(i) = std::env::var("OPENRAGE_SUGGEST") {
        ui.invoke_suggestion_open(i.parse().unwrap_or(0));
    }
    if std::env::var("OPENRAGE_HISTORY").is_ok() {
        history::open(ui);
    }
    if let Ok(path) = std::env::var("OPENRAGE_EDITOR") {
        open_editor(ui, path);
    }
    if let Ok(path) = std::env::var("OPENRAGE_TEXT") {
        open_text_file(ui, path);
    }
    if let Ok(path) = std::env::var("OPENRAGE_XML") {
        open_xml_view(ui, path);
    }
    if std::env::var("OPENRAGE_UNLOCK").is_ok() {
        ui.set_unlock_visible(true);
    }
    // `OPENRAGE_CAST=<row>` with `OPENRAGE_XML=<cut>`: press that actor's Play animation.
    if let Ok(row) = std::env::var("OPENRAGE_CAST") {
        if let Some(x) = xmlview::newest() {
            x.invoke_play_actor(row.parse().unwrap_or(0));
        }
        let path = path.clone();
        slint::Timer::single_shot(Duration::from_secs(20), move || {
            if let Some(Ok(img)) = viewer::newest().map(|v| v.window().take_snapshot()) {
                let _ =
                    rage_formats::png(img.width(), img.height(), img.as_bytes()).map(|png| std::fs::write(&path, png));
            }
            let _ = slint::quit_event_loop();
        });
        return;
    }
    // `OPENRAGE_SCENE=<cut>` (+ `OPENRAGE_SCENE_TIME`, `OPENRAGE_FREE`): the whole cutscene.
    // `OPENRAGE_SCENE_SNAP_AT=<ms>` takes the picture that soon (while loading);
    // `OPENRAGE_SCENE_PLAY=<s>` plays that long and quits without a picture.
    // `OPENRAGE_MAP=<ymap>` (GTA IV: `.wpl` or a map `.img`): the map file's 3D scene, once its models have loaded.
    if let Ok(map) = std::env::var("OPENRAGE_MAP") {
        let lower = map.to_ascii_lowercase();
        if lower.ends_with(".wpl") || lower.ends_with(".img") {
            scene::open_map(ui, map);
        } else {
            open_xml_view(ui, map);
            if let Some(x) = xmlview::newest() {
                x.invoke_show_map();
            }
        }
        let waiting = std::rc::Rc::new(slint::Timer::default());
        let (started, timer) = (Instant::now(), std::rc::Rc::clone(&waiting));
        waiting.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
            if !scene::ready() && started.elapsed() < Duration::from_secs(180) {
                return;
            }
            timer.stop();
            let path = path.clone();
            slint::Timer::single_shot(Duration::from_millis(800), move || {
                if let Some(Ok(img)) = scene::newest().map(|w| w.window().take_snapshot()) {
                    let _ = rage_formats::png(img.width(), img.height(), img.as_bytes())
                        .map(|png| std::fs::write(&path, png));
                }
                let _ = slint::quit_event_loop();
            });
        });
        return;
    }
    if let Ok(cut) = std::env::var("OPENRAGE_SCENE") {
        if with_app(|a| a.game_dir.as_deref().is_some_and(openrage_app::gta4::is_gta4_dir)) {
            scene::open_gta4(ui, cut);
        } else {
            open_xml_view(ui, cut);
            if let Some(x) = xmlview::newest() {
                x.invoke_play_scene();
            }
        }
        if let Ok(at) = std::env::var("OPENRAGE_SCENE_SNAP_AT") {
            let path = path.clone();
            slint::Timer::single_shot(Duration::from_millis(at.parse().unwrap_or(1000)), move || {
                if let Some(Ok(img)) = scene::newest().map(|w| w.window().take_snapshot()) {
                    let _ = rage_formats::png(img.width(), img.height(), img.as_bytes())
                        .map(|png| std::fs::write(&path, png));
                }
                let _ = slint::quit_event_loop();
            });
            return;
        }
        let waiting = std::rc::Rc::new(slint::Timer::default());
        let (started, timer) = (Instant::now(), std::rc::Rc::clone(&waiting));
        waiting.start(slint::TimerMode::Repeated, Duration::from_millis(500), move || {
            if !scene::ready() && started.elapsed() < Duration::from_secs(180) {
                return;
            }
            timer.stop();
            if let Ok(secs) = std::env::var("OPENRAGE_SCENE_PLAY") {
                slint::Timer::single_shot(Duration::from_secs(secs.parse().unwrap_or(10)), || {
                    let _ = slint::quit_event_loop();
                });
                return;
            }
            let time = std::env::var("OPENRAGE_SCENE_TIME").ok().and_then(|t| t.parse().ok()).unwrap_or(0.0);
            scene::debug_pose(time, std::env::var("OPENRAGE_FREE").is_ok());
            let path = path.clone();
            slint::Timer::single_shot(Duration::from_millis(800), move || {
                if let Some(Ok(img)) = scene::newest().map(|w| w.window().take_snapshot()) {
                    let _ = rage_formats::png(img.width(), img.height(), img.as_bytes())
                        .map(|png| std::fs::write(&path, png));
                }
                let _ = slint::quit_event_loop();
            });
        });
        return;
    }
    let weak = ui.as_weak();
    // An outfit loading in the background needs a few more seconds.
    let wait = if std::env::var("OPENRAGE_OUTFIT").is_ok() { 30 } else { 2 };
    slint::Timer::single_shot(Duration::from_secs(wait), move || {
        let Some(ui) = weak.upgrade() else { return };
        let snap = match (viewer::newest(), editor::newest(), history::window()) {
            _ if let Some(x) = xmlview::newest() => {
                if let Ok(tab) = std::env::var("OPENRAGE_TAB") {
                    x.set_tab(tab.parse().unwrap_or(0));
                }
                if let Ok(filter) = std::env::var("OPENRAGE_FILTER") {
                    x.set_filter(filter.clone().into());
                    x.invoke_filter_edited(filter.into());
                }
                // `OPENRAGE_NODE_ACTION=row,action` runs a tree row's menu action.
                if let Ok(spec) = std::env::var("OPENRAGE_NODE_ACTION")
                    && let Some((row, action)) = spec.split_once(',')
                {
                    if action == "select" {
                        x.invoke_select(row.parse().unwrap_or(0));
                    } else {
                        x.invoke_node_action(row.parse().unwrap_or(0), action.into());
                    }
                }
                // `OPENRAGE_WINDOW_CLICK=x,y[,right]` clicks into this window first.
                debug_click(x.window(), "OPENRAGE_WINDOW_CLICK");
                x.window().take_snapshot()
            }
            (Some(v), ..) => {
                debug_click(v.window(), "OPENRAGE_WINDOW_CLICK");
                v.window().take_snapshot()
            }
            (None, Some(e), _) => {
                debug_click(e.window(), "OPENRAGE_WINDOW_CLICK");
                e.window().take_snapshot()
            }
            (None, None, Some(h)) => h.window().take_snapshot(),
            (None, None, None) => ui.window().take_snapshot(),
        };
        match snap {
            Ok(img) => {
                let _ =
                    rage_formats::png(img.width(), img.height(), img.as_bytes()).map(|png| std::fs::write(&path, png));
            }
            Err(e) => eprintln!("snapshot failed: {e}"),
        }
        let _ = slint::quit_event_loop();
    });
}

/// Clicks into a window at `x,y[,right]` read from the environment variable `var`.
#[cfg(debug_assertions)]
pub(crate) fn debug_click(w: &slint::Window, var: &str) {
    let Ok(at) = std::env::var(var) else { return };
    let parts: Vec<&str> = at.split(',').map(str::trim).collect();
    if let [x, y, rest @ ..] = parts.as_slice()
        && let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>())
    {
        use slint::platform::{PointerEventButton, WindowEvent};
        let button = if rest.first() == Some(&"right") { PointerEventButton::Right } else { PointerEventButton::Left };
        let position = slint::LogicalPosition::new(x, y);
        w.dispatch_event(WindowEvent::PointerMoved { position });
        w.dispatch_event(WindowEvent::PointerPressed { position, button });
        w.dispatch_event(WindowEvent::PointerReleased { position, button });
    }
}
