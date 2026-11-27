//! Games: choosing one, and the first-run setup that finds the keys.

use crate::*;

/// The games set up so far that are still installed.
pub(crate) fn installed_games(cfg: &Config) -> Vec<PathBuf> {
    cfg.known_games().into_iter().filter(|d| detect::is_game_dir(d)).collect()
}

thread_local! {
    /// Game icons by folder, read once from the games' programs.
    static ICONS: std::cell::RefCell<std::collections::HashMap<PathBuf, Option<slint::Image>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The icon of a game's own program (read once per folder).
fn game_icon(dir: &Path) -> Option<slint::Image> {
    ICONS.with(|icons| {
        icons
            .borrow_mut()
            .entry(dir.to_path_buf())
            .or_insert_with(|| {
                let (w, h, rgba) = detect::game_icon(dir)?;
                let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, w, h);
                Some(slint::Image::from_rgba8(buffer))
            })
            .clone()
    })
}

/// Fills the game switcher (File → Switch game…, settings).
pub(crate) fn refresh_games(ui: &AppWindow) {
    let (rows, ask) = with_app(|a| {
        let rows: Vec<GameRow> = installed_games(&a.cfg)
            .iter()
            .map(|d| {
                let icon = game_icon(d);
                GameRow {
                    name: detect::game_name(d).unwrap_or("Game").into(),
                    path: d.display().to_string().into(),
                    is_default: a.cfg.default_game.as_ref() == Some(d),
                    is_current: a.game_dir.as_ref() == Some(d),
                    has_icon: icon.is_some(),
                    icon: icon.unwrap_or_default(),
                }
            })
            .collect();
        (rows, a.cfg.default_game.is_none())
    });
    ui.set_games(ModelRc::new(VecModel::from(rows)));
    ui.set_ask_game(ask);
}

/// What the start window ended with.
pub(crate) enum LaunchChoice {
    /// A game of the list, and whether to always open it.
    Game(usize, bool),
    AddGame,
}

/// Runs the start window (several games, none the default) in an event loop
/// of its own, before the explorer's: the explorer then starts the usual way
/// with the game chosen. `Ok(None)`: closed without a choice. `Err`: the
/// window could not be shown.
pub(crate) fn run_launcher(ui: &AppWindow) -> Result<Option<LaunchChoice>, slint::PlatformError> {
    let win = LauncherWindow::new()?;
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(&win);
    win.set_games(ui.get_games());
    let choice: std::rc::Rc<std::cell::RefCell<Option<LaunchChoice>>> = Default::default();
    // After a choice the window shows "Opening …" (set by the window itself)
    // and stays open while the explorer gets ready; its event loop ends a
    // moment later, once that has been drawn.
    let finish = |c: &std::rc::Rc<std::cell::RefCell<Option<LaunchChoice>>>, made: LaunchChoice| {
        *c.borrow_mut() = Some(made);
        slint::Timer::single_shot(Duration::from_millis(80), || {
            let _ = slint::quit_event_loop();
        });
    };
    let c = std::rc::Rc::clone(&choice);
    win.on_chosen(move |i, remember| finish(&c, LaunchChoice::Game(i.max(0) as usize, remember)));
    let c = std::rc::Rc::clone(&choice);
    win.on_add_game(move || finish(&c, LaunchChoice::AddGame));
    #[cfg(debug_assertions)]
    debug_launcher(&win);
    win.show()?;
    // Ends after a choice, or when the window is closed (no choice).
    slint::run_event_loop()?;
    let made = choice.borrow_mut().take();
    if made.is_some() {
        LAUNCHER.with(|l| *l.borrow_mut() = Some(win));
    }
    Ok(made)
}

thread_local! {
    /// The start window while the explorer gets ready.
    static LAUNCHER: std::cell::RefCell<Option<LauncherWindow>> = const { std::cell::RefCell::new(None) };
}

/// Closes the start window once the explorer is up.
pub(crate) fn close_launcher() {
    if let Some(win) = LAUNCHER.with(|l| l.borrow_mut().take()) {
        let _ = win.hide();
    }
}

