//! Games: choosing one, and the first-run setup that finds the keys.

use crate::*;

/// The games set up so far that are still installed.
pub(crate) fn installed_games(cfg: &Config) -> Vec<PathBuf> {
    cfg.known_games().into_iter().filter(|d| detect::is_gta5_dir(d) || is_rdr2_dir(d) || is_gta4_dir(d)).collect()
}

/// Fills the game switcher (File → Switch game…, settings).
pub(crate) fn refresh_games(ui: &AppWindow) {
    let (rows, ask) = with_app(|a| {
        let rows: Vec<GameRow> = installed_games(&a.cfg)
            .iter()
            .map(|d| GameRow {
                name: detect::game_name(d).unwrap_or("Game").into(),
                path: d.display().to_string().into(),
                is_default: a.cfg.default_game.as_ref() == Some(d),
                is_current: a.game_dir.as_ref() == Some(d),
            })
            .collect();
        (rows, a.cfg.default_game.is_none())
    });
    ui.set_games(ModelRc::new(VecModel::from(rows)));
    ui.set_ask_game(ask);
}

/// Opens a game that was set up before (from the chooser); `remember` makes it the default.
pub(crate) fn open_game(ui: &AppWindow, index: usize, remember: bool) {
    let Some(dir) = with_app(|a| installed_games(&a.cfg).get(index).cloned()) else { return };
    let ready = if is_rdr2_dir(&dir) {
        register_rdr2(&dir)
    } else if is_gta4_dir(&dir) {
        openrage_app::gta4::register(&dir).is_ok()
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
        ui.set_setup_magic_path(a.cfg.magic_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default().into());
        ui.set_setup_can_cancel(
            can_cancel
                && (a.keys.is_some() || rage_rpf::rpf8::has_default_keys() || rage_rpf::gta4::default_key().is_some())
                && a.game_dir.is_some(),
        );
    });
    let model: Vec<SharedString> = detected.into_iter().map(Into::into).collect();
    ui.set_setup_detected(ModelRc::new(VecModel::from(model)));
    ui.set_setup_log(SharedString::new());
    ui.set_setup_visible(true);
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

pub(crate) fn run_setup(ui: &AppWindow) {
    let game_dir = openrage_app::gta4::normalize_dir(&PathBuf::from(ui.get_setup_game_dir().trim()));
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
        return;
    }
    if !detect::is_gta5_dir(&game_dir) {
        ui.set_setup_log(
            format!("{} does not contain GTA5_Enhanced.exe, GTA5.exe, GTAIV.exe or RDR2.exe.", game_dir.display())
                .into(),
        );
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
