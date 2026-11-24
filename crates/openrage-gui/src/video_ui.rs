//! Playing Bink 2 videos in the preview.
//!
//! The sound (silence if the video has none) goes through the sound player,
//! whose position is the clock; a worker thread decodes the picture for it.

use crate::*;
use rage_formats::awc::Pcm;
use rage_formats::bink2::{self, Player};
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

fn picture(player: &Player<Vec<u8>>) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    let f = player.frame();
    slint::SharedPixelBuffer::clone_from_slice(&f.rgba(), f.width, f.height)
}

/// Sound is filled up with silence to this many samples (46 minutes of stereo at 48 kHz).
const MAX_SILENCE: usize = 1 << 28;

/// The sound of a video, as long as the video. Silence if there is none.
fn sound(video: &bink2::Bink2, data: &[u8]) -> (Pcm, Option<String>) {
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

/// Selecting a `.bk2`: its first picture and the player.
pub(crate) fn start_video_preview(ui: &AppWindow, vpath: String, info: Meta, generation: u64) {
    let (counter, game_dir, keys) =
        with_app(|a| (Arc::clone(&a.preview_generation), a.game_dir.clone(), a.keys.clone()));
    ui.set_preview_loading(true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let current = || counter.load(Ordering::SeqCst) == generation;
        let mut meta = info;
        let opened = (|| -> anyhow::Result<_> {
            let game_dir = game_dir.ok_or_else(|| anyhow::anyhow!("no game folder"))?;
            let data = read_vpath(&game_dir, &vpath, keys)?;
            if !current() {
                return Ok(None);
            }
            meta.extend(openrage_app::media::rows(&vpath, &data)?);
            let mut player = Player::new(data)?;
            player.seek(0)?;
            let image = picture(&player);
            let (pcm, error) = sound(&player.video, player.data());
            Ok(Some((player, image, pcm, error)))
        })();
        if !current() {
            return;
        }
        let (mut player, image, pcm, sound_error) = match opened {
            Ok(Some(v)) => v,
            Ok(None) => return,
            Err(e) => {
                meta.push(kv("Error", format!("{e:#}")));
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    if counter.load(Ordering::SeqCst) == generation {
                        ui.set_preview_loading(false);
                        with_app(|a| a.play_when_ready = false);
                        set_meta(&ui, &meta);
                    }
                });
                return;
            }
        };
        if let Some(e) = sound_error {
            meta.push(kv("Sound", e));
        }
        let job = Arc::new((Mutex::new(Job::Show(0)), Condvar::new()));
        let video =
            Video { job: Arc::clone(&job), fps: player.video.frames_per_second(), frames: player.video.frames() };
        let shown = weak.upgrade_in_event_loop({
            let counter = Arc::clone(&counter);
            move |ui| {
                if counter.load(Ordering::SeqCst) != generation {
                    return; // drops the video, which ends its thread
                }
                ui.set_preview_loading(false);
                set_meta(&ui, &meta);
                ui.set_preview_image(slint::Image::from_rgba8(image));
                ui.set_has_image(true);
                with_app(|a| {
                    a.sound = Some(Arc::new(pcm));
                    a.sound_start = 0.0;
                    a.video = Some(video);
                });
                ui.set_video_mode(true);
                ui.set_audio_mode(true);
                ui.set_audio_has_prev(false);
                ui.set_audio_has_next(false);
                update_sound_ui(&ui);
                if with_app(|a| std::mem::take(&mut a.play_when_ready)) {
                    toggle_sound(&ui);
                }
            }
        });
        if shown.is_err() {
            return;
        }
        decode_loop(&mut player, &job, |player| {
            let image = picture(player);
            let counter = Arc::clone(&counter);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if counter.load(Ordering::SeqCst) == generation {
                    ui.set_preview_image(slint::Image::from_rgba8(image));
                }
            });
        });
    });
}

/// Decodes towards the wanted frame and shows it, until told to stop.
fn decode_loop(player: &mut Player<Vec<u8>>, job: &(Mutex<Job>, Condvar), mut show: impl FnMut(&Player<Vec<u8>>)) {
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
