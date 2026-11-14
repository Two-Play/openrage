//! OpenRAGE graphical explorer.

#![cfg_attr(windows, windows_subsystem = "windows")]
// Slint's generated code allows `unsafe` for itself; our own code may not use it.
#![deny(unsafe_code)]

mod about;
mod accent;
mod archive_ops;
mod audio;
mod badges;
mod browser;
mod callbacks;
mod debug;
mod docs;
mod editor;
mod extract_ui;
mod gpu;
mod history;
mod model_ui;
mod mods_ui;
mod navigate;
mod plugins;
mod plugins_ui;
mod preview;
mod scene;
mod search;
mod setup_ui;
mod sound_ui;
mod util;
mod viewer;
mod xmlview;

use std::cell::RefCell;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use openrage_app::config::Config;
use openrage_app::detect::{self, is_rdr2_dir};
use openrage_app::extract::{extract_entry, human_size};
use openrage_app::gta4::is_gta4_dir;
use openrage_app::install::{self, Op};
use openrage_app::setup::{self, KeyBlobRequired, KeySource};
use openrage_app::{game_language_from_locale, index_path, load_keys, suggest};
use rage_crypto::GtaVKeys;
use rage_index::{EntryTag, HitReason, Index, LANGUAGES};
use rage_rpf::vfs::{self, Node};
use rage_rpf::{EntryId, EntryKind, Rpf7, RpfError};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use browser::{History, Item, ItemKind, Listing, Location, Mark, Tree};
use rage_mods::{ChangeKind, CompareMode, Origin};

use about::*;
use archive_ops::*;
use callbacks::*;
use debug::*;
use docs::*;
use extract_ui::*;
use model_ui::*;
use mods_ui::*;
use navigate::*;
use plugins_ui::*;
use preview::*;
use search::*;
use setup_ui::*;
use sound_ui::*;
use util::*;

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
    /// Bumped by each navigation; a folder listed in the background is shown
    /// only if no newer navigation started meanwhile.
    navigate_generation: Arc<AtomicU64>,
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
    /// The selected GTA IV model whose own textures the Textures button opens.
    model_textures_path: Option<String>,
    /// The selected sound, decoded, and the audio output playing it.
    sound: Option<Arc<rage_formats::awc::Pcm>>,
    output: Option<audio::Output>,
    /// Where playback starts (seconds), set by seeking while stopped.
    sound_start: f32,
    /// Play the selected sound as soon as it is decoded (double-click).
    play_when_ready: bool,
    audio_timer: slint::Timer,
    /// The list in the preview area: models suggested for a clip dictionary
    /// (`suggest_ycd` set), or the sounds of an audio container (`suggest_awc`).
    suggestions: Vec<String>,
    suggest_ycd: String,
    suggest_awc: String,
    /// Models for `suggest_ycd` are still being looked for.
    suggest_pending: bool,
    index_running: bool,
    /// Plugin package shown in the install dialog.
    plugin_package: Option<Arc<openrage_plugins::package::Package>>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(a.borrow_mut().as_mut().expect("app initialized")))
}

fn main() -> anyhow::Result<()> {
    let ui = AppWindow::new()?;
    // Wayland shows a window's icon only through its app id and the desktop
    // entry of the same name (`openrage.desktop`, icon `openrage`); X11 uses
    // it as WM_CLASS. Must be set before the first window is shown.
    if let Err(e) = slint::set_xdg_app_id("openrage") {
        eprintln!("app id: {e}");
    }
    let cfg = Config::load();
    plugins::reload(&cfg);
    let keys = load_keys().unwrap_or(None);
    // Several games set up and none chosen as default: ask (after the window is up).
    let known = installed_games(&cfg);
    let default = cfg.default_game.clone().filter(|d| known.contains(d));
    let ask = default.is_none() && known.len() > 1;
    let game_dir = if ask {
        None
    } else {
        default.or_else(|| cfg.game_dir.clone().filter(|d| known.contains(d))).or_else(|| known.first().cloned())
    };
    let rdr2_ready = game_dir.as_deref().is_some_and(|d| {
        (is_rdr2_dir(d) && register_rdr2(d)) || (is_gta4_dir(d) && openrage_app::gta4::register(d).is_ok())
    });

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
            navigate_generation: Arc::new(AtomicU64::new(0)),
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
            model_textures_path: None,
            sound: None,
            output: None,
            sound_start: 0.0,
            play_when_ready: false,
            audio_timer: slint::Timer::default(),
            suggestions: Vec::new(),
            suggest_ycd: String::new(),
            suggest_awc: String::new(),
            suggest_pending: false,
            index_running: false,
            plugin_package: None,
        })
    });

    ui.set_mono_font(mono_font().into());
    ui.set_theme_mode(with_app(|a| theme_mode(a.cfg.theme.as_deref())));
    let (mode, color) = with_app(|a| accent::from_setting(a.cfg.accent.as_deref()));
    accent::set(mode, color);
    accent::apply(&ui);
    ui.set_history_in_window(with_app(|a| a.cfg.history_window.unwrap_or(true)));
    let languages: Vec<SharedString> =
        std::iter::once("All languages".into()).chain(LANGUAGES.iter().map(|l| capitalize(l).into())).collect();
    ui.set_languages(ModelRc::new(VecModel::from(languages)));
    ui.set_language_index(with_app(|a| {
        a.language.as_deref().and_then(|l| LANGUAGES.iter().position(|x| *x == l)).map_or(0, |i| i as i32 + 1)
    }));
    wire_callbacks(&ui);

    // Shows the progress of background loads (e.g. "Loading textures 5/13").
    let progress_timer = slint::Timer::default();
    let w = ui.as_weak();
    progress_timer.start(slint::TimerMode::Repeated, Duration::from_millis(150), move || {
        let Some(ui) = w.upgrade() else { return };
        if !ui.get_preview_loading() {
            return;
        }
        match openrage_app::progress::get() {
            Some((label, done, total)) if total > 0 => {
                ui.set_preview_progress(done as f32 / total as f32);
                let step = (done + 1).min(total);
                ui.set_preview_progress_text(
                    format!("{label} {step}/{total} · {} %", done.min(total) * 100 / total).into(),
                );
            }
            Some((label, _, _)) => {
                ui.set_preview_progress(-1.0);
                ui.set_preview_progress_text(format!("{label}…").into());
            }
            None => {
                ui.set_preview_progress(-1.0);
                ui.set_preview_progress_text(SharedString::new());
            }
        }
    });
    refresh_games(&ui);
    let ready = with_app(|app| app.game_dir.is_some() && (app.keys.is_some() || rdr2_ready));
    if ask {
        ui.invoke_open_game_chooser(false);
    } else if ready {
        start_browsing(&ui);
    } else {
        show_setup(&ui, false);
    }
    #[cfg(debug_assertions)]
    debug_snapshot(&ui);
    let result = ui.run();
    gpu::shutdown();
    result?;
    Ok(())
}

fn with_ui(w: &Weak<AppWindow>, f: impl FnOnce(&AppWindow)) {
    if let Some(ui) = w.upgrade() {
        f(&ui);
    }
}