/// Opens a game that was set up before (from the chooser); `remember` makes it the default.
pub(crate) fn open_game(ui: &AppWindow, index: usize, remember: bool) {
    let Some(dir) = with_app(|a| installed_games(&a.cfg).get(index).cloned()) else { return };
    let ready = if is_rdr2_dir(&dir) {
        register_rdr2(&dir)
    } else if is_gta4_dir(&dir) {
        openrage_app::gta4::register(&dir).is_ok()
    } else if detect::is_gta6_dir(&dir) {
        true
    } else {
        with_app(|a| a.keys.is_some())
    };
    let saved = with_app(|a| {
        a.game_dir = Some(dir.clone());
        a.cfg.game_dir = Some(dir.clone());
        if remember {
            a.cfg.default_game = Some(dir.clone());
        }
        a.cfg.save()
    });
    refresh_games(ui);
    if ready {
        start_browsing(ui);
    } else {
        show_setup(ui, true);
    }
    if let Err(e) = saved {
        show_error(ui, format!("Could not save the settings: {e}"));
    }
}

/// Loads the RDR2 archive keys (from `openrage rdr2-keys`) and the cached file
/// names for archives opened through the virtual file system; false without
/// keys. A damaged key file is reported on stderr and counts as no keys (the
/// setup then asks for them again).
pub(crate) fn register_rdr2(game_dir: &Path) -> bool {
    openrage_app::rdr2::register(game_dir).unwrap_or_else(|e| {
        eprintln!("RDR2 keys: {e:#}");
        false
    })
}

pub(crate) fn show_setup(ui: &AppWindow, can_cancel: bool) {
    let detected: Vec<String> = detect::find_installs().iter().map(|p| p.display().to_string()).collect();
    with_app(|a| {
        let game = a.game_dir.clone().or_else(|| a.cfg.game_dir.clone()).map(|p| p.display().to_string());
        let game = game.or_else(|| detected.first().cloned()).unwrap_or_default();
        ui.set_setup_game_dir(game.into());
        // A key blob that is gone (moved, or a temporary copy) is not offered again.
        let magic = a.cfg.magic_path.as_ref().filter(|p| p.is_file());
        ui.set_setup_magic_path(magic.map(|p| p.display().to_string()).unwrap_or_default().into());
        // Closing goes back to a game that is open and readable.
        let open = a.game_dir.as_deref().is_some_and(|d| {
            detect::is_gta6_dir(d)
                || a.keys.is_some()
                || rage_rpf::rpf8::has_default_keys()
                || rage_rpf::gta4::default_key().is_some()
        });
        ui.set_setup_can_cancel(can_cancel && open);
        ui.set_setup_has_games(!installed_games(&a.cfg).is_empty());
    });
    let model: Vec<SharedString> = detected.into_iter().map(Into::into).collect();
    ui.set_setup_detected(ModelRc::new(VecModel::from(model)));
    show_folder_hint(ui);
    ui.set_setup_visible(true);
}

/// Says right away whether the folder in the setup holds a game OpenRAGE knows.
pub(crate) fn show_folder_hint(ui: &AppWindow) {
    let text = ui.get_setup_game_dir();
    let dir = openrage_app::gta4::normalize_dir(&PathBuf::from(text.trim()));
    let hint = if text.trim().is_empty() {
        String::new()
    } else if let Some(name) = detect::game_name(&dir) {
        if detect::is_gta6_dir(&dir) {
            format!(
                "Found {name}. RPF9 archives open read-only in recovery view: original names and encrypted files are unavailable. The first scan can take several minutes. Press Open to continue."
            )
        } else {
            format!("Found {name}. Press Open to continue.")
        }
    } else if dir.is_dir() {
        format!("No supported game in {}. {NO_GAME}", dir.display())
    } else {
        format!("{} does not exist.", dir.display())
    };
    ui.set_setup_log(hint.into());
}

pub(crate) fn pick_path(w: Weak<AppWindow>, file: bool, title: &'static str, apply: fn(&AppWindow, PathBuf)) {
    std::thread::spawn(move || {
        let dialog = rfd::FileDialog::new().set_title(title);
        let picked = if file { dialog.add_filter("Key blob", &["dat"]).pick_file() } else { dialog.pick_folder() };
        if let Some(p) = picked {
            let _ = w.upgrade_in_event_loop(move |ui| apply(&ui, p));
        }
    });
}

/// What a game folder must hold.
const NO_GAME: &str = "The folder must contain GTA5_Enhanced.exe, GTA5.exe, GTAIV.exe, RDR2.exe or game_scarlett_master.exe, or RPF9 archives of GTA VI.";

