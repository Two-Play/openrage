//! A whole cutscene in one scene: every actor with its animation, through the
//! cutscene camera or a free orbit camera (GPU only).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// The map around the scene (models with their placements in the scene).
    surroundings: Vec<(Arc<ModelPreview>, Look, Vec<Mat4>)>,
    show_surroundings: bool,
    /// The surroundings are still loading.
    surroundings_pending: bool,
    /// The cutscene's lights, ready for the renderer; `show_lights` switches them.
    lights: Vec<crate::gpu::SceneLight>,
    show_lights: bool,
    timer: slint::Timer,
}

type Shared = Rc<RefCell<Option<State>>>;

/// How far the background loads are, shown on the scene while they run.
#[derive(Default)]
struct LoadProgress {
    /// What is loading; empty when nothing is.
    label: std::sync::Mutex<String>,
    done: AtomicUsize,
    /// 0: amount unknown.
    total: AtomicUsize,
}

impl LoadProgress {
    fn set(&self, label: &str, done: usize, total: usize) {
        if let Ok(mut l) = self.label.lock() {
            label.clone_into(&mut l);
        }
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    fn step(&self, done: usize, total: usize) {
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    /// Shows the progress on the window; false once nothing loads any more.
    fn show(&self, win: &SceneWindow) -> bool {
        let label = self.label.lock().map(|l| l.clone()).unwrap_or_default();
        let (done, total) = (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed));
        if total > 0 && !label.is_empty() {
            win.set_load_text(format!("{label} {done}/{total}").into());
            win.set_load_progress(done as f32 / total as f32);
        } else {
            win.set_load_text(label.clone().into());
            win.set_load_progress(-1.0);
        }
        !label.is_empty()
    }
}

thread_local! {
    static OPEN: RefCell<Vec<(u64, SceneWindow, Shared)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Opens the scene of the cutscene at `cut_path`; `models` holds a model path
/// per actor (found through the search index), `None` where there is none.
pub fn open(ui: &AppWindow, cut_path: String, cut: Cutscene, models: Vec<Option<String>>) {
    let Ok(win) = SceneWindow::new() else { return };
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(&win);
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
    let progress = watch_progress(&win);
    let weak = win.as_weak();
    std::thread::spawn(move || {
        let loaded = load(&game_dir, keys, &cut_path, &cut.actors, &models, &progress);
        let _ = weak.upgrade_in_event_loop(move |win| match loaded {
            Ok((dict, previews)) => {
                if let Some(state) =
                    OPEN.with(|o| o.borrow().iter().find(|(i, ..)| *i == id).map(|(_, _, s)| Rc::clone(s)))
                {
                    start(&win, &state, id, dict, &cut, previews, progress);
                }
            }
            Err(e) => {
                progress.set("", 0, 0);
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
    progress: &LoadProgress,
) -> anyhow::Result<Loaded> {
    progress.set("Loading the animation …", 0, 0);
    let (node, _) = rage_mods::resolve(game_dir, cut_path, keys.clone())?;
    let Node::ArchiveFile(rpf, id) = node else { anyhow::bail!("{cut_path} is not inside an archive") };
    let dict = cutscene::animation(&rpf, id)?;
    let game_dir: PathBuf = game_dir.to_owned();
    let total = models.iter().filter(|m| m.is_some()).count();
    let done = AtomicUsize::new(0);
    progress.set("Loading the actors' models", 0, total);
    // Models load in parallel; most cutscenes have a dozen or more.
    let previews = std::thread::scope(|s| {
        let handles: Vec<_> = models
            .iter()
            .zip(actors)
            .map(|(m, actor)| {
                let (game_dir, keys) = (&game_dir, keys.clone());
                let done = &done;
                s.spawn(move || {
                    let path = m.as_ref()?;
                    let loaded = openrage_app::models::load(game_dir, keys.clone(), path);
                    progress.step(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                    let mut loaded = loaded.ok()?;
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
    id: u64,
    dict: ClipDictionary,
    cut: &Cutscene,
    previews: Vec<Option<ModelPreview>>,
    progress: Arc<LoadProgress>,
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
    win.set_has_lights(!cut.lights.is_empty());
    win.set_lights_on(!cut.lights.is_empty());
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
        surroundings: Vec::new(),
        show_surroundings: true,
        surroundings_pending: true,
        lights: scene_lights(&cut.lights),
        show_lights: !cut.lights.is_empty(),
        timer: slint::Timer::default(),
    });
    load_surroundings(win, id, cut, (lo, hi), progress);
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

/// Loads the map objects around the scene in the background (the world
/// cache is built on first use, which takes a little while).
fn load_surroundings(
    win: &SceneWindow,
    id: u64,
    cut: &Cutscene,
    bounds: ([f32; 3], [f32; 3]),
    progress: Arc<LoadProgress>,
) {
    let Some((game_dir, keys)) = crate::with_app(|a| a.game_dir.clone().map(|d| (d, a.keys.clone()))) else {
        progress.set("", 0, 0);
        return;
    };
    // Scene → world: rotate about Z, then move to the scene's origin.
    let to_world = math::mul(
        &math::from_trs(cut.offset, [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
        &math::rotation_z(cut.rotation.to_radians()),
    );
    let to_scene = math::inverse_affine(&to_world);
    let (lo, hi) = bounds;
    let centre_local = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let centre = math::transform_point(&to_world, centre_local);
    let half = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt() / 2.0;
    let radius = (half + 25.0).clamp(30.0, 120.0);
    progress.set("Reading the map …", 0, 0);
    let weak = win.as_weak();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Vec<(ModelPreview, Vec<Mat4>)>> {
            let world = openrage_app::world::load(&game_dir, keys.clone())?;
            let files = openrage_app::world::files(&game_dir)?;
            progress.set("Finding the map objects around the scene …", 0, 0);
            let placed = openrage_app::world::objects_near(&world, &game_dir, keys.clone(), centre, radius);
            progress.set("Loading the surroundings", 0, 0);
            let models = openrage_app::world::load_models(&world, &files, &game_dir, keys, &placed, &|done, total| {
                progress.step(done, total)
            });
            Ok(models
                .into_iter()
                .map(|(name, loaded, transforms)| {
                    let materials = render::Materials::new(&loaded);
                    let look = std::sync::RwLock::new(loaded.default_look());
                    let placements = transforms.iter().map(|t| math::mul(&to_scene, t)).collect();
                    (ModelPreview { loaded, look, materials, name }, placements)
                })
                .collect())
        })();
        progress.set("", 0, 0);
        let _ = weak.upgrade_in_event_loop(move |win| {
            let Some(state) = OPEN.with(|o| o.borrow().iter().find(|(i, ..)| *i == id).map(|(_, _, s)| Rc::clone(s)))
            else {
                return;
            };
            if let Some(st) = state.borrow_mut().as_mut() {
                st.surroundings_pending = false;
            }
            let status = win.get_status().to_string();
            match result {
                Ok(models) => {
                    let objects: usize = models.iter().map(|(_, p)| p.len()).sum();
                    if let Some(st) = state.borrow_mut().as_mut() {
                        st.surroundings = models
                            .into_iter()
                            .map(|(m, p)| {
                                let mut look = m.loaded.default_look();
                                // Decals (dirt, stains, signs on floors) need blending the
                                // scene does not do; drawn opaque they cover the floor.
                                for (slot, l) in m.loaded.materials.iter().zip(look.materials.iter_mut()) {
                                    if rage_formats::shaders::name(slot.name_hash).is_some_and(|n| n.contains("decal"))
                                    {
                                        l.visible = false;
                                    }
                                }
                                (Arc::new(m), look, p)
                            })
                            .collect();
                    }
                    win.set_surroundings(1);
                    win.set_status(format!("{status} · {objects} map objects around it").into());
                }
                Err(e) => win.set_status(format!("{status} · no surroundings: {e:#}").into()),
            }
            render_frame(&win, &state);
        });
    });
}

/// Shows the loads' progress on the window until they are done.
fn watch_progress(win: &SceneWindow) -> Arc<LoadProgress> {
    let progress = Arc::new(LoadProgress::default());
    let timer = Rc::new(slint::Timer::default());
    let (weak, p, t) = (win.as_weak(), Arc::clone(&progress), Rc::clone(&timer));
    // The timer keeps itself alive through its own callback until it stops.
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
        let busy = weak.upgrade().is_some_and(|w| p.show(&w));
        if !busy {
            t.stop();
        }
    });
    progress
}

/// The cutscene's lights for the renderer. Their intensities use a unit of
/// the game's renderer; they are scaled relative to the brightest light so
/// the scene keeps the cutscene's key and fill lights.
fn scene_lights(lights: &[cutscene::Light]) -> Vec<crate::gpu::SceneLight> {
    let brightest = lights.iter().map(|l| l.intensity).fold(0.0f32, f32::max).max(1e-6);
    let mut out: Vec<crate::gpu::SceneLight> = lights
        .iter()
        .map(|l| {
            let strength = 1.6 * (l.intensity / brightest).sqrt();
            crate::gpu::SceneLight {
                position: l.position,
                color: [l.color[0] * strength, l.color[1] * strength, l.color[2] * strength],
                range: l.range,
                spot: l.spot.then_some((l.direction, l.cone)),
            }
        })
        .collect();
    // The strongest ones if there are more than the renderer takes.
    out.sort_by(|a, b| b.color[0].total_cmp(&a.color[0]));
    out.truncate(crate::gpu::MAX_LIGHTS);
    out
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
    let mut items: Vec<crate::gpu::SceneItem> = st
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
    if st.show_surroundings {
        for (preview, look, placements) in &st.surroundings {
            for t in placements {
                items.push(crate::gpu::SceneItem { preview, look, model: *t, skin: None });
            }
        }
    }
    let camera = if st.cut_camera { cut_camera(st) } else { None }
        .unwrap_or_else(|| render::orbit_camera(st.bounds.0, st.bounds.1, st.view));
    let gpu = if std::env::var_os("OPENRAGE_SOFTWARE_RENDERER").is_none() {
        let lights: &[crate::gpu::SceneLight] = if st.show_lights { &st.lights } else { &[] };
        crate::gpu::render_scene(&items, lights, &camera, width, height)
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
    win.on_toggle_lights({
        let (w, s) = (w.clone(), Rc::clone(&s));
        move || {
            let Some(win) = w.upgrade() else { return };
            if let Some(st) = s.borrow_mut().as_mut() {
                st.show_lights = !st.show_lights && !st.lights.is_empty();
                win.set_lights_on(st.show_lights);
            }
            render_frame(&win, &s);
        }
    });
    win.on_toggle_surroundings(move || {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.show_surroundings = !st.show_surroundings;
            win.set_surroundings(if st.show_surroundings { 1 } else { 2 });
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
    OPEN.with(|o| {
        o.borrow().last().is_some_and(|(_, _, s)| s.borrow().as_ref().is_some_and(|st| !st.surroundings_pending))
    })
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
