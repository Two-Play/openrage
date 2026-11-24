//! Playing Bink videos in the preview; [`crate::player`] has a window for them.
//!
//! The sound (silence if the video has none) goes through the sound player,
//! whose position is the clock; a worker thread decodes the picture for it.

use crate::*;
use rage_formats::awc::Pcm;
use rage_formats::bink::{self, Player};
use std::sync::{Condvar, Mutex};

/// While the decoder catches up, a picture is shown this often, so that
/// seeking far in a long video shows progress.
const CATCH_UP_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq)]
enum Job {
    Show(usize),
    Stop,
}

/// The decoder thread of the selected video; ends when dropped.
pub(crate) struct Video {
    job: Arc<(Mutex<Job>, Condvar)>,
    fps: f64,
    frames: usize,
}

impl Video {
    fn send(&self, job: Job) {
        let (lock, changed) = &*self.job;
        let mut now = lock.lock().unwrap_or_else(|e| e.into_inner());
        if *now != job {
            *now = job;
            changed.notify_one();
        }
    }

    /// Asks for the picture at `secs`.
    pub(crate) fn show_at(&self, secs: f32) {
        let frame = (f64::from(secs.max(0.0)) * self.fps) as usize;
        self.send(Job::Show(frame.min(self.frames.saturating_sub(1))));
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        self.send(Job::Stop);
    }
}

fn picture(player: &Player<Shared>) -> Picture {
    let f = player.frame();
    slint::SharedPixelBuffer::clone_from_slice(&f.rgba(), f.width, f.height)
}

/// Sound is filled up with silence to this many samples (46 minutes of stereo at 48 kHz).
const MAX_SILENCE: usize = 1 << 28;

/// The sound of a video, as long as the video. Silence if there is none.
fn sound(video: &bink::Bink, data: &[u8]) -> (Pcm, Option<String>) {
    let (mut pcm, error) = match video.sound(data, 0) {
        Some(Ok(pcm)) => (pcm, None),
        Some(Err(e)) => (Pcm { sample_rate: 8000, channels: 1, samples: Vec::new() }, Some(e.to_string())),
        None => (Pcm { sample_rate: 8000, channels: 1, samples: Vec::new() }, None),
    };
    // Silence up to the end; not without limit, should the frame rate be damaged.
    let samples = (video.duration() * f64::from(pcm.sample_rate)) as usize;
    let samples = samples.saturating_mul(pcm.channels.max(1) as usize).min(MAX_SILENCE);
    pcm.samples.resize(samples.max(pcm.samples.len()).max(1), 0);
    (pcm, error)
}

/// A video file, shared by the threads that decode picture and sound.
#[derive(Clone)]
struct Shared(Arc<Vec<u8>>);

impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

pub(crate) type Picture = slint::SharedPixelBuffer<slint::Rgba8Pixel>;

/// What opening and playing a video gives, in this order.
pub(crate) enum Event {
    /// Facts about it, its first picture and its decoder.
    Opened(Vec<(String, String)>, Picture, Video),
    /// Its sound, and why there is none if it cannot be read.
    Sound(Pcm, Option<String>),
    Picture(Picture),
    /// It cannot be read; nothing follows.
    Failed(String),
}

/// Opens a video in the background. `event` is called from other threads, and
/// gives whether the events are still wanted. The decoder runs until the
/// [`Video`] is dropped.
pub(crate) fn open_video(
    vpath: String,
    game_dir: Option<PathBuf>,
    keys: Option<Arc<GtaVKeys>>,
    event: impl Fn(Event) -> bool + Send + Clone + 'static,
) {
    std::thread::spawn(move || {
        let opened = (|| -> anyhow::Result<_> {
            let game_dir = game_dir.ok_or_else(|| anyhow::anyhow!("no game folder"))?;
            let data = Shared(Arc::new(read_vpath(&game_dir, &vpath, keys)?));
            let rows = openrage_app::media::rows(data.as_ref())?;
            let mut player = Player::new(data.clone())?;
            player.seek(0)?;
            Ok((rows, player, data))
        })();
        let (rows, mut player, data) = match opened {
            Ok(v) => v,
            Err(e) => {
                event(Event::Failed(format!("{e:#}")));
                return;
            }
        };
        let job = Arc::new((Mutex::new(Job::Show(0)), Condvar::new()));
        let video =
            Video { job: Arc::clone(&job), fps: player.video.frames_per_second(), frames: player.video.frames() };
        if !event(Event::Opened(rows, picture(&player), video)) {
            return;
        }
        // The sound of a long video takes a while: the picture is there meanwhile.
        let (file, event_of_sound) = (player.video.clone(), event.clone());
        std::thread::spawn(move || {
            let (pcm, error) = sound(&file, data.as_ref());
            event_of_sound(Event::Sound(pcm, error));
        });
        decode_loop(&mut player, &job, |player| {
            event(Event::Picture(picture(player)));
        });
    });
}

