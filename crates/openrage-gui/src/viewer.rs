//! Separate viewer windows for models and images (opened by double-click).
//!
//! Each window keeps its own copy of what was selected and its own camera, so
//! several models can be compared side by side while browsing on.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openrage_app::render::{self, Camera, View};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::fly::Fly;
use crate::{AppWindow, ClipRow, MaterialRow, ModelPreview, PartRow, ViewerWindow};
use openrage_app::anim::Binding;
use rage_formats::clip::ClipDictionary;
use rage_formats::math::Mat4;

/// An animation playing in a viewer.
pub struct AnimState {
    dict: Arc<ClipDictionary>,
    clip: Option<usize>,
    /// Bone binding per part of the clip (one for most clips; one per section
    /// of a joined cutscene).
    bindings: Vec<Option<Binding>>,
    /// Seconds into the clip.
    time: f32,
    playing: bool,
    speed: f32,
    last: Instant,
}

impl AnimState {
    fn clip_duration(&self) -> f32 {
        self.clip.and_then(|c| self.dict.clips.get(c)).map_or(0.0, |c| c.duration())
    }

    /// Skinning matrices for the current time.
    pub fn skin(&self, m: &ModelPreview) -> Option<Vec<Mat4>> {
        let clip = self.dict.clips.get(self.clip?)?;
        let (index, part, at) = clip.part_at(self.time)?;
        let anim = self.dict.animations.get(part.animation)?;
        let skeleton = m.loaded.skeleton.as_ref()?;
        Some(self.bindings.get(index)?.as_ref()?.skin(skeleton, anim, anim.frame_at(at)))
    }
}

pub enum Content {
    Model(Arc<ModelPreview>),
    Image(slint::Image),
}

/// What a row of the parts list stands for.
#[derive(Clone)]
enum PartEntry {
    Part(usize),
    Group(Vec<usize>),
}

struct State {
    content: Content,
    anim: Option<AnimState>,
    /// Clip dictionaries found by the animation search.
    results: Vec<String>,
    anim_timer: slint::Timer,
    /// Rows of the parts list.
    entries: Vec<(String, i32, PartEntry)>,
    view: View,
    fly: Option<Fly>,
    timer: slint::Timer,
    /// Waits for an outfit loading in the background.
    outfit_timer: slint::Timer,
}

impl State {
    fn model(&self) -> Option<&Arc<ModelPreview>> {
        match &self.content {
            Content::Model(m) => Some(m),
            Content::Image(_) => None,
        }
    }

    /// Size of the shown parts (for camera speed and clipping).
    fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let m = self.model()?;
        let look = m.look.read().ok()?;
        render::bounds(&m.loaded, &look.parts)
    }

    fn camera(&self, lo: [f32; 3], hi: [f32; 3]) -> Camera {
        match &self.fly {
            Some(f) => {
                let radius = (0..3).map(|i| (hi[i] - lo[i]).powi(2)).sum::<f32>().sqrt() / 2.0;
                f.camera(radius)
            }
            None => render::orbit_camera(lo, hi, self.view),
        }
    }
}

