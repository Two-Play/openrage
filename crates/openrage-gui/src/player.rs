//! The video player: a window for each video, with its own sound.

use std::cell::RefCell;
use std::rc::Rc;

use crate::video_ui::{Event, Video, open_video};
use crate::*;
use rage_formats::awc::Pcm;
use sound_ui::SOUND_END_MARGIN;

type Shared = Rc<RefCell<State>>;

thread_local! {
    static OPEN: RefCell<Vec<(u64, PlayerWindow, Shared)>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
    /// As last set in a window.
    static VOLUME: Cell<f32> = const { Cell::new(1.0) };
}

#[derive(Default)]
struct State {
    video: Option<Video>,
    pcm: Option<Arc<Pcm>>,
    has_sound: bool,
    output: Option<audio::Output>,
    /// Where playback starts, set by seeking while stopped.
    start: f32,
    timer: slint::Timer,
}

impl State {
    fn total(&self) -> f32 {
        self.pcm.as_ref().map_or(0.0, |p| p.duration_secs()).max(0.001)
    }

    /// The output, unless it played to its end.
    fn running(&self) -> Option<&audio::Output> {
        self.output.as_ref().filter(|o| !o.finished())
    }

    fn position(&self) -> f32 {
        self.running().map_or(self.start, |o| o.position().as_secs_f32()).min(self.total())
    }

    fn seek(&mut self, secs: f32) {
        let secs = secs.clamp(0.0, self.total());
        match self.running() {
            Some(o) => o.seek(Duration::from_secs_f32(secs)),
            None => self.start = secs,
        }
    }

    fn play(&mut self) {
        let Some(pcm) = self.pcm.clone() else { return };
        let start = if self.start >= self.total() - SOUND_END_MARGIN { 0.0 } else { self.start };
        let start = Duration::from_secs_f32(start);
        // Without sound the pictures still play.
        let sound = self.has_sound.then(|| audio::Output::play(Arc::clone(&pcm), start).ok()).flatten();
        let output = sound.unwrap_or_else(|| audio::Output::silent(pcm, start));
        output.set_volume(VOLUME.with(Cell::get));
        self.output = Some(output);
    }
}

/// Opens the selected video in a window of its own, where it plays.
pub(crate) fn open_selected(ui: &AppWindow) {
    let target = with_app(|a| {
        let listing = a.listing.as_ref()?;
        let item = listing.items.get(a.selected?)?;
        let vpath = item.vpath.clone().unwrap_or_else(|| browser::join(&listing.vpath, &item.name));
        Some((item.name.rsplit('/').next().unwrap_or_default().to_owned(), vpath))
    });
    let Some((name, vpath)) = target else { return };
    stop_sound(ui);
    let Ok(win) = PlayerWindow::new() else { return };
    win.set_theme_mode(ui.get_theme_mode());
    accent::apply(&win);
    win.set_mono_font(ui.get_mono_font());
    win.set_heading(name.into());
    win.set_volume(VOLUME.with(Cell::get));
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    let state = Rc::new(RefCell::new(State::default()));
    wire(&win, &state, id);
    OPEN.with(|o| o.borrow_mut().push((id, win.clone_strong(), state)));
    if let Err(e) = win.show() {
        show_error(ui, format!("Cannot open the player: {e}"));
        forget(id);
        return;
    }
    let (game_dir, keys) = with_app(|a| (a.game_dir.clone(), a.keys.clone()));
    let weak = win.as_weak();
    open_video(vpath, game_dir, keys, move |event| {
        weak.upgrade_in_event_loop(move |win| {
            let state = OPEN.with(|o| o.borrow().iter().find(|(i, ..)| *i == id).map(|(_, _, s)| Rc::clone(s)));
            // A closed window drops the video, which ends its thread.
            if let Some(state) = state {
                handle(&win, &state, event);
            }
        })
        .is_ok()
    });
}

fn handle(win: &PlayerWindow, state: &Shared, event: Event) {
    match event {
        Event::Opened(_, picture, video) => {
            win.set_picture(slint::Image::from_rgba8(picture));
            win.set_has_picture(true);
            state.borrow_mut().video = Some(video);
        }
        Event::Sound(pcm, _) => {
            {
                let mut s = state.borrow_mut();
                s.has_sound = pcm.samples.iter().any(|&v| v != 0);
                s.pcm = Some(Arc::new(pcm));
            }
            win.set_ready(true);
            toggle(win, state);
        }
        Event::Picture(picture) => win.set_picture(slint::Image::from_rgba8(picture)),
        Event::Failed(e) => win.set_error(e.into()),
    }
}