/// Selecting a video: its first picture and the player.
pub(crate) fn start_video_preview(ui: &AppWindow, vpath: String, info: Meta, generation: u64) {
    let (counter, game_dir, keys) =
        with_app(|a| (Arc::clone(&a.preview_generation), a.game_dir.clone(), a.keys.clone()));
    ui.set_preview_loading(true);
    let weak = ui.as_weak();
    open_video(vpath, game_dir, keys, move |event| {
        let info = info.clone();
        let here = Arc::clone(&counter);
        counter.load(Ordering::SeqCst) == generation
            && weak
                .upgrade_in_event_loop(move |ui| {
                    if here.load(Ordering::SeqCst) == generation {
                        preview_event(&ui, event, info);
                    }
                })
                .is_ok()
    });
}

fn preview_event(ui: &AppWindow, event: Event, mut meta: Meta) {
    match event {
        Event::Opened(rows, image, video) => {
            ui.set_preview_loading(false);
            meta.extend(rows);
            set_meta(ui, &meta);
            ui.set_preview_image(slint::Image::from_rgba8(image));
            ui.set_has_image(true);
            ui.set_video_mode(true);
            with_app(|a| a.video = Some(video));
            if with_app(|a| std::mem::take(&mut a.play_when_ready)) {
                player::open_selected(ui);
            }
        }
        Event::Sound(pcm, error) => {
            if let Some(e) = error {
                ui.set_status(format!("The sound of this video cannot be read: {e}").into());
            }
            with_app(|a| {
                a.sound = Some(Arc::new(pcm));
                a.sound_start = 0.0;
            });
            ui.set_audio_mode(true);
            ui.set_audio_has_prev(false);
            ui.set_audio_has_next(false);
            update_sound_ui(ui);
        }
        Event::Picture(image) => ui.set_preview_image(slint::Image::from_rgba8(image)),
        Event::Failed(e) => {
            meta.push(kv("Error", e));
            ui.set_preview_loading(false);
            with_app(|a| a.play_when_ready = false);
            set_meta(ui, &meta);
        }
    }
}

/// Decodes towards the wanted frame and shows it, until told to stop.
fn decode_loop(player: &mut Player<Shared>, job: &(Mutex<Job>, Condvar), mut show: impl FnMut(&Player<Shared>)) {
    let (lock, changed) = job;
    let mut shown = player.position();
    let mut shown_at = Instant::now();
    // After a damaged frame: wait for another wish instead of trying again.
    let mut failed = None;
    loop {
        let wanted = {
            let mut now = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match *now {
                    Job::Stop => return,
                    Job::Show(f) if shown != Some(f) && failed != Some(f) => break f,
                    _ => now = changed.wait(now).unwrap_or_else(|e| e.into_inner()),
                }
            }
        };
        match player.step_towards(wanted) {
            Ok(there) => {
                if there || shown_at.elapsed() >= CATCH_UP_INTERVAL {
                    show(player);
                    shown_at = Instant::now();
                }
                shown = if there { Some(wanted) } else { None };
            }
            Err(_) => failed = Some(wanted),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_map_to_frames() {
        let job = Arc::new((Mutex::new(Job::Show(0)), Condvar::new()));
        let video = Video { job: Arc::clone(&job), fps: 30.0, frames: 90 };
        let wanted = |secs| {
            video.show_at(secs);
            *job.0.lock().unwrap()
        };
        assert!(wanted(0.0) == Job::Show(0));
        assert!(wanted(1.51) == Job::Show(45));
        assert!(wanted(99.0) == Job::Show(89));
        assert!(wanted(-1.0) == Job::Show(0));
        drop(video);
        assert!(*job.0.lock().unwrap() == Job::Stop);
    }
}