thread_local! {
    static OPEN: RefCell<Vec<(u64, ViewerWindow)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Opens a viewer window for `content`, labelled with the main window's
/// current selection (title, path, details).
pub fn open(ui: &AppWindow, content: Content) {
    let labels = Labels {
        heading: ui.get_info_title().into(),
        path: ui.get_info_path().into(),
        icon: ui.get_info_icon().into(),
        tint: ui.get_info_tint(),
        rows: ui.get_info_rows(),
    };
    open_with(ui, content, labels, None);
}

/// What a viewer window shows about its model.
pub struct Labels {
    pub heading: String,
    pub path: String,
    pub icon: String,
    pub tint: slint::Color,
    pub rows: ModelRc<crate::InfoRow>,
}

type SharedState = Rc<RefCell<State>>;

/// Radians of rotation per pixel of dragging.
const ORBIT_RADIANS_PER_PIXEL: f32 = 0.01;
/// The camera never looks further up or down than this (radians).
const MAX_PITCH: f32 = 1.5;
/// Zoom factor per wheel notch.
const ZOOM_PER_NOTCH: f32 = 1.1;
/// Wheel delta of one notch.
const WHEEL_NOTCH: f32 = 120.0;
/// How often the free-fly camera moves while a key is held.
const FLY_FRAME: Duration = Duration::from_millis(16);
/// The playback speeds of the speed menu.
const ANIM_SPEEDS: [f32; 4] = [0.25, 0.5, 1.0, 2.0];
/// Appended to the status line of RDR2 clips.
const RDR2_MOTION_NOTE: &str = " · RDR2: some motion is approximated";

/// Opens a viewer; with `animation` (path of a `.ycd`) its first clip plays.
pub fn open_with(ui: &AppWindow, content: Content, labels: Labels, animation: Option<String>) {
    let Ok(win) = ViewerWindow::new() else { return };
    fill_window(ui, &win, labels, &content);
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let entries = match &content {
        Content::Model(m) => part_entries(&m.loaded),
        Content::Image(_) => Vec::new(),
    };
    if let Content::Model(m) = &content {
        refresh_lists(&win, m, &entries);
        if let Some(p) = &m.loaded.ped {
            let names: Vec<slint::SharedString> = (0..p.outfits).map(|k| slint::format!("Outfit {k}")).collect();
            win.set_outfits(ModelRc::new(VecModel::from(names)));
            win.set_outfit_index(p.outfit as i32);
        }
    }
    let has_skeleton = matches!(&content, Content::Model(m) if m.loaded.skeleton.is_some());
    win.set_has_skeleton(has_skeleton);
    let state = Rc::new(RefCell::new(State {
        content,
        anim: None,
        results: Vec::new(),
        anim_timer: slint::Timer::default(),
        entries,
        view: View::default(),
        fly: None,
        timer: slint::Timer::default(),
        outfit_timer: slint::Timer::default(),
    }));

    wire_orbit(&win, &state);
    wire_zoom(&win, &state);
    wire_fly(&win, &state);
    wire_parts(ui, &win, &state);
    wire_materials(ui, &win, &state);
    let (w, s) = (win.as_weak(), Rc::clone(&state));
    win.on_resized(move || render(&w, &s));
    wire_reset_view(&win, &state);
    wire_exports(ui, &win, &state);
    wire_animation(&win, &state);
    wire_window_close(&win, id);

    start_initial_animation(&win, &state, animation, has_skeleton);
    if win.show().is_ok() {
        render(&win.as_weak(), &state);
        OPEN.with(|o| o.borrow_mut().push((id, win)));
    }
}

/// The window's texts and, for an image, the image: what it shows before the model's lists.
fn fill_window(ui: &AppWindow, win: &ViewerWindow, labels: Labels, content: &Content) {
    win.set_theme_mode(ui.get_theme_mode());
    crate::accent::apply(win);
    win.set_mono_font(ui.get_mono_font());
    win.set_heading(labels.heading.into());
    win.set_path(labels.path.into());
    win.set_icon_name(labels.icon.into());
    win.set_tint(labels.tint);
    win.set_rows(labels.rows);
    win.set_model_mode(matches!(content, Content::Model(_)));
    if let Content::Image(img) = content {
        win.set_image(img.clone());
    }
}

fn wire_orbit(win: &ViewerWindow, state: &SharedState) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_orbit(move |dx, dy| {
        let mut st = s.borrow_mut();
        match &mut st.fly {
            // Dragging looks around.
            Some(f) => f.look(dx, dy),
            None => {
                st.view.yaw -= dx * ORBIT_RADIANS_PER_PIXEL;
                st.view.pitch = (st.view.pitch + dy * ORBIT_RADIANS_PER_PIXEL).clamp(-MAX_PITCH, MAX_PITCH);
            }
        }
        drop(st);
        render(&w, &s);
    });
}