fn toggle(win: &PlayerWindow, state: &Shared) {
    {
        let mut s = state.borrow_mut();
        let end = s.total() - SOUND_END_MARGIN;
        match s.running() {
            Some(o) if o.is_playing() => o.pause(),
            Some(o) => {
                if o.position().as_secs_f32() >= end {
                    o.seek(Duration::ZERO);
                }
                o.resume();
            }
            None => s.play(),
        }
    }
    let (weak, shared) = (win.as_weak(), Rc::downgrade(state));
    state.borrow().timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
        if let (Some(win), Some(state)) = (weak.upgrade(), shared.upgrade()) {
            refresh(&win, &state);
        }
    });
    refresh(win, state);
}

/// Shows where the video is.
fn refresh(win: &PlayerWindow, state: &Shared) {
    let mut s = state.borrow_mut();
    if s.pcm.is_none() {
        return;
    }
    if s.output.as_ref().is_some_and(audio::Output::finished) {
        s.output = None;
        s.start = 0.0;
        if win.get_loop() {
            s.play();
        }
    }
    let (position, total) = (s.position(), s.total());
    let playing = s.running().is_some_and(audio::Output::is_playing);
    if !playing {
        s.timer.stop();
    }
    if let Some(video) = &s.video {
        video.show_at(position);
    }
    win.set_playing(playing);
    win.set_position(position / total);
    win.set_time(format!("{} / {}", browser::seconds(position), browser::seconds(total)).into());
}

fn wire(win: &PlayerWindow, state: &Shared, id: u64) {
    let with = |f: fn(&PlayerWindow, &Shared)| {
        let (weak, state) = (win.as_weak(), Rc::downgrade(state));
        move || {
            if let (Some(win), Some(state)) = (weak.upgrade(), state.upgrade()) {
                f(&win, &state);
            }
        }
    };
    win.on_toggle(with(|win, state| {
        if win.get_ready() {
            toggle(win, state);
        }
    }));
    win.on_stop(with(|win, state| {
        {
            let mut s = state.borrow_mut();
            s.output = None;
            s.start = 0.0;
        }
        refresh(win, state);
    }));
    win.on_volume_changed(with(|win, state| {
        VOLUME.with(|v| v.set(win.get_volume()));
        if let Some(o) = &state.borrow().output {
            o.set_volume(win.get_volume());
        }
    }));
    let (weak, shared) = (win.as_weak(), Rc::downgrade(state));
    win.on_seek(move |fraction| {
        if let (Some(win), Some(state)) = (weak.upgrade(), shared.upgrade()) {
            {
                let mut s = state.borrow_mut();
                let secs = s.total() * fraction.clamp(0.0, 1.0);
                s.seek(secs);
            }
            refresh(&win, &state);
        }
    });
    let (weak, shared) = (win.as_weak(), Rc::downgrade(state));
    win.on_skip(move |secs| {
        if let (Some(win), Some(state)) = (weak.upgrade(), shared.upgrade()) {
            {
                let mut s = state.borrow_mut();
                let target = s.position() + secs;
                s.seek(target);
            }
            refresh(&win, &state);
        }
    });
    win.on_close_player(move || forget(id));
    win.window().on_close_requested(move || {
        forget(id);
        slint::CloseRequestResponse::HideWindow
    });
}

/// Closes a window: its sound and its decoder stop.
fn forget(id: u64) {
    let gone = OPEN.with(|o| {
        let mut open = o.borrow_mut();
        open.iter().position(|(i, ..)| *i == id).map(|i| open.remove(i))
    });
    if let Some((_, win, state)) = gone {
        let _ = win.hide();
        let mut s = state.borrow_mut();
        s.timer.stop();
        s.output = None;
        s.video = None;
        // The callbacks of the window may be running.
        slint::Timer::single_shot(Duration::ZERO, move || drop(win));
    }
}

pub(crate) fn set_theme(mode: i32) {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w, _)| w.set_theme_mode(mode)));
}

pub(crate) fn set_accent() {
    OPEN.with(|o| o.borrow().iter().for_each(|(_, w, _)| accent::apply(w)));
}

/// The newest window (for debug snapshots).
#[cfg(debug_assertions)]
pub(crate) fn newest() -> Option<PlayerWindow> {
    OPEN.with(|o| o.borrow().last().map(|(_, w, _)| w.clone_strong()))
}
