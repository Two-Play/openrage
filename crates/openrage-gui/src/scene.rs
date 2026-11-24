//! A whole cutscene in one scene: every actor with its animation, through the
//! cutscene camera or a free orbit camera (GPU only).

use rayon::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use openrage_app::anim::Binding;
use openrage_app::cutscene::{self, Actor, Cutscene};
use openrage_app::gta6_world::{self, Lod};
use openrage_app::models::Look;
use openrage_app::render::{self, Camera, View};
use openrage_app::{gta4_cutscene, gta4_effects};
use rage_crypto::GtaVKeys;
use rage_formats::clip::ClipDictionary;
use rage_formats::math::{self, Mat4};
use rage_rpf::vfs::Node;
use slint::ComponentHandle;

use crate::fly::Fly;
use crate::stream::{Drawn, Stream, scene_look};
use crate::{AppWindow, ModelPreview, SceneWindow};

struct SceneActor {
    preview: Arc<ModelPreview>,
    look: Look,
    /// Outfits it changes into (GTA IV): from when, dressed so.
    costumes: Vec<(f32, Arc<ModelPreview>, Look)>,
    /// Accessories on its bones (GTA IV hats, glasses): the model, the bone
    /// and the bone's rest pose (the accessory sits in the bone's space).
    attachments: Vec<(Arc<ModelPreview>, Look, usize, Mat4)>,
    clip: usize,
    /// Bone binding per part (section) of the clip; empty without a skeleton.
    bindings: Vec<Option<Binding>>,
}

impl SceneActor {
    /// The model as dressed at `time`.
    fn dressed(&self, time: f32) -> (&Arc<ModelPreview>, &Look) {
        self.costumes
            .iter()
            .rev()
            .find(|(from, ..)| *from <= time)
            .map_or((&self.preview, &self.look), |(_, p, l)| (p, l))
    }
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
    /// Per surroundings model: shown (the object list of a map scene).
    object_shown: Vec<bool>,
    /// The cutscene's lights, ready for the renderer; `show_lights` switches them.
    lights: Vec<crate::gpu::SceneLight>,
    /// Animated lights (GTA IV): their clips, and the brightest intensity
    /// any reaches (their strengths are relative to it).
    light_clips: Vec<usize>,
    brightest_light: f32,
    show_lights: bool,
    /// Subtitles: (start, end, text) in seconds.
    subtitles: Vec<(f32, f32, String)>,
    /// The cutscene's sound (GTA IV), from its start, and where it plays
    /// while the scene does and the sound is on.
    audio: Option<Arc<rage_formats::awc::Pcm>>,
    output: Option<crate::audio::Output>,
    sound_on: bool,
    /// Particle effects (GTA IV) placed in the scene, and their lists.
    effects: Vec<gta4_effects::Instance>,
    effect_library: Arc<gta4_effects::Library>,
    timer: slint::Timer,
    /// The flying camera, when it flies, and what moves it.
    fly: Option<Fly>,
    fly_timer: slint::Timer,
    /// Under a sky: a map, with how it is shown.
    outdoors: Option<Weather>,
    /// Tells it from other scenes.
    number: u64,
    /// Moves the water.
    water_timer: slint::Timer,
    /// Loads the details of GTA VI's whole map around the camera.
    stream: Option<Stream>,
}

impl State {
    /// The camera when no cutscene's is used.
    fn free_camera(&self) -> render::Camera {
        let (lo, hi) = self.bounds;
        match &self.fly {
            Some(f) => {
                let mut camera = f.camera((0..3).map(|k| (hi[k] - lo[k]).powi(2)).sum::<f32>().sqrt() / 2.0);
                // It can be close to things however large the scene is.
                camera.distance = camera.distance.min(50.0);
                if let Some(w) = &self.outdoors {
                    camera.fov = w.fov.to_radians();
                }
                camera
            }
            None => render::orbit_camera(lo, hi, self.view),
        }
    }

    /// Makes the sound follow the scene: playing from the scene's time while
    /// it plays and the sound is on, stopped otherwise.
    fn sync_sound(&mut self) {
        let wanted = self.playing && self.sound_on;
        match (&self.output, &self.audio) {
            (Some(out), _) if wanted && !out.finished() => {
                let at = Duration::from_secs_f32(self.time);
                if (out.position().as_secs_f32() - self.time).abs() > 0.15 {
                    out.seek(at);
                }
                out.resume();
            }
            (Some(out), _) if !wanted => out.pause(),
            (_, Some(pcm)) if wanted && self.time < pcm.duration_secs() => {
                self.output = crate::audio::Output::play(Arc::clone(pcm), Duration::from_secs_f32(self.time)).ok();
            }
            _ => {}
        }
    }
}

/// How often moving water is drawn.
const WATER_STEP: Duration = Duration::from_millis(50);

/// Another number for each scene.
fn scene_number() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The most pixels a scene is drawn with (Full HD).
const MOST_PIXELS: f32 = 1920.0 * 1080.0;

/// How a map is shown (the settings beside it).
#[derive(Clone, Copy)]
struct Weather {
    hour: f32,
    /// 0 to 1.
    haze: f32,
    brightness: f32,
    /// The flying camera's angle, in degrees.
    fov: f32,
    /// How far things are in detail, 1 as in the game.
    range: f32,
    /// The water moves.
    moving: bool,
    /// There is a sea at height 0.
    sea: bool,
    since: Instant,
}

impl Default for Weather {
    fn default() -> Self {
        Self {
            hour: 13.0,
            haze: 0.3,
            brightness: 1.0,
            fov: 60.0,
            range: 1.0,
            moving: true,
            sea: false,
            since: Instant::now(),
        }
    }
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
    let path = cut_path.clone();
    open_scene(
        ui,
        cut_path,
        Box::new(move |game_dir, keys, progress| {
            let (dict, previews) = load(game_dir, keys, &path, &cut.actors, &models, progress)?;
            let clips = cutscene::actor_clips(&cut.actors, &dict);
            Ok(Loaded {
                dict,
                previews,
                clips,
                cut,
                subtitles: Vec::new(),
                surroundings: Surroundings::World,
                problems: Vec::new(),
                audio: None,
                light_clips: Vec::new(),
                costumes: Vec::new(),
                props: Vec::new(),
                paints: Vec::new(),
                effects: Effects::default(),
            })
        }),
    );
}