fn wire_zoom(win: &ViewerWindow, state: &SharedState) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_zoom(move |delta| {
        let mut st = s.borrow_mut();
        if let Some(f) = &mut st.fly {
            f.wheel(delta);
            if let Some(win) = w.upgrade() {
                win.set_fly_speed(f.speed_text().into());
            }
        } else {
            st.view.zoom = (st.view.zoom * ZOOM_PER_NOTCH.powf(delta / WHEEL_NOTCH)).clamp(0.2, 20.0);
        }
        drop(st);
        render(&w, &s);
    });
}

/// The free-fly camera: its switch, the view along an axis, keys and focus.
fn wire_fly(win: &ViewerWindow, state: &SharedState) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_toggle_fly(move || toggle_fly(&w, &s));
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_look_along(move |axis| {
        {
            let mut st = s.borrow_mut();
            st.fly = None;
            st.view = crate::axis_view(axis, st.view);
        }
        if let Some(win) = w.upgrade() {
            win.set_fly_mode(false);
        }
        render(&w, &s);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_key_down(move |text, shift| {
        let key = text.to_lowercase();
        if key == "f" {
            toggle_fly(&w, &s);
            return;
        }
        if key == "r" {
            if let Some(win) = w.upgrade() {
                win.invoke_reset_view();
            }
            return;
        }
        let mut st = s.borrow_mut();
        if !st.fly.as_mut().is_some_and(|f| f.key_down(&key, shift)) {
            return;
        }
        if !st.timer.running() {
            let (w2, s2) = (w.clone(), Rc::clone(&s));
            st.timer.start(slint::TimerMode::Repeated, FLY_FRAME, move || tick(&w2, &s2));
        }
    });
    let s = Rc::clone(state);
    win.on_key_up(move |text, shift| {
        let mut st = s.borrow_mut();
        if !st.fly.as_mut().is_some_and(|f| f.key_up(&text.to_lowercase(), shift)) {
            st.timer.stop();
        }
    });
    let s = Rc::clone(state);
    win.on_focus_lost(move || {
        let mut st = s.borrow_mut();
        if let Some(f) = &mut st.fly {
            f.release();
        }
        st.timer.stop();
    });
}

/// Showing and hiding parts, and the outfit of a ped.
fn wire_parts(ui: &AppWindow, win: &ViewerWindow, state: &SharedState) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    let main = ui.as_weak();
    win.on_part_toggled(move |row, on| {
        let st = s.borrow();
        let parts = match st.entries.get(row as usize) {
            Some((_, _, PartEntry::Part(p))) => vec![*p],
            Some((_, _, PartEntry::Group(ps))) => ps.clone(),
            None => return,
        };
        change_look(&st, |look| parts.iter().for_each(|&p| look.parts[p] = on));
        drop(st);
        look_changed(&w, &s, &main);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    let main = ui.as_weak();
    win.on_all_parts(move |on| {
        change_look(&s.borrow(), |look| look.parts.iter_mut().for_each(|v| *v = on));
        look_changed(&w, &s, &main);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_outfit_selected(move |n| change_outfit(&w, &s, n));
}

fn wire_materials(ui: &AppWindow, win: &ViewerWindow, state: &SharedState) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    let main = ui.as_weak();
    win.on_material_visible(move |i, on| {
        change_look(&s.borrow(), |look| {
            if let Some(m) = look.materials.get_mut(i as usize) {
                m.visible = on;
            }
        });
        look_changed(&w, &s, &main);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    let main = ui.as_weak();
    win.on_material_color(move |i, c| {
        set_tint(&s.borrow(), i, Some([c.red(), c.green(), c.blue()].map(|v| v as f32 / 255.0)));
        look_changed(&w, &s, &main);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    let main = ui.as_weak();
    win.on_material_hex(move |i, text| {
        let Some(rgb) = parse_hex(&text) else { return };
        set_tint(&s.borrow(), i, Some(rgb));
        look_changed(&w, &s, &main);
    });
    let (w, s) = (win.as_weak(), Rc::clone(state));
    let main = ui.as_weak();
    win.on_material_reset(move |i| {
        set_tint(&s.borrow(), i, None);
        look_changed(&w, &s, &main);
    });
}

fn wire_reset_view(win: &ViewerWindow, state: &SharedState) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_reset_view(move || {
        let mut st = s.borrow_mut();
        st.view = View::default();
        st.fly = None;
        st.timer.stop();
        drop(st);
        if let Some(win) = w.upgrade() {
            win.set_fly_mode(false);
        }
        render(&w, &s);
    });
}

fn wire_exports(ui: &AppWindow, win: &ViewerWindow, state: &SharedState) {
    let s = Rc::clone(state);
    let main = ui.as_weak();
    win.on_export_gltf(move || {
        if let Content::Model(m) = &s.borrow().content {
            crate::save_glb(Arc::clone(m), main.clone());
        }
    });
    let s = Rc::clone(state);
    let (main, w) = (ui.as_weak(), win.as_weak());
    win.on_save_png(move || {
        if let (Content::Image(img), Some(win)) = (&s.borrow().content, w.upgrade()) {
            crate::save_png_image(img, win.get_heading().to_string(), main.clone());
        }
    });
}

/// The clip dictionary search, opening one, choosing a clip and playback.
fn wire_animation(win: &ViewerWindow, state: &SharedState) {
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_anim_search(move |text| anim_search(&w, &s, &text));
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_anim_open(move |i| anim_open(&w, &s, i));
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_anim_select(move |i| anim_select(&w, &s, i));
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_anim_toggle(move || anim_toggle(&w, &s));
    let (w, s) = (win.as_weak(), Rc::clone(state));
    win.on_anim_seek(move |v| {
        if let Some(a) = s.borrow_mut().anim.as_mut() {
            a.time = v.clamp(0.0, 1.0) * a.clip_duration();
        }
        update_anim_ui(&w, &s);
        render(&w, &s);
    });
    let s = Rc::clone(state);
    win.on_anim_speed(move |i| {
        if let Some(a) = s.borrow_mut().anim.as_mut() {
            a.speed = ANIM_SPEEDS.get(i as usize).copied().unwrap_or(1.0);
        }
    });
}

/// Opens the clip dictionary of search result `i` (`-1`: closes the open one).
fn anim_open(w: &slint::Weak<ViewerWindow>, s: &SharedState, i: i32) {
    let Some(win) = w.upgrade() else { return };
    if i < 0 {
        s.borrow_mut().anim = None;
        win.set_anim_dictionary("".into());
        win.set_anim_clip(-1);
        render(w, s);
        return;
    }
    let Some(vpath) = s.borrow().results.get(i as usize).cloned() else { return };
    win.set_anim_status(format!("Loading {vpath} …").into());
    match crate::load_clip_dictionary(&vpath) {
        Ok(dict) => show_clip_dictionary(&win, s, &vpath, dict),
        Err(e) => win.set_anim_status(format!("Cannot read {vpath}: {e:#}").into()),
    }
}

fn show_clip_dictionary(win: &ViewerWindow, s: &SharedState, vpath: &str, dict: ClipDictionary) {
    let rows: Vec<ClipRow> = dict
        .clips
        .iter()
        .map(|c| ClipRow { name: c.name.clone().into(), info: format!("{:.1} s", c.duration()).into() })
        .collect();
    let name = vpath.rsplit('/').next().unwrap_or(vpath).to_owned();
    win.set_anim_clips(ModelRc::new(VecModel::from(rows)));
    win.set_anim_dictionary(name.into());
    win.set_anim_clip(-1);
    // RDR2 dictionaries: one channel kind is not decoded yet and
    // is interpolated between values every 64 frames.
    let rdr2 = dict.animations.iter().any(|a| a.flags != 0);
    let note = if rdr2 { RDR2_MOTION_NOTE } else { "" };
    win.set_anim_status(format!("{} clips · {vpath}{note}", dict.clips.len()).into());
    s.borrow_mut().anim = Some(AnimState {
        dict: Arc::new(dict),
        clip: None,
        bindings: Vec::new(),
        time: 0.0,
        playing: false,
        speed: 1.0,
        last: Instant::now(),
    });
}

/// Binds clip `i` of the open dictionary to the model and starts playing it.
fn anim_select(w: &slint::Weak<ViewerWindow>, s: &SharedState, i: i32) {
    if !bind_clip(w, s, i) {
        return;
    }
    start_anim_timer(w, s);
}

/// Whether there is a clip `i` to play (then it is bound and the status shown).
fn bind_clip(w: &slint::Weak<ViewerWindow>, s: &SharedState, i: i32) -> bool {
    let mut st = s.borrow_mut();
    let skeleton = st.model().and_then(|m| m.loaded.skeleton.clone());
    let expressions = st.model().map(|m| m.loaded.expressions.clone()).unwrap_or_default();
    let Some(a) = st.anim.as_mut() else { return false };
    let Some(clip) = a.dict.clips.get(i as usize) else { return false };
    let bindings: Vec<Option<Binding>> = clip
        .parts
        .iter()
        .map(|p| {
            let anim = &a.dict.animations[p.animation];
            skeleton.as_ref().map(|sk| Binding::new(sk, anim).with_expressions(sk, anim, &expressions))
        })
        .collect();
    let moved = bindings.first().and_then(Option::as_ref).map_or(0, Binding::animated_bones);
    a.bindings = bindings;
    a.clip = Some(i as usize);
    a.time = 0.0;
    a.playing = true;
    a.last = Instant::now();
    let rdr2 = a.dict.animations.iter().any(|x| x.flags != 0);
    if let Some(win) = w.upgrade() {
        win.set_anim_clip(i);
        if moved == 0 {
            win.set_anim_status("This clip animates none of the model's bones (different skeleton).".into());
        } else {
            let note = if rdr2 { RDR2_MOTION_NOTE } else { "" };
            win.set_anim_status(format!("{moved} bones animated{note}").into());
        }
    }
    true
}

fn anim_toggle(w: &slint::Weak<ViewerWindow>, s: &SharedState) {
    let playing = {
        let mut st = s.borrow_mut();
        let Some(a) = st.anim.as_mut() else { return };
        if a.clip.is_none() {
            return;
        }
        if !a.playing && a.time >= a.clip_duration() {
            a.time = 0.0;
        }
        a.playing = !a.playing;
        a.last = Instant::now();
        a.playing
    };
    if playing {
        start_anim_timer(w, s);
    } else {
        s.borrow().anim_timer.stop();
        update_anim_ui(w, s);
    }
}

fn wire_window_close(win: &ViewerWindow, id: u64) {
    let w = win.as_weak();
    let wp = win.as_weak();
    win.on_copy_path(move || {
        if let Some(win) = wp.upgrade() {
            let path = win.get_path();
            if let Err(e) = crate::copy_to_clipboard(&path) {
                eprintln!("Cannot copy to the clipboard: {e}");
            }
        }
    });
    win.on_close_viewer(move || {
        if let Some(win) = w.upgrade() {
            let _ = win.hide();
        }
        forget(id);
    });
    win.window().on_close_requested(move || {
        forget(id);
        slint::CloseRequestResponse::HideWindow
    });
}

/// Starts the clip given to [`open_with`], or suggests the generic movement clips for peds.
fn start_initial_animation(win: &ViewerWindow, state: &SharedState, animation: Option<String>, has_skeleton: bool) {
    if let Some(ycd) = animation.filter(|_| has_skeleton) {
        let rows = vec![ClipRow { name: ycd.rsplit('/').next().unwrap_or(&ycd).into(), info: ycd.clone().into() }];
        win.set_anim_results(ModelRc::new(VecModel::from(rows)));
        state.borrow_mut().results = vec![ycd];
        win.invoke_anim_open(0);
        win.invoke_anim_select(best_clip_index(win, state) as i32);
        win.set_panel_tab(3);
    } else if has_skeleton {
        // Suggest the generic movement clips for peds.
        let female = win.get_heading().contains("_f_");
        anim_search(&win.as_weak(), state, if female { "move_f@generic" } else { "move_m@generic" });
    }
}

/// The clip that moves most of this model's bones (in a cutscene: its own).
fn best_clip_index(win: &ViewerWindow, state: &SharedState) -> usize {
    let st = state.borrow();
    let skeleton = st.model().and_then(|m| m.loaded.skeleton.as_ref());
    match (st.anim.as_ref(), skeleton) {
        (Some(a), Some(sk)) => {
            let tags: std::collections::HashSet<u16> = sk.bones.iter().map(|b| b.tag).collect();
            openrage_app::suggest::best_clip(&openrage_app::suggest::clip_tags(&a.dict), &tags, &win.get_path())
        }
        _ => 0,
    }
}

/// Loads another outfit of an RDR2 ped in the background and shows it in
/// place of the current one (camera and animation stay).
fn change_outfit(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>, outfit: i32) {
    let Some(ped) = state.borrow().model().and_then(|m| m.loaded.ped.clone()) else { return };
    let Some(game) = crate::game_dir() else { return };
    if let Some(win) = w.upgrade() {
        win.set_outfit_status(format!("Loading outfit {outfit} …").into());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(openrage_app::models::load_rdr2_ped(&game, &ped.name, outfit.max(0) as usize));
    });
    // Poll for the result on the UI thread (the state is not Send).
    let (w, s) = (w.clone(), Rc::clone(state));
    state.borrow().outfit_timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
        let Ok(result) = rx.try_recv() else { return };
        s.borrow().outfit_timer.stop();
        let Some(win) = w.upgrade() else { return };
        match result {
            Ok(loaded) => {
                let preview = Arc::new(crate::model_preview(loaded));
                let entries = part_entries(&preview.loaded);
                refresh_lists(&win, &preview, &entries);
                let clip = {
                    let mut st = s.borrow_mut();
                    st.entries = entries;
                    st.content = Content::Model(preview);
                    st.anim.as_ref().and_then(|a| a.clip)
                };
                win.set_outfit_status("".into());
                // The new parts need the clip bound to them again.
                if let Some(c) = clip {
                    win.invoke_anim_select(c as i32);
                }
                render(&w, &s);
            }
            Err(e) => win.set_outfit_status(format!("Cannot load outfit {outfit}: {e:#}").into()),
        }
    });
}

/// Drops a closed window (later, not from inside its own callback).
fn forget(id: u64) {
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        OPEN.with(|o| o.borrow_mut().retain(|(i, _)| *i != id));
    });
}

fn render(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>) {
    let Some(win) = w.upgrade() else { return };
    let st = state.borrow();
    if let Content::Model(m) = &st.content {
        let width = (win.get_area_width() - 32.0).max(64.0) as usize;
        let height = (win.get_area_height() - 32.0).max(64.0) as usize;
        let skin = st.anim.as_ref().and_then(|a| a.skin(m));
        if let Some((img, gizmo)) = crate::render_image(m, |lo, hi| st.camera(lo, hi), skin.as_deref(), width, height) {
            win.set_image(img);
            win.set_gizmo(gizmo);
        }
    }
}

fn anim_search(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>, text: &str) {
    let Some(win) = w.upgrade() else { return };
    // A full path (from "Copy path") opens directly: RDR2 has no search index.
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    let found = if text.contains('/') && (lower.ends_with(".ycd") || lower.ends_with(".#cd")) {
        vec![text.to_owned()]
    } else {
        let gta6 = crate::with_app(|a| a.game_dir.as_deref().is_some_and(openrage_app::detect::is_gta6_dir));
        crate::find_files(text, if gta6 { ".#cd" } else { ".ycd" }, 200)
    };
    let rows: Vec<ClipRow> = found
        .iter()
        .map(|v| {
            let (dir, name) = v.rsplit_once('/').unwrap_or(("", v));
            ClipRow { name: name.into(), info: dir.into() }
        })
        .collect();
    win.set_anim_status(if found.is_empty() {
        format!("No clip dictionaries match \"{text}\" (the search index may still be building).").into()
    } else {
        "".into()
    });
    win.set_anim_results(ModelRc::new(VecModel::from(rows)));
    win.set_anim_dictionary("".into());
    state.borrow_mut().results = found;
}

fn start_anim_timer(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>) {
    let (w2, s2) = (w.clone(), Rc::clone(state));
    state.borrow().anim_timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
        let done = {
            let mut st = s2.borrow_mut();
            let looping = w2.upgrade().is_some_and(|win| win.get_anim_loop());
            let Some(a) = st.anim.as_mut() else { return };
            let dt = a.last.elapsed().as_secs_f32().min(0.1);
            a.last = Instant::now();
            let total = a.clip_duration();
            a.time += dt * a.speed;
            if a.time >= total {
                if looping && total > 0.0 {
                    a.time %= total;
                } else {
                    a.time = total;
                    a.playing = false;
                }
            }
            !a.playing
        };
        update_anim_ui(&w2, &s2);
        render(&w2, &s2);
        if done {
            s2.borrow().anim_timer.stop();
        }
    });
    update_anim_ui(w, state);
}

