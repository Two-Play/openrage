//! A whole cutscene in one scene: every actor with its animation, through the
//! cutscene camera or a free orbit camera (GPU only).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openrage_app::anim::Binding;
use openrage_app::cutscene::{self, Actor, Cutscene};
use openrage_app::models::Look;
use openrage_app::render::{self, Camera, View};
use rage_crypto::GtaVKeys;
use rage_formats::clip::ClipDictionary;
use rage_formats::math::{self, Mat4};
use rage_rpf::vfs::Node;
use slint::ComponentHandle;

use crate::{AppWindow, ModelPreview, SceneWindow};

struct SceneActor {
    preview: Arc<ModelPreview>,
    look: Look,
    clip: usize,
    /// Bone binding per part (section) of the clip; empty without a skeleton.
    bindings: Vec<Option<Binding>>,
}

struct State {
    dict: Arc<ClipDictionary>,
    actors: Vec<SceneActor>,
    camera_clip: Option<usize>,
    duration: f32,
    time: f32,
    playing: bool,
    last: Instant,
    cut_camera: bool,
    view: View,
    /// Scene bounds for the free camera.
    bounds: ([f32; 3], [f32; 3]),
    timer: slint::Timer,
}

type Shared = Rc<RefCell<Option<State>>>;

thread_local! {
    static OPEN: RefCell<Vec<(u64, SceneWindow, Shared)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Opens the scene of the cutscene at `cut_path`; `models` holds a model path
/// per actor (found through the search index), `None` where there is none.
pub fn open(ui: &AppWindow, cut_path: String, cut: Cutscene, models: Vec<Option<String>>) {
    let Ok(win) = SceneWindow::new() else { return };
    win.set_theme_mode(ui.get_theme_mode());
    let name = cut_path.rsplit('/').next().unwrap_or(&cut_path).to_owned();
    win.set_heading(name.into());
    win.set_path(cut_path.clone().into());
    win.set_duration(cut.duration.max(0.1));
    win.set_status("Loading the actors' models and the animation …".into());
    let state: Rc<RefCell<Option<State>>> = Rc::new(RefCell::new(None));
    wire(&win, &state);
    win.window().on_close_requested(|| {
        forget_closed();
        slint::CloseRequestResponse::HideWindow
    });
    let _ = win.show();
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    OPEN.with(|o| o.borrow_mut().push((id, win.clone_strong(), Rc::clone(&state))));

    let Some((game_dir, keys)) = crate::with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone()))) else { return };
    let weak = win.as_weak();
    std::thread::spawn(move || {
        let loaded = load(&game_dir, keys, &cut_path, &cut.actors, &models);
        let _ = weak.upgrade_in_event_loop(move |win| match loaded {
            Ok((dict, previews)) => {
                if let Some(state) =
                    OPEN.with(|o| o.borrow().iter().find(|(i, ..)| *i == id).map(|(_, _, s)| Rc::clone(s)))
                {
                    start(&win, &state, dict, &cut, previews);
                }
            }
            Err(e) => {
                win.set_loading(false);
                win.set_status(format!("Cannot load the cutscene: {e:#}").into());
            }
        });
    });
}

type Loaded = (ClipDictionary, Vec<Option<ModelPreview>>);