/// Leaves a setup that cannot be closed: back to the list of games when there
/// is one, otherwise out of OpenRAGE.
pub(crate) fn leave_setup(ui: &AppWindow) {
    if with_app(|a| installed_games(&a.cfg).is_empty()) {
        let _ = slint::quit_event_loop();
    } else {
        ui.set_setup_visible(false);
        ui.invoke_open_game_chooser(false);
    }
}

/// Opens a game that needs no keys.
fn open_without_keys(ui: &AppWindow, game_dir: PathBuf) {
    let saved = with_app(|a| {
        a.cfg.remember_game(&game_dir);
        a.game_dir = Some(game_dir.clone());
        a.cfg.game_dir = Some(game_dir);
        a.cfg.save()
    });
    ui.set_setup_visible(false);
    refresh_games(ui);
    start_browsing(ui);
    if let Err(e) = saved {
        show_error(ui, format!("Could not save the settings: {e}"));
    }
}

pub(crate) fn run_setup(ui: &AppWindow) {
    let game_dir = openrage_app::gta4::normalize_dir(&PathBuf::from(ui.get_setup_game_dir().trim()));
    if detect::is_gta6_dir(&game_dir) {
        open_without_keys(ui, game_dir);
        return;
    }
    if is_gta4_dir(&game_dir) {
        ui.set_setup_running(true);
        ui.set_setup_log("Looking for the archive key in GTAIV.exe…".into());
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let result = openrage_app::gta4::register(&game_dir);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_setup_running(false);
                match result {
                    Ok(_) => {
                        let saved = with_app(|a| {
                            a.cfg.remember_game(&game_dir);
                            a.game_dir = Some(game_dir.clone());
                            a.cfg.game_dir = Some(game_dir);
                            a.cfg.save()
                        });
                        ui.set_setup_visible(false);
                        refresh_games(&ui);
                        start_browsing(&ui);
                        if let Err(e) = saved {
                            show_error(&ui, format!("Could not save the settings: {e}"));
                        }
                    }
                    Err(e) => ui.set_setup_log(format!("Error: {e:#}").into()),
                }
            });
        });
        return;
    }
    let magic = Some(ui.get_setup_magic_path().trim().to_owned()).filter(|s| !s.is_empty()).map(PathBuf::from);
    if is_rdr2_dir(&game_dir) {
        if !register_rdr2(&game_dir) {
            ui.set_setup_log(
                "This is Red Dead Redemption 2. Its archive keys exist only while the game runs: make a memory dump of the running game and run\n  openrage rdr2-keys --dump <dump file> <game folder>\nonce (see the README), then try again. `openrage rdr2-names <game folder>` adds file names."
                    .into(),
            );
            return;
        }
        open_without_keys(ui, game_dir);
        return;
    }
    if !detect::is_gta5_dir(&game_dir) {
        ui.set_setup_log(format!("No supported game in {}. {NO_GAME}", game_dir.display()).into());
        return;
    }
    // Keys set up before that open this game: no scan, no key blob needed.
    if let Some(keys) = setup::cached_keys_for(&game_dir) {
        finish_setup(ui, game_dir, magic, keys);
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
                if let Some(required) = e.downcast_ref::<KeyBlobRequired>() {
                    push(&format!("{} does not contain the NG keys in plain form.", required.exe_name()));
                    push("This is so for GTA V Enhanced, and for some updates of GTA V Legacy.");
                    push("Select a CodeWalker key blob (magic.dat) and try again.");
                } else {
                    push(&format!("Error: {e:#}"));
                }
                let _ = weak.upgrade_in_event_loop(|ui| ui.set_setup_running(false));
            }
        }
    });
}

pub(crate) fn finish_setup(ui: &AppWindow, game_dir: PathBuf, magic: Option<PathBuf>, keys: Arc<GtaVKeys>) {
    let saved = with_app(|a| {
        a.keys = Some(keys);
        a.cfg.remember_game(&game_dir);
        a.game_dir = Some(game_dir.clone());
        a.cfg.game_dir = Some(game_dir);
        a.cfg.magic_path = magic;
        a.cfg.save()
    });
    ui.set_setup_running(false);
    ui.set_setup_visible(false);
    refresh_games(ui);
    start_browsing(ui);
    if let Err(e) = saved {
        show_error(ui, format!("Could not save settings: {e:#}"));
    }
}