fn update_anim_ui(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>) {
    let (Some(win), st) = (w.upgrade(), state.borrow()) else { return };
    let Some(a) = &st.anim else { return };
    let total = a.clip_duration().max(0.001);
    win.set_anim_position(a.time / total);
    win.set_anim_playing(a.playing);
    win.set_anim_time(format!("{:.2} / {:.2} s", a.time, total).into());
}

/// Switches between orbiting and flying, starting where the camera is.
fn toggle_fly(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>) {
    let mut st = state.borrow_mut();
    if st.model().is_none() {
        return;
    }
    if st.fly.take().is_none() {
        let Some((lo, hi)) = st.bounds() else { return };
        let cam = render::orbit_camera(lo, hi, st.view);
        st.fly = Some(Fly::new(&cam, 2.0 * cam.radius));
    }
    st.timer.stop();
    let flying = st.fly.is_some();
    let speed = st.fly.as_ref().map(Fly::speed_text).unwrap_or_default();
    drop(st);
    if let Some(win) = w.upgrade() {
        win.set_fly_mode(flying);
        win.set_fly_speed(speed.into());
    }
    render(w, state);
}

/// Moves the flying camera while movement keys are held.
fn tick(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>) {
    if let Some(f) = &mut state.borrow_mut().fly {
        f.tick();
    }
    render(w, state);
}