fn load(
    game_dir: &std::path::Path,
    keys: Option<Arc<GtaVKeys>>,
    cut_path: &str,
    actors: &[Actor],
    models: &[Option<String>],
) -> anyhow::Result<Loaded> {
    let (node, _) = rage_mods::resolve(game_dir, cut_path, keys.clone())?;
    let Node::ArchiveFile(rpf, id) = node else { anyhow::bail!("{cut_path} is not inside an archive") };
    let dict = cutscene::animation(&rpf, id)?;
    let game_dir: PathBuf = game_dir.to_owned();
    // Models load in parallel; most cutscenes have a dozen or more.
    let previews = std::thread::scope(|s| {
        let handles: Vec<_> = models
            .iter()
            .zip(actors)
            .map(|(m, actor)| {
                let (game_dir, keys) = (&game_dir, keys.clone());
                s.spawn(move || {
                    let mut loaded = openrage_app::models::load(game_dir, keys.clone(), m.as_ref()?).ok()?;
                    // Props whose textures live in the dictionary their archetype names.
                    if let Some(type_file) = &actor.type_file
                        && let Some(extra) = cutscene::prop_textures(game_dir, keys, type_file, &actor.name)
                    {
                        loaded.add_textures(extra.0, extra.1);
                    }
                    let materials = render::Materials::new(&loaded);
                    let name = loaded.drawables.first().map(|(n, _)| n.clone()).unwrap_or_default();
                    let look = std::sync::RwLock::new(loaded.default_look());
                    Some(ModelPreview { loaded, look, materials, name })
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    Ok((dict, previews))
}

fn start(
    win: &SceneWindow,
    state: &Rc<RefCell<Option<State>>>,
    dict: ClipDictionary,
    cut: &Cutscene,
    previews: Vec<Option<ModelPreview>>,
) {
    let clips = cutscene::actor_clips(&cut.actors, &dict);
    let mut actors = Vec::new();
    let (mut missing_model, mut missing_clip) = (0, 0);
    for ((actor, preview), clip) in cut.actors.iter().zip(previews).zip(clips) {
        if actor.kind.model_extensions().is_empty() {
            continue;
        }
        let Some(preview) = preview else {
            missing_model += 1;
            continue;
        };
        let Some(clip) = clip else {
            missing_clip += 1;
            continue;
        };
        let bindings = match &preview.loaded.skeleton {
            Some(sk) => dict.clips[clip]
                .parts
                .iter()
                .map(|p| dict.animations.get(p.animation).map(|a| Binding::new(sk, a)))
                .collect(),
            None => Vec::new(),
        };
        let mut look = preview.loaded.default_look();
        if actor.kind == cutscene::Kind::Ped {
            hide_oversized_parts(&preview, &mut look);
        }
        actors.push(SceneActor { preview: Arc::new(preview), look, clip, bindings });
    }
    let duration = dict.clips.iter().map(|c| c.duration()).fold(cut.duration, f32::max);
    let camera_clip = cutscene::camera_clip(&dict);
    let dict = Arc::new(dict);
    // Bounds for the free camera: where the actors stand at the start.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for a in &actors {
        let m = cutscene::mover(&dict, &dict.clips[a.clip], 0.0);
        let p = [m[12], m[13], m[14]];
        for k in 0..3 {
            lo[k] = lo[k].min(p[k] - 1.0);
            hi[k] = hi[k].max(p[k] + 1.0);
        }
    }
    if lo[0] > hi[0] {
        (lo, hi) = ([-1.0; 3], [1.0; 3]);
    }
    let mut status = format!("{} actors", actors.len());
    if missing_model > 0 {
        status.push_str(&format!(" · {missing_model} models not found"));
    }
    if missing_clip > 0 {
        status.push_str(&format!(" · {missing_clip} without animation"));
    }
    status.push_str(" · drag to turn the free camera, scroll to zoom, Space plays");
    win.set_status(status.into());
    win.set_loading(false);
    win.set_has_camera(camera_clip.is_some());
    win.set_cut_camera(camera_clip.is_some());
    win.set_duration(duration.max(0.1));
    *state.borrow_mut() = Some(State {
        dict,
        actors,
        camera_clip,
        duration,
        time: 0.0,
        playing: true,
        last: Instant::now(),
        cut_camera: camera_clip.is_some(),
        view: View { zoom: 2.2, ..View::default() },
        bounds: (lo, hi),
        timer: slint::Timer::default(),
    });
    win.set_playing(true);
    let (w, s) = (win.as_weak(), Rc::clone(state));
    if let Some(st) = state.borrow().as_ref() {
        st.timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || tick(&w, &s));
    }
    render_frame(win, state);
}

/// Some ped models carry large flat helper surfaces (25 m across); a person
/// is never wider than a few metres, so such parts are hidden in the scene.
fn hide_oversized_parts(preview: &ModelPreview, look: &mut Look) {
    for (i, part) in preview.loaded.parts.iter().enumerate() {
        let Some((_, d)) = preview.loaded.drawables.get(part.drawable) else { continue };
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in d.meshes.iter().flat_map(|m| &m.positions) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if (0..3).any(|k| hi[k] - lo[k] > 5.0)
            && let Some(v) = look.parts.get_mut(i)
        {
            *v = false;
        }
    }
}

fn tick(w: &slint::Weak<SceneWindow>, state: &Rc<RefCell<Option<State>>>) {
    let Some(win) = w.upgrade() else { return };
    {
        let mut st = state.borrow_mut();
        let Some(st) = st.as_mut() else { return };
        let now = Instant::now();
        if st.playing {
            st.time += now.duration_since(st.last).as_secs_f32();
            if st.time >= st.duration {
                st.time = 0.0;
            }
        }
        st.last = now;
        if !st.playing {
            return;
        }
    }
    render_frame(&win, state);
}

/// The cutscene camera at the current time.
fn cut_camera(st: &State) -> Option<Camera> {
    let clip = st.dict.clips.get(st.camera_clip?)?;
    let (eye, rot, fov) = cutscene::camera(&st.dict, clip, st.time)?;
    let m = math::from_trs([0.0; 3], rot, [1.0; 3]);
    let axis = |v: [f32; 3]| math::transform_dir(&m, v);
    Some(Camera {
        eye,
        forward: axis([0.0, 1.0, 0.0]),
        right: axis([1.0, 0.0, 0.0]),
        up: axis([0.0, 0.0, 1.0]),
        distance: 5.0,
        radius: 250.0,
        fov: fov.to_radians(),
    })
}

fn render_frame(win: &SceneWindow, state: &Rc<RefCell<Option<State>>>) {
    let st = state.borrow();
    let Some(st) = st.as_ref() else { return };
    let width = win.get_area_width().max(64.0) as u32;
    let height = win.get_area_height().max(64.0) as u32;
    let mut poses: Vec<(Mat4, Option<Vec<Mat4>>)> = Vec::with_capacity(st.actors.len());
    for a in &st.actors {
        let clip = &st.dict.clips[a.clip];
        let model = cutscene::mover(&st.dict, clip, st.time);
        let skin = clip.part_at(st.time).and_then(|(i, part, at)| {
            let anim = st.dict.animations.get(part.animation)?;
            let sk = a.preview.loaded.skeleton.as_ref()?;
            Some(a.bindings.get(i)?.as_ref()?.skin(sk, anim, anim.frame_at(at)))
        });
        poses.push((model, skin));
    }
    let items: Vec<crate::gpu::SceneItem> = st
        .actors
        .iter()
        .zip(&poses)
        .map(|(a, (model, skin))| crate::gpu::SceneItem {
            preview: &a.preview,
            look: &a.look,
            model: *model,
            skin: skin.as_deref(),
        })
        .collect();
    let camera = if st.cut_camera { cut_camera(st) } else { None }
        .unwrap_or_else(|| render::orbit_camera(st.bounds.0, st.bounds.1, st.view));
    let gpu = if std::env::var_os("OPENRAGE_SOFTWARE_RENDERER").is_none() {
        crate::gpu::render_scene(&items, &camera, width, height)
    } else {
        None
    };
    match gpu {
        Some(img) => win.set_frame(img),
        None => {
            // No GPU: the software renderer, at a smaller size to stay responsive.
            let (sw, sh) = ((width / 2).max(64) as usize, (height / 2).max(64) as usize);
            let soft: Vec<render::SceneItem> = st
                .actors
                .iter()
                .zip(&poses)
                .map(|(a, (model, skin))| render::SceneItem {
                    model: &a.preview.loaded,
                    look: &a.look,
                    materials: &a.preview.materials,
                    transform: *model,
                    skin: skin.as_deref(),
                })
                .collect();
            let rgba = render::render_scene(&soft, &camera, sw, sh);
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, sw as u32, sh as u32);
            win.set_frame(slint::Image::from_rgba8_premultiplied(buffer));
        }
    }
    win.set_time(st.time);
    win.set_time_text(format!("{:.1} / {:.1} s", st.time, st.duration).into());
}

fn wire(win: &SceneWindow, state: &Rc<RefCell<Option<State>>>) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_toggle_play(move || {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.playing = !st.playing;
            st.last = Instant::now();
            win.set_playing(st.playing);
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_seek(move |t| {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.time = t.clamp(0.0, st.duration);
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_toggle_camera(move || {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.cut_camera = !st.cut_camera && st.camera_clip.is_some();
            win.set_cut_camera(st.cut_camera);
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_rotate(move |dx, dy| {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.cut_camera = false;
            win.set_cut_camera(false);
            st.view.yaw -= dx * 0.01;
            st.view.pitch = (st.view.pitch + dy * 0.01).clamp(-1.4, 1.4);
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_zoom(move |delta| {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.cut_camera = false;
            win.set_cut_camera(false);
            st.view.zoom = (st.view.zoom * (1.0 + delta * 0.001)).clamp(0.1, 20.0);
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_resized(move || {
        if let Some(win) = w.upgrade() {
            render_frame(&win, &s);
        }
    });
}

/// Forgets a closed window (after the close event has been handled).
fn forget_closed() {
    slint::Timer::single_shot(Duration::ZERO, || {
        OPEN.with(|o| o.borrow_mut().retain(|(_, w, _)| w.window().is_visible()));
    });
}

/// Whether the newest scene has loaded (for debug snapshots).
#[cfg(debug_assertions)]
pub fn ready() -> bool {
    OPEN.with(|o| o.borrow().last().is_some_and(|(_, _, s)| s.borrow().is_some()))
}

/// Pauses the newest scene at `time`, through the cutscene or the free camera (debug snapshots).
#[cfg(debug_assertions)]
pub fn debug_pose(time: f32, free: bool) {
    let Some((win, state)) = OPEN.with(|o| o.borrow().last().map(|(_, w, s)| (w.clone_strong(), Rc::clone(s)))) else {
        return;
    };
    if let Some(st) = state.borrow_mut().as_mut() {
        st.playing = false;
        st.time = time.clamp(0.0, st.duration);
        st.cut_camera = !free && st.camera_clip.is_some();
        win.set_playing(false);
        win.set_cut_camera(st.cut_camera);
    }
    render_frame(&win, &state);
}

/// The most recently opened scene window (for debug snapshots).
#[cfg(debug_assertions)]
pub fn newest() -> Option<SceneWindow> {
    OPEN.with(|o| o.borrow().last().map(|(_, w, _)| w.clone_strong()))
}