/// Opens a GTA IV cutscene (`.cut`) as a scene: its sections one after
/// another, with the actors, the camera and the subtitles.
pub fn open_gta4(ui: &AppWindow, cut_path: String) {
    let path = cut_path.clone();
    open_scene(
        ui,
        cut_path,
        Box::new(move |game_dir, _keys, progress| {
            progress.set("Reading the cutscene and its animation …", 0, 0);
            let scene = gta4_cutscene::load(game_dir, &path)?;
            let actors = &scene.cutscene.actors;
            let total = scene.models.iter().filter(|m| m.is_some()).count();
            let done = AtomicUsize::new(0);
            progress.set("Loading the actors' models", 0, total);
            let previews = scene
                .models
                .par_iter()
                .zip(actors)
                .zip(&scene.outfits)
                .map(|((location, actor), outfit)| {
                    let loaded = openrage_app::progress::quietly(|| {
                        gta4_cutscene::load_model(game_dir, &actor.name, location.as_ref()?, outfit).ok()
                    });
                    if location.is_some() {
                        progress.step(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                    }
                    loaded.map(preview)
                })
                .collect();
            // Outfits changed into later: the model again, dressed so.
            let costumes = scene
                .models
                .par_iter()
                .zip(actors)
                .zip(&scene.outfit_changes)
                .map(|((location, actor), changes)| {
                    let Some(location) = location else { return Vec::new() };
                    changes
                        .iter()
                        .filter_map(|(from, outfit)| {
                            let loaded = openrage_app::progress::quietly(|| {
                                gta4_cutscene::load_model(game_dir, &actor.name, location, outfit).ok()
                            })?;
                            Some((*from, preview(loaded)))
                        })
                        .collect()
                })
                .collect();
            let props = actors
                .par_iter()
                .zip(&scene.props)
                .map(|(actor, chosen)| {
                    openrage_app::progress::quietly(|| gta4_cutscene::load_props(game_dir, &path, &actor.name, chosen))
                        .into_iter()
                        .map(|(m, bone)| (preview(m), bone))
                        .collect()
                })
                .collect();
            let clips = (0..actors.len()).map(Some).collect();
            Ok(Loaded {
                costumes,
                props,
                paints: scene.paints.clone(),
                dict: scene.animated.dict,
                previews,
                clips,
                cut: scene.cutscene,
                subtitles: scene.subtitles,
                surroundings: Surroundings::Gta4 {
                    episode: openrage_app::gta4_cutscene::episode(&path).to_owned(),
                    origins: scene.cut.sections.iter().map(|s| s.offset).collect(),
                    remove: scene.cut.sections.iter().flat_map(|s| s.remove.iter().cloned()).collect(),
                },
                problems: scene
                    .animated
                    .problems
                    .into_iter()
                    .chain(scene.missing_audio.iter().map(|n| format!("sound {n}")))
                    .collect(),
                audio: scene.audio,
                light_clips: scene.animated.lights,
                effects: Effects {
                    shifts: scene
                        .cut
                        .sections
                        .iter()
                        .map(|s| std::array::from_fn(|k| s.offset[k] - scene.cut.sections[0].offset[k]))
                        .collect(),
                    placed: scene.effects,
                    library: Arc::new(scene.effect_library),
                },
            })
        }),
    );
}

/// A cutscene's actors and animation, loaded in the background.
struct Loaded {
    dict: ClipDictionary,
    previews: Vec<Option<ModelPreview>>,
    /// Per actor: its clip in `dict`.
    clips: Vec<Option<usize>>,
    cut: Cutscene,
    /// (start, end, text) in seconds.
    subtitles: Vec<(f32, f32, String)>,
    /// Which map to load around the scene.
    surroundings: Surroundings,
    /// Animation pieces the game's files lack.
    problems: Vec<String>,
    /// The sound (GTA IV), stereo, from the cutscene's start.
    audio: Option<rage_formats::awc::Pcm>,
    /// Animated lights' clips (GTA IV).
    light_clips: Vec<usize>,
    /// Per actor: the outfits it changes into, from when (GTA IV).
    costumes: Vec<Vec<(f32, ModelPreview)>>,
    /// Per actor: its accessories and the bones they sit on (GTA IV).
    props: Vec<Vec<(ModelPreview, &'static str)>>,
    /// Per actor: its paint (GTA IV vehicles).
    paints: Vec<Option<[[f32; 3]; 3]>>,
    effects: Effects,
}

/// A GTA IV cutscene's particle effects as the cutscene names them.
#[derive(Default)]
struct Effects {
    placed: Vec<gta4_cutscene::PlacedEffect>,
    /// Per section: from the first section's origin to its own.
    shifts: Vec<[f32; 3]>,
    library: Arc<gta4_effects::Library>,
}

/// The map around a cutscene.
#[derive(Clone)]
enum Surroundings {
    /// GTA V: the world cache's objects near the scene.
    World,
    /// GTA IV: the objects the episode's placement files put near the
    /// sections' origins (in the world), without those the cutscene removes.
    Gta4 { episode: String, origins: Vec<[f32; 3]>, remove: Vec<([f32; 3], String)> },
}

type Loader = Box<dyn FnOnce(&std::path::Path, Option<Arc<GtaVKeys>>, &LoadProgress) -> anyhow::Result<Loaded> + Send>;

fn preview(loaded: openrage_app::models::LoadedModel) -> ModelPreview {
    let materials = render::Materials::new(&loaded);
    let name = loaded.drawables.first().map(|(n, _)| n.clone()).unwrap_or_default();
    let look = std::sync::RwLock::new(loaded.default_look());
    ModelPreview { loaded, look, materials, name }
}

fn open_scene(ui: &AppWindow, cut_path: String, loader: Loader) {
    let Ok(win) = SceneWindow::new() else { return };
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(&win);
    let name = cut_path.rsplit('/').next().unwrap_or(&cut_path).to_owned();
    win.set_heading(name.into());
    win.set_path(cut_path.clone().into());
    win.set_status("Loading the actors' models and the animation …".into());
    let state: Rc<RefCell<Option<State>>> = Rc::new(RefCell::new(None));
    wire(&win, &state);
    let closing = Rc::downgrade(&state);
    win.window().on_close_requested(move || {
        // The sound and the animation stop with the window.
        if let Some(st) = closing.upgrade().as_ref().and_then(|s| s.borrow_mut().take()) {
            st.timer.stop();
        }
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
        let loaded = loader(&game_dir, keys, &progress);
        let _ = weak.upgrade_in_event_loop(move |win| match loaded {
            Ok(loaded) => {
                if let Some(state) =
                    OPEN.with(|o| o.borrow().iter().find(|(i, ..)| *i == id).map(|(_, _, s)| Rc::clone(s)))
                {
                    start(&win, &state, id, loaded, progress);
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

fn load(
    game_dir: &std::path::Path,
    keys: Option<Arc<GtaVKeys>>,
    cut_path: &str,
    actors: &[Actor],
    models: &[Option<String>],
    progress: &LoadProgress,
) -> anyhow::Result<(ClipDictionary, Vec<Option<ModelPreview>>)> {
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
                    let loaded =
                        openrage_app::progress::quietly(|| openrage_app::models::load(game_dir, keys.clone(), path));
                    progress.step(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                    let mut loaded = loaded.ok()?;
                    // Props whose textures live in the dictionary their archetype names.
                    if let Some(type_file) = &actor.type_file
                        && let Some(extra) = cutscene::prop_textures(game_dir, keys, type_file, &actor.name)
                    {
                        loaded.add_textures(extra.0, extra.1);
                    }
                    Some(preview(loaded))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    Ok((dict, previews))
}

fn start(win: &SceneWindow, state: &Rc<RefCell<Option<State>>>, id: u64, loaded: Loaded, progress: Arc<LoadProgress>) {
    let Loaded {
        dict,
        previews,
        clips,
        cut,
        subtitles,
        surroundings,
        problems,
        audio,
        light_clips,
        costumes,
        props,
        paints,
        effects,
    } = loaded;
    let cut = &cut;
    let mut actors = Vec::new();
    let (mut missing_model, mut missing_clip) = (0, 0);
    let mut costumes = costumes.into_iter();
    let mut props = props.into_iter();
    let mut paints = paints.into_iter();
    // Per cut actor: its place in `actors` (effects attach to them).
    let mut actor_of: Vec<Option<usize>> = Vec::new();
    for ((actor, preview), clip) in cut.actors.iter().zip(previews).zip(clips) {
        actor_of.push(None);
        let later = costumes.next().unwrap_or_default();
        let worn = props.next().unwrap_or_default();
        let paint = paints.next().flatten();
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
                .map(|p| {
                    dict.animations
                        .get(p.animation)
                        .map(|a| Binding::new(sk, a).with_expressions(sk, a, &preview.loaded.expressions))
                })
                .collect(),
            None => Vec::new(),
        };
        let dress = |preview: &ModelPreview| {
            let mut look = preview.loaded.default_look();
            if actor.kind == cutscene::Kind::Ped {
                hide_oversized_parts(preview, &mut look);
            }
            look
        };
        let mut look = dress(&preview);
        if let Some(colours) = &paint {
            openrage_app::gta4_cutscene::paint(&preview.loaded, &mut look, colours);
        }
        let costumes = later
            .into_iter()
            .map(|(from, p)| {
                let look = dress(&p);
                (from, Arc::new(p), look)
            })
            .collect();
        let attachments = match &preview.loaded.skeleton {
            Some(sk) => {
                let rest = sk.world_transforms();
                worn.into_iter()
                    .filter_map(|(p, bone)| {
                        let b = sk.find_name(bone)?;
                        let look = p.loaded.default_look();
                        Some((Arc::new(p), look, b, rest[b]))
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        actors.push(SceneActor { preview: Arc::new(preview), look, costumes, attachments, clip, bindings });
        if let Some(last) = actor_of.last_mut() {
            *last = Some(actors.len() - 1);
        }
    }
    let duration = dict.clips.iter().map(|c| c.duration()).fold(cut.duration, f32::max);
    let camera_clip = cutscene::camera_clip(&dict);
    let dict = Arc::new(dict);
    let (instances, unknown_effects) = place_effects(&dict, &actors, &actor_of, &effects);
    // Bounds for the free camera: where the actors stand at the start.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for a in &actors {
        if !on_stage(&dict, &dict.clips[a.clip], 0.0) {
            continue;
        }
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
    if !instances.is_empty() || unknown_effects > 0 {
        status.push_str(&format!(" · {} effects", instances.len()));
        if unknown_effects > 0 {
            status.push_str(&format!(" ({unknown_effects} not in the effect lists)"));
        }
    }
    if !problems.is_empty() {
        status.push_str(&format!(" · missing in the game's files: {}", problems.join(", ")));
    }
    status.push_str(" · drag to turn the free camera, scroll to zoom, Space plays");
    win.set_status(status.into());
    win.set_loading(false);
    win.set_has_camera(camera_clip.is_some());
    let has_lights = !cut.lights.is_empty() || !light_clips.is_empty();
    // The brightest any animated light gets, sampled every half second.
    let brightest_light = light_clips
        .iter()
        .filter_map(|&c| dict.clips.get(c))
        .flat_map(|clip| {
            (0..=(clip.duration() * 2.0) as usize)
                .filter_map(|k| openrage_app::gta4_cutscene::light_at(&dict, clip, k as f32 / 2.0))
                .map(|l| l.intensity)
                .collect::<Vec<_>>()
        })
        .fold(0.0f32, f32::max);
    win.set_has_lights(has_lights);
    // Debug snapshots stay silent.
    let sound_on = !(cfg!(debug_assertions) && std::env::var_os("OPENRAGE_SNAPSHOT").is_some());
    win.set_has_sound(audio.is_some());
    win.set_sound_on(sound_on);
    win.set_lights_on(has_lights);
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
        object_shown: Vec::new(),
        lights: scene_lights(&cut.lights),
        show_lights: has_lights,
        light_clips,
        brightest_light,
        subtitles,
        audio: audio.map(Arc::new),
        output: None,
        sound_on,
        effects: instances,
        effect_library: effects.library,
        timer: slint::Timer::default(),
        fly: None,
        fly_timer: slint::Timer::default(),
        number: scene_number(),
        outdoors: None,
        water_timer: slint::Timer::default(),
        stream: None,
    });
    if let Some(st) = state.borrow_mut().as_mut() {
        st.sync_sound();
    }
    load_surroundings(win, id, cut, (lo, hi), surroundings, progress);
    win.set_playing(true);
    let (w, s) = (win.as_weak(), Rc::clone(state));
    if let Some(st) = state.borrow().as_ref() {
        st.timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || tick(&w, &s));
    }
    render_frame(win, state);
}

/// Where an actor (or one of its bones, by tag; 0 is the model itself) is at `time`.
fn actor_bone_at(dict: &ClipDictionary, a: &SceneActor, bone: u16, time: f32) -> Mat4 {
    let clip = &dict.clips[a.clip];
    let model = cutscene::mover(dict, clip, time);
    if bone == 0 {
        return model;
    }
    let posed = (|| {
        let sk = a.dressed(time).0.loaded.skeleton.as_ref()?;
        let b = sk.find_tag(bone)?;
        let (i, part, at) = clip.part_at(time)?;
        let anim = dict.animations.get(part.animation)?;
        let skin = a.bindings.get(i)?.as_ref()?.skin(sk, anim, anim.frame_at(at));
        Some(math::mul(&model, &math::mul(skin.get(b)?, &sk.world_transforms()[b])))
    })();
    posed.unwrap_or(model)
}

/// The cutscene's effects placed in the scene: each with where it is while
/// its emitters run (following the actor and bone it is attached to). Also
/// returns how many effects the effect lists lack.
fn place_effects(
    dict: &ClipDictionary,
    actors: &[SceneActor],
    actor_of: &[Option<usize>],
    effects: &Effects,
) -> (Vec<gta4_effects::Instance>, usize) {
    let lib = &effects.library;
    let instances = effects
        .placed
        .par_iter()
        .enumerate()
        .filter_map(|(n, e)| {
            let (list, effect) = lib.find(&e.effect.name)?;
            let fx = &lib.lists[list].effects[effect];
            let emitting = fx.events.iter().map(|ev| lib.lists[list].emitters[ev.emitter].duration).fold(0.0, f32::max);
            let local = math::mul(
                &math::from_trs(e.effect.position, [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
                &gta4_effects::euler(e.effect.rotation),
            );
            let actor = e.actor.and_then(|a| actor_of.get(a).copied().flatten()).and_then(|a| actors.get(a));
            let shift = effects.shifts.get(e.section).copied().unwrap_or([0.0; 3]);
            let samples = (((e.end - e.start) + emitting) / gta4_effects::TRACK_STEP).ceil() as usize + 1;
            let track = (0..samples.min(4000))
                .map(|k| {
                    let t = e.start + k as f32 * gta4_effects::TRACK_STEP;
                    let base = match actor {
                        Some(a) => actor_bone_at(dict, a, e.effect.bone, t),
                        None => math::from_trs(shift, [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
                    };
                    math::mul(&base, &local)
                })
                .collect();
            Some(gta4_effects::Instance {
                list,
                effect,
                start: e.start,
                end: e.end,
                track,
                seed: (n as u32).wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995,
            })
        })
        .collect::<Vec<_>>();
    let unknown = effects.placed.len() - instances.len();
    (instances, unknown)
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
    which: Surroundings,
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
    let cut_offset = cut.offset;
    progress.set("Reading the map …", 0, 0);
    let weak = win.as_weak();
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<Vec<(ModelPreview, Vec<Mat4>)>> {
            let models = match which {
                Surroundings::World => {
                    let world = openrage_app::world::load(&game_dir, keys.clone())?;
                    let files = openrage_app::world::files(&game_dir)?;
                    progress.set("Finding the map objects around the scene …", 0, 0);
                    let placed = openrage_app::world::objects_near(&world, &game_dir, keys.clone(), centre, radius);
                    progress.set("Loading the surroundings", 0, 0);
                    openrage_app::world::load_models(&world, &files, &game_dir, keys, &placed, &|done, total| {
                        progress.step(done, total)
                    })
                }
                Surroundings::Gta4 { episode, origins, remove } => {
                    // Around the actors in the first section, and around every other section's origin.
                    let shift: [f32; 3] = std::array::from_fn(|k| centre[k] - cut_offset[k]);
                    let centres: Vec<[f32; 3]> = std::iter::once(centre)
                        .chain(origins.iter().skip(1).map(|o| std::array::from_fn(|k| o[k] + shift[k])))
                        .collect();
                    progress.set("Loading the surroundings", 0, 0);
                    let scene = openrage_app::gta4::load_objects_near(
                        &game_dir,
                        &episode,
                        &centres,
                        radius,
                        &remove,
                        &|done, total| progress.step(done, total),
                    )?;
                    scene.models
                }
            };
            // Decoding the textures takes a while for hundreds of models: in parallel.
            Ok(models
                .into_par_iter()
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
    scene_lights_scaled(lights, lights.iter().map(|l| l.intensity).fold(0.0f32, f32::max))
}

/// Like [`scene_lights`], relative to a given brightest intensity.
fn scene_lights_scaled(lights: &[cutscene::Light], brightest: f32) -> Vec<crate::gpu::SceneLight> {
    let brightest = brightest.max(1e-6);
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
            // The sound keeps the time while it plays.
            if let Some(out) = st.output.as_ref().filter(|o| o.is_playing()) {
                st.time = out.position().as_secs_f32();
            }
            if st.time >= st.duration {
                st.time = 0.0;
                st.sync_sound();
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
    draw(win, state, false);
}

/// The same; `moving` if it is drawn again soon.
fn draw(win: &SceneWindow, state: &Rc<RefCell<Option<State>>>, moving: bool) {
    if let Some(st) = state.borrow_mut().as_mut() {
        let eye = st.free_camera().eye;
        if let Some(stream) = &mut st.stream {
            stream.look_from(eye, st.outdoors.map_or(1.0, |w| w.range));
        }
    }
    let st = state.borrow();
    let Some(st) = st.as_ref() else { return };
    let (width, height) = (win.get_area_width().max(64.0), win.get_area_height().max(64.0));
    // Pictures come back from the graphics card: not more of them than needed
    // for a smooth flight.
    let scale = (MOST_PIXELS / (width * height)).sqrt().min(1.0);
    let (width, height) = ((width * scale) as u32, (height * scale) as u32);
    let mut poses: Vec<(Mat4, Option<Vec<Mat4>>)> = Vec::with_capacity(st.actors.len());
    // Actors the current section does not have stay out of the picture.
    let shown: Vec<&SceneActor> =
        st.actors.iter().filter(|a| on_stage(&st.dict, &st.dict.clips[a.clip], st.time)).collect();
    for a in &shown {
        let clip = &st.dict.clips[a.clip];
        let model = cutscene::mover(&st.dict, clip, st.time);
        let skin = clip.part_at(st.time).and_then(|(i, part, at)| {
            let anim = st.dict.animations.get(part.animation)?;
            let sk = a.dressed(st.time).0.loaded.skeleton.as_ref()?;
            Some(a.bindings.get(i)?.as_ref()?.skin(sk, anim, anim.frame_at(at)))
        });
        poses.push((model, skin));
    }
    // Accessories where their bone is now.
    let worn: Vec<(&Arc<ModelPreview>, &Look, Mat4)> = shown
        .iter()
        .zip(&poses)
        .flat_map(|(a, (model, skin))| {
            a.attachments.iter().filter_map(move |(p, look, bone, rest)| {
                let at = skin.as_ref()?.get(*bone)?;
                Some((p, look, math::mul(model, &math::mul(at, rest))))
            })
        })
        .collect();
    let mut items: Vec<crate::gpu::SceneItem> = shown
        .iter()
        .zip(&poses)
        .map(|(a, (model, skin))| {
            let (preview, look) = a.dressed(st.time);
            crate::gpu::SceneItem { preview, look, model: *model, skin: skin.as_deref() }
        })
        .chain(worn.iter().map(|(preview, look, at)| crate::gpu::SceneItem { preview, look, model: *at, skin: None }))
        .collect();
    if st.show_surroundings {
        for (k, (preview, look, placements)) in st.surroundings.iter().enumerate() {
            if !st.object_shown.get(k).copied().unwrap_or(true) {
                continue;
            }
            for t in placements {
                items.push(crate::gpu::SceneItem { preview, look, model: *t, skin: None });
            }
        }
    }
    let camera = if st.cut_camera { cut_camera(st) } else { None }.unwrap_or_else(|| st.free_camera());
    let gpu = if std::env::var_os("OPENRAGE_SOFTWARE_RENDERER").is_none() {
        // Animated lights (GTA IV) where they are now.
        let animated: Vec<crate::gpu::SceneLight>;
        let lights: &[crate::gpu::SceneLight] = if !st.show_lights {
            &[]
        } else if st.light_clips.is_empty() {
            &st.lights
        } else {
            let now: Vec<cutscene::Light> = st
                .light_clips
                .iter()
                .filter_map(|&c| openrage_app::gta4_cutscene::light_at(&st.dict, st.dict.clips.get(c)?, st.time))
                .collect();
            animated = scene_lights_scaled(&now, st.brightest_light);
            &animated
        };
        let sprites = gta4_effects::sprites_at(&st.effect_library, &st.effects, st.time);
        // Seen from above, haze would hide a whole map.
        let outdoors = st.outdoors.map(|w| crate::gpu::Outdoors {
            haze: if st.fly.is_some() { w.haze * w.haze / 1000.0 } else { 0.0 },
            hour: w.hour,
            brightness: w.brightness,
            seconds: w.since.elapsed().as_secs_f32(),
            sea: w.sea,
        });
        let live = moving.then_some(st.number);
        let around = crate::gpu::Around { lights, sprites: &sprites, outdoors, live };
        crate::gpu::render_scene(&items, &around, &camera, width, height)
    } else {
        None
    };
    match gpu {
        Some(img) => win.set_frame(img),
        None => {
            // No GPU: the software renderer, at a smaller size to stay responsive.
            let (sw, sh) = ((width / 2).max(64) as usize, (height / 2).max(64) as usize);
            let mut soft: Vec<render::SceneItem> = shown
                .iter()
                .zip(&poses)
                .map(|(a, (model, skin))| {
                    let (preview, look) = a.dressed(st.time);
                    render::SceneItem {
                        model: &preview.loaded,
                        look,
                        materials: &preview.materials,
                        transform: *model,
                        skin: skin.as_deref(),
                    }
                })
                .chain(worn.iter().map(|(preview, look, at)| render::SceneItem {
                    model: &preview.loaded,
                    look,
                    materials: &preview.materials,
                    transform: *at,
                    skin: None,
                }))
                .collect();
            // A map scene is all surroundings.
            if st.actors.is_empty() && st.show_surroundings {
                for (k, (preview, look, placements)) in st.surroundings.iter().enumerate() {
                    if !st.object_shown.get(k).copied().unwrap_or(true) {
                        continue;
                    }
                    for t in placements {
                        soft.push(render::SceneItem {
                            model: &preview.loaded,
                            look,
                            materials: &preview.materials,
                            transform: *t,
                            skin: None,
                        });
                    }
                }
            }
            let rgba = render::render_scene(&soft, &camera, sw, sh);
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, sw as u32, sh as u32);
            win.set_frame(slint::Image::from_rgba8_premultiplied(buffer));
        }
    }
    win.set_time(st.time);
    win.set_time_text(format!("{:.1} / {:.1} s", st.time, st.duration).into());
    let subtitle = st.subtitles.iter().find(|(start, end, _)| (*start..*end).contains(&st.time)).map(|(_, _, t)| t);
    win.set_subtitle(subtitle.map(|t| t.as_str()).unwrap_or_default().into());
}

/// Whether an actor is in the scene at `time`: its part of the clip there
/// has an animation (GTA IV actors are left out of some sections).
fn on_stage(dict: &ClipDictionary, clip: &rage_formats::clip::Clip, time: f32) -> bool {
    clip.part_at(time).and_then(|(_, p, _)| dict.animations.get(p.animation)).is_none_or(|a| !a.tracks.is_empty())
}

/// Opens a map file (`.ymap`; GTA IV `.wpl` or a map archive) as a scene: every object it places, with its
/// model and textures, an object list and glTF export. GTA VI: a `.#map` with what is around it; with an empty
/// path, the whole map from afar.
pub fn open_map(ui: &AppWindow, vpath: String) {
    let Ok(win) = SceneWindow::new() else { return };
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(&win);
    let name = if vpath.is_empty() { "Map" } else { vpath.rsplit('/').next().unwrap_or(&vpath) }.to_owned();
    win.set_heading(name.clone().into());
    win.set_path(vpath.clone().into());
    win.set_timeline(false);
    win.set_status("Reading the map …".into());
    let state: Shared = Rc::new(RefCell::new(None));
    wire(&win, &state);
    wire_map(&win, &state, ui.as_weak(), name);
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
        progress.set("Reading the game's map files (once after the game changed, up to a minute) …", 0, 0);
        let result = (|| -> anyhow::Result<MapLoaded> {
            let scene = if openrage_app::gta4::is_gta4_dir(&game_dir) {
                // GTA IV: a placement file (.wpl) or a map archive (.img).
                openrage_app::gta4::load_map(&game_dir, &vpath, &|done, total| progress.step(done, total))?
            } else if openrage_app::detect::is_gta6_dir(&game_dir) {
                let show = |what: &str, done, total| progress.set(what, done, total);
                if vpath.is_empty() {
                    return load_whole_map(&game_dir, &show);
                } else {
                    gta6_world::place(&game_dir, &vpath, gta6_world::AROUND, &show)?
                }
            } else {
                let world = openrage_app::world::load(&game_dir, keys.clone())?;
                let files = openrage_app::world::files(&game_dir)?;
                progress.set("Loading the map's models", 0, 0);
                openrage_app::world::load_map(&world, &files, &game_dir, keys, &vpath, &|done, total| {
                    progress.step(done, total)
                })?
            };
            let models = scene
                .models
                .into_par_iter()
                .map(|(name, loaded, transforms)| {
                    let materials = render::Materials::new(&loaded);
                    let look = std::sync::RwLock::new(loaded.default_look());
                    (Arc::new(ModelPreview { loaded, look, materials, name }), transforms)
                })
                .collect();
            let missing = scene.missing.iter().map(|(_, n)| n).sum();
            Ok(MapLoaded { models, objects: scene.objects, missing, lod: None })
        })();
        progress.set("", 0, 0);
        let _ = weak.upgrade_in_event_loop(move |win| {
            let Some(state) = OPEN.with(|o| o.borrow().iter().find(|(i, ..)| *i == id).map(|(_, _, s)| Rc::clone(s)))
            else {
                return;
            };
            win.set_loading(false);
            match result {
                Ok(loaded) => start_map(&win, &state, loaded),
                Err(e) => win.set_status(format!("Cannot show the map: {e:#}").into()),
            }
        });
    });
}

/// A map file's models with their placements, loaded in the background.
struct MapLoaded {
    models: Vec<(Arc<ModelPreview>, Vec<Mat4>)>,
    objects: usize,
    /// Objects whose model was not found.
    missing: usize,
    /// The whole map of GTA VI, whose details are loaded around the camera,
    /// and the hashes of the models' names.
    lod: Option<(Arc<Lod>, Vec<u32>)>,
}

/// GTA VI's whole map from afar.
fn load_whole_map(game: &Path, show: &gta6_world::Progress) -> anyhow::Result<MapLoaded> {
    let lod = Arc::new(Lod::open(game, show)?);
    let loaded =
        lod.load(&lod.far(), gta6_world::Quality::Far, &|done, total| show("Loading the map's models", done, total));
    let mut places = lod.cut(&[0.0, 0.0, 1e6], 1.0, &|_| true, &|_| false, std::iter::empty()).draw;
    let objects = places.values().map(Vec::len).sum();
    let (keys, models): (Vec<u32>, Vec<Arc<ModelPreview>>) = crate::stream::ready(loaded).into_iter().unzip();
    anyhow::ensure!(!models.is_empty(), "the map files of this game have no distant view");
    let models: Vec<(Arc<ModelPreview>, Vec<Mat4>)> =
        models.into_iter().zip(&keys).map(|(p, key)| (p, places.remove(key).unwrap_or_default())).collect();
    let missing = places.values().map(Vec::len).sum();
    Ok(MapLoaded { models, objects, missing, lod: Some((lod, keys)) })
}

fn start_map(win: &SceneWindow, state: &Shared, scene: MapLoaded) {
    let whole_map = scene.lod.is_some();
    win.set_view_sea(whole_map);
    let mut surroundings: Vec<(Arc<ModelPreview>, Look, Vec<Mat4>)> = scene
        .models
        .into_iter()
        .map(|(p, transforms)| {
            let look = scene_look(&p);
            (p, look, transforms)
        })
        .collect();
    let far: Option<HashMap<u32, Arc<ModelPreview>>> = scene
        .lod
        .as_ref()
        .map(|(_, keys)| keys.iter().copied().zip(surroundings.iter().map(|(p, ..)| Arc::clone(p))).collect());
    surroundings.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    let items: Vec<(&openrage_app::models::LoadedModel, &Look, &[Mat4])> =
        surroundings.iter().map(|(p, l, t)| (&p.loaded, l, t.as_slice())).collect();
    let bounds = openrage_app::world::scene_bounds(&items).unwrap_or(([-1.0; 3], [1.0; 3]));
    let rows: Vec<crate::PartRow> = surroundings
        .iter()
        .map(|(p, _, t)| crate::PartRow { name: p.name.clone().into(), visible: true, level: 0, count: t.len() as i32 })
        .collect();
    // What is there changes with the camera in a whole map.
    win.set_objects(slint::ModelRc::new(slint::VecModel::from(if far.is_some() { Vec::new() } else { rows })));
    win.set_can_export(!surroundings.is_empty());
    win.set_can_fly(true);
    win.set_show_panel(true);
    let (lo, hi) = bounds;
    let mut status = format!(
        "{} objects · {} models · {:.0} × {:.0} m",
        scene.objects,
        surroundings.len(),
        hi[0] - lo[0],
        hi[1] - lo[1]
    );
    if scene.missing > 0 {
        status.push_str(&format!(" · {} without a model", scene.missing));
    }
    win.set_status(status.into());
    let object_shown = vec![true; surroundings.len()];
    *state.borrow_mut() = Some(State {
        dict: Arc::new(ClipDictionary::default()),
        actors: Vec::new(),
        camera_clip: None,
        duration: 0.0,
        time: 0.0,
        playing: false,
        last: Instant::now(),
        cut_camera: false,
        view: View::default(),
        bounds,
        surroundings,
        show_surroundings: true,
        surroundings_pending: false,
        object_shown,
        lights: Vec::new(),
        show_lights: false,
        light_clips: Vec::new(),
        brightest_light: 0.0,
        subtitles: Vec::new(),
        audio: None,
        output: None,
        sound_on: false,
        effects: Vec::new(),
        effect_library: Arc::default(),
        timer: slint::Timer::default(),
        fly: None,
        fly_timer: slint::Timer::default(),
        number: scene_number(),
        outdoors: Some(Weather { sea: whole_map, ..Weather::default() }),
        water_timer: slint::Timer::default(),
        stream: None,
    });
    if let (Some((lod, _)), Some(far)) = (scene.lod, far) {
        let weak = win.as_weak();
        let stream = Stream::start(lod, far, move |drawn, more| {
            let weak = weak.clone();
            weak.upgrade_in_event_loop(move |win| show_stream(&win, drawn, more)).is_ok()
        });
        if let Some(st) = state.borrow_mut().as_mut() {
            st.stream = Some(stream);
        }
    }
    move_water(win, state);
    render_frame(win, state);
}

/// The whole map in the detail loaded by now.
fn show_stream(win: &SceneWindow, drawn: Drawn, more: bool) {
    let state = OPEN.with(|o| {
        let open = o.borrow();
        let window = |w: &SceneWindow| std::ptr::eq(w.window(), win.window());
        open.iter().find(|(_, w, _)| window(w)).map(|(.., s)| Rc::clone(s))
    });
    let Some(state) = state else { return };
    if let Some(st) = state.borrow_mut().as_mut() {
        let objects: usize = drawn.iter().map(|(.., places)| places.len()).sum();
        let loading = if more { " · loading details …" } else { "" };
        win.set_status(format!("{objects} objects · {} models{loading}", drawn.len()).into());
        st.object_shown = vec![true; drawn.len()];
        st.surroundings = drawn;
    }
    render_frame(win, &state);
}

/// The object list and the export of a map scene.
fn wire_map(win: &SceneWindow, state: &Shared, main: slint::Weak<AppWindow>, name: String) {
    let set_shown = |s: &Shared, w: &slint::Weak<SceneWindow>, which: Option<usize>, on: bool| {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            for (k, v) in st.object_shown.iter_mut().enumerate() {
                if which.is_none_or(|i| i == k) {
                    *v = on;
                }
            }
            let rows: Vec<crate::PartRow> = st
                .surroundings
                .iter()
                .zip(&st.object_shown)
                .map(|((p, _, t), v)| crate::PartRow {
                    name: p.name.clone().into(),
                    visible: *v,
                    level: 0,
                    count: t.len() as i32,
                })
                .collect();
            crate::viewer::update_model(win.get_objects(), rows, |m| win.set_objects(m));
        }
        render_frame(&win, s);
    };
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_object_toggled(move |i, on| set_shown(&s, &w, usize::try_from(i).ok(), on));
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_all_objects(move |on| set_shown(&s, &w, None, on));
    let s = Rc::clone(state);
    win.on_export_gltf(move || {
        let st = s.borrow();
        let Some(st) = st.as_ref() else { return };
        // The shown objects, as they are (the export runs in the background).
        let shown: Vec<(Arc<ModelPreview>, Look, Vec<Mat4>)> = st
            .surroundings
            .iter()
            .zip(&st.object_shown)
            .filter(|(_, v)| **v)
            .map(|((p, l, t), _)| (Arc::clone(p), l.clone(), t.clone()))
            .collect();
        let (lo, hi) = st.bounds;
        let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]];
        crate::save_scene_glb(name.clone(), shown, center, main.clone());
    });
}

fn wire(win: &SceneWindow, state: &Rc<RefCell<Option<State>>>) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_view_changed(move || {
        let Some(win) = w.upgrade() else { return };
        if let Some(weather) = s.borrow_mut().as_mut().and_then(|st| st.outdoors.as_mut()) {
            weather.hour = win.get_view_hour();
            weather.haze = win.get_view_haze();
            weather.brightness = win.get_view_brightness();
            weather.fov = win.get_view_fov();
            weather.range = win.get_view_range();
            weather.moving = win.get_view_water();
            weather.sea = win.get_view_sea();
        }
        move_water(&win, &s);
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_toggle_fly(move || toggle_fly(&w, &s));
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_key_down(move |text, shift| {
        let key = text.to_lowercase();
        if key == "f" {
            toggle_fly(&w, &s);
            return;
        }
        let mut st = s.borrow_mut();
        let Some(st) = st.as_mut() else { return };
        if st.fly.as_mut().is_some_and(|f| f.key_down(&key, shift)) && !st.fly_timer.running() {
            let (w, s) = (w.clone(), Rc::clone(&s));
            st.fly_timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
                let Some(win) = w.upgrade() else { return };
                if let Some(f) = s.borrow_mut().as_mut().and_then(|st| st.fly.as_mut()) {
                    f.tick();
                }
                draw(&win, &s, true);
            });
        }
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_key_up(move |text, shift| {
        let mut stopped = false;
        if let Some(st) = s.borrow_mut().as_mut()
            && !st.fly.as_mut().is_some_and(|f| f.key_up(&text.to_lowercase(), shift))
        {
            stopped = st.fly_timer.running();
            st.fly_timer.stop();
        }
        // Where it stopped.
        if let Some(win) = w.upgrade().filter(|_| stopped) {
            render_frame(&win, &s);
        }
    });
    let s = Rc::clone(state);
    win.on_focus_lost(move || {
        if let Some(st) = s.borrow_mut().as_mut() {
            if let Some(f) = &mut st.fly {
                f.release();
            }
            st.fly_timer.stop();
        }
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_toggle_play(move || {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.playing = !st.playing;
            st.last = Instant::now();
            win.set_playing(st.playing);
            st.sync_sound();
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_seek(move |t| {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.time = t.clamp(0.0, st.duration);
            st.sync_sound();
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_toggle_sound(move || {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.sound_on = !st.sound_on && st.audio.is_some();
            win.set_sound_on(st.sound_on);
            st.sync_sound();
        }
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
            match &mut st.fly {
                Some(f) => f.look(dx, dy),
                None => {
                    st.view.yaw -= dx * 0.01;
                    st.view.pitch = (st.view.pitch + dy * 0.01).clamp(-1.5, 1.5);
                }
            }
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_zoom(move |delta| {
        let Some(win) = w.upgrade() else { return };
        if let Some(st) = s.borrow_mut().as_mut() {
            st.cut_camera = false;
            win.set_cut_camera(false);
            match &mut st.fly {
                Some(f) => {
                    f.wheel(delta);
                    win.set_fly_speed(f.speed_text().into());
                }
                // Far enough in to see a street of a whole map.
                None => st.view.zoom = (st.view.zoom * 1.1f32.powf(delta / 120.0)).clamp(0.1, 200.0),
            }
        }
        render_frame(&win, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_toggle_lights({
        let (w, s) = (w.clone(), Rc::clone(&s));
        move || {
            let Some(win) = w.upgrade() else { return };
            if let Some(st) = s.borrow_mut().as_mut() {
                st.show_lights = !st.show_lights && (!st.lights.is_empty() || !st.light_clips.is_empty());
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

/// Keeps drawing while water moves.
fn move_water(win: &SceneWindow, state: &Shared) {
    let st = state.borrow();
    let Some(st) = st.as_ref() else { return };
    if !st.outdoors.is_some_and(|w| w.moving) {
        st.water_timer.stop();
    } else if !st.water_timer.running() {
        let (w, s) = (win.as_weak(), Rc::downgrade(state));
        st.water_timer.start(slint::TimerMode::Repeated, WATER_STEP, move || {
            if let (Some(win), Some(s)) = (w.upgrade(), s.upgrade())
                && win.window().is_visible()
                // Flying draws anyway.
                && !s.borrow().as_ref().is_some_and(|st| st.fly_timer.running())
            {
                draw(&win, &s, true);
            }
        });
    }
}

/// Switches between orbiting and flying, starting where the camera is.
fn toggle_fly(w: &slint::Weak<SceneWindow>, state: &Shared) {
    let Some(win) = w.upgrade() else { return };
    if let Some(st) = state.borrow_mut().as_mut() {
        st.fly_timer.stop();
        st.fly = match st.fly.take() {
            Some(_) => None,
            None => {
                let from = if st.cut_camera { cut_camera(st) } else { None }.unwrap_or_else(|| st.free_camera());
                let mut fly = Fly::new(&from, 0.0);
                // A large scene is not crossed in seconds, a small one not in minutes.
                fly.set_speed((from.distance * 0.1).clamp(3.0, 400.0));
                Some(fly)
            }
        };
        st.cut_camera = false;
        win.set_cut_camera(false);
        win.set_fly_mode(st.fly.is_some());
        win.set_fly_speed(st.fly.as_ref().map(Fly::speed_text).unwrap_or_default().into());
    }
    render_frame(&win, state);
}

/// Forgets a closed window (after the close event has been handled).
fn forget_closed() {
    slint::Timer::single_shot(Duration::ZERO, || {
        OPEN.with(|o| o.borrow_mut().retain(|(_, w, _)| w.window().is_visible()));
    });
}

/// Flies the newest scene's camera to a place (for debug snapshots): x, y, z, yaw, pitch.
#[cfg(debug_assertions)]
pub fn fly_to(place: [f32; 5]) {
    let Some((win, state)) = OPEN.with(|o| o.borrow().last().map(|(_, w, s)| (w.clone_strong(), Rc::clone(s)))) else {
        return;
    };
    if let Some(st) = state.borrow_mut().as_mut() {
        let [x, y, z, yaw, pitch] = place;
        st.fly = Some(Fly::new(&render::fly_camera([x, y, z], yaw, pitch, 1.0), 100.0));
        win.set_fly_mode(true);
        // `OPENRAGE_HOUR=<hour>`: the time of day.
        if let (Some(w), Some(hour)) =
            (&mut st.outdoors, std::env::var("OPENRAGE_HOUR").ok().and_then(|h| h.parse().ok()))
        {
            w.hour = hour;
            win.set_view_hour(hour);
        }
    }
    render_frame(&win, &state);
}

/// Draws the newest scene so many times while turning, and prints how long a picture takes (milliseconds).
#[cfg(debug_assertions)]
pub fn bench(pictures: u32) {
    let Some((win, state)) = OPEN.with(|o| o.borrow().last().map(|(_, w, s)| (w.clone_strong(), Rc::clone(s)))) else {
        return;
    };
    let started = Instant::now();
    for _ in 0..pictures {
        if let Some(st) = state.borrow_mut().as_mut() {
            match &mut st.fly {
                Some(f) => f.look(5.0, 0.0),
                None => st.view.yaw += 0.02,
            }
        }
        draw(&win, &state, true);
    }
    eprintln!("bench: {:.2} ms a picture", started.elapsed().as_secs_f32() * 1000.0 / pictures as f32);
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