/// Rows of the parts list: groups (wheels, ped components, …) with their
/// parts below; a group of one part is a single row.
fn part_entries(model: &openrage_app::models::LoadedModel) -> Vec<(String, i32, PartEntry)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, p) in model.parts.iter().enumerate() {
        if p.group.is_empty() {
            groups.push((String::new(), vec![i]));
        } else if let Some(g) = groups.iter_mut().find(|(name, _)| *name == p.group) {
            g.1.push(i);
        } else {
            groups.push((p.group.clone(), vec![i]));
        }
    }
    let mut out = Vec::new();
    for (group, mut members) in groups {
        members.sort_by(|a, b| model.parts[*a].name.cmp(&model.parts[*b].name));
        match members.as_slice() {
            [single] => {
                let name = &model.parts[*single].name;
                let label =
                    if group.is_empty() || group == *name { name.clone() } else { format!("{group} · {name}") };
                out.push((label, 0, PartEntry::Part(*single)));
            }
            _ => {
                out.push((group, 0, PartEntry::Group(members.clone())));
                for m in members {
                    out.push((model.parts[m].name.clone(), 1, PartEntry::Part(m)));
                }
            }
        }
    }
    out
}

/// Fills the parts and materials lists from the model's current look.
fn refresh_lists(win: &ViewerWindow, model: &ModelPreview, entries: &[(String, i32, PartEntry)]) {
    let Ok(look) = model.look.read() else { return };
    let parts: Vec<PartRow> = entries
        .iter()
        .map(|(name, level, entry)| {
            let (visible, count) = match entry {
                PartEntry::Part(p) => (look.parts.get(*p).copied().unwrap_or(false), 0),
                PartEntry::Group(ps) => (ps.iter().any(|&p| look.parts[p]), ps.len() as i32),
            };
            PartRow { name: name.clone().into(), visible, level: *level, count }
        })
        .collect();
    let rgb = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    let materials: Vec<MaterialRow> = model
        .loaded
        .materials
        .iter()
        .zip(&look.materials)
        .map(|(slot, m)| MaterialRow {
            label: slot.label.clone().into(),
            visible: m.visible,
            color: slint::Color::from_rgb_u8(rgb(m.tint[0]), rgb(m.tint[1]), rgb(m.tint[2])),
            paint: slot.paint,
            textured: slot.textured,
        })
        .collect();
    update_model(win.get_parts(), parts, |m| win.set_parts(m));
    update_model(win.get_materials(), materials, |m| win.set_materials(m));
}

/// Updates rows in place (keeps the list's scroll position) or replaces the model.
pub(crate) fn update_model<T: Clone + PartialEq + 'static>(
    current: ModelRc<T>,
    rows: Vec<T>,
    set: impl FnOnce(ModelRc<T>),
) {
    if current.row_count() == rows.len() {
        for (i, row) in rows.into_iter().enumerate() {
            if current.row_data(i).as_ref() != Some(&row) {
                current.set_row_data(i, row);
            }
        }
    } else {
        set(ModelRc::new(VecModel::from(rows)));
    }
}

fn change_look(state: &State, f: impl FnOnce(&mut openrage_app::models::Look)) {
    if let Some(m) = state.model()
        && let Ok(mut look) = m.look.write()
    {
        f(&mut look);
    }
}

/// Sets a material's colour (`None`: back to its default).
fn set_tint(state: &State, material: i32, rgb: Option<[f32; 3]>) {
    let Some(m) = state.model() else { return };
    let default = m.loaded.default_look();
    change_look(state, |look| {
        let i = material as usize;
        if let (Some(slot), Some(def)) = (look.materials.get_mut(i), default.materials.get(i)) {
            slot.tint = match rgb {
                Some([r, g, b]) => [r, g, b, 1.0],
                None => def.tint,
            };
        }
    });
}

/// `#RRGGBB` or `RRGGBB` → colour.
fn parse_hex(text: &str) -> Option<[f32; 3]> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some([(v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF].map(|c| c as f32 / 255.0))
}

/// After the look changed: refresh the lists, this viewer and the main preview
/// (they share the look).
fn look_changed(w: &slint::Weak<ViewerWindow>, state: &Rc<RefCell<State>>, main: &slint::Weak<AppWindow>) {
    if let Some(win) = w.upgrade() {
        let st = state.borrow();
        if let Some(m) = st.model() {
            refresh_lists(&win, m, &st.entries);
        }
    }
    render(w, state);
    if let Some(ui) = main.upgrade() {
        crate::render_model(&ui);
    }
}

/// Applies the color scheme to every open viewer.
pub fn set_theme(mode: i32) {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| w.set_theme_mode(mode)));
}

/// The newest viewer window (for debug snapshots).
#[cfg(debug_assertions)]
pub fn newest() -> Option<ViewerWindow> {
    OPEN.with(|o| o.borrow().last().map(|(_, w)| w.clone_strong()))
}

/// Applies a changed accent color to every open window of this kind.
pub fn set_accent() {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w)| crate::accent::apply(w)));
}
