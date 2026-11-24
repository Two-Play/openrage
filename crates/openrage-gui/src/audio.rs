//! Playing decoded sounds and drawing their waveform.

use std::cell::Cell;
use std::num::NonZero;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rage_formats::awc::Pcm;

/// Plays decoded samples without copying them.
struct Samples {
    pcm: Arc<Pcm>,
    next: usize,
}

impl Samples {
    fn channel_count(&self) -> usize {
        self.pcm.channels.max(1) as usize
    }
}

impl Iterator for Samples {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<rodio::Sample> {
        let sample = *self.pcm.samples.get(self.next)?;
        self.next += 1;
        Some(rodio::Sample::from(sample) / 32768.0)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.pcm.samples.len().saturating_sub(self.next);
        (left, Some(left))
    }
}

impl rodio::Source for Samples {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(self.pcm.channels).unwrap_or(NonZero::<u16>::MIN)
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(self.pcm.sample_rate).unwrap_or(NonZero::<u32>::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f32(self.pcm.duration_secs()))
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        let frame = (pos.as_secs_f64() * f64::from(self.pcm.sample_rate)) as usize;
        self.next = frame.saturating_mul(self.channel_count()).min(self.pcm.samples.len());
        Ok(())
    }
}

/// Something playing, opened on first use and kept while it plays.
pub struct Output {
    pcm: Arc<Pcm>,
    player: Player,
}

enum Player {
    Device {
        player: rodio::Player,
        _sink: rodio::MixerDeviceSink,
    },
    /// No sound: a clock. Where it stood, and since when it runs.
    Clock(Cell<(Duration, Option<Instant>)>),
}

impl Output {
    /// Starts playing `pcm` from `start`.
    pub fn play(pcm: Arc<Pcm>, start: Duration) -> Result<Self, String> {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        sink.log_on_drop(false);
        let player = rodio::Player::connect_new(sink.mixer());
        player.append(Samples { pcm: Arc::clone(&pcm), next: 0 });
        if !start.is_zero() {
            let _ = player.try_seek(start);
        }
        player.play();
        Ok(Self { pcm, player: Player::Device { player, _sink: sink } })
    }

    /// The same without sound, for what is not only heard.
    pub fn silent(pcm: Arc<Pcm>, start: Duration) -> Self {
        Self { pcm, player: Player::Clock(Cell::new((start, Some(Instant::now())))) }
    }

    pub fn pcm(&self) -> &Arc<Pcm> {
        &self.pcm
    }

    pub fn is_playing(&self) -> bool {
        match &self.player {
            Player::Device { player: p, .. } => !p.is_paused() && !p.empty(),
            Player::Clock(c) => c.get().1.is_some() && !self.finished(),
        }
    }

    pub fn finished(&self) -> bool {
        match &self.player {
            Player::Device { player: p, .. } => p.empty(),
            Player::Clock(_) => self.position().as_secs_f32() >= self.pcm.duration_secs(),
        }
    }

    pub fn pause(&self) {
        match &self.player {
            Player::Device { player: p, .. } => p.pause(),
            Player::Clock(c) => c.set((self.position(), None)),
        }
    }

    pub fn resume(&self) {
        match &self.player {
            Player::Device { player: p, .. } => p.play(),
            Player::Clock(c) => c.set((self.position(), Some(Instant::now()))),
        }
    }

    pub fn set_volume(&self, volume: f32) {
        if let Player::Device { player: p, .. } = &self.player {
            p.set_volume(volume);
        }
    }

    pub fn seek(&self, pos: Duration) {
        match &self.player {
            Player::Device { player: p, .. } => {
                let _ = p.try_seek(pos);
            }
            Player::Clock(c) => c.set((pos, c.get().1.map(|_| Instant::now()))),
        }
    }

    pub fn position(&self) -> Duration {
        match &self.player {
            Player::Device { player: p, .. } => p.get_pos(),
            Player::Clock(c) => {
                let (at, since) = c.get();
                at + since.map_or(Duration::ZERO, |s| s.elapsed())
            }
        }
    }
}

/// Draws the waveform (peak per column, channels mixed) as RGBA with a
/// transparent background.
pub fn waveform(pcm: &Pcm, width: usize, height: usize, color: [u8; 3]) -> Vec<u8> {
    let (w, h) = (width.max(1), height.max(2));
    let mut out = vec![0u8; w * h * 4];
    let channels = pcm.channels.max(1) as usize;
    let frames = pcm.samples.len() / channels;
    if frames == 0 {
        return out;
    }
    let mid = h as f32 / 2.0;
    for x in 0..w {
        let start = x * frames / w;
        let end = ((x + 1) * frames / w).max(start + 1).min(frames);
        let (mut lo, mut hi) = (0i32, 0i32);
        for f in start..end {
            let v: i32 = pcm.samples[f * channels..f * channels + channels].iter().map(|&s| s as i32).sum::<i32>()
                / channels as i32;
            lo = lo.min(v);
            hi = hi.max(v);
        }
        let top = (mid - hi as f32 / 32768.0 * mid * 0.95).floor().max(0.0) as usize;
        let bottom = (mid - lo as f32 / 32768.0 * mid * 0.95).ceil().min(h as f32 - 1.0) as usize;
        for y in top.min(bottom)..=bottom.max(top) {
            let i = (y * w + x) * 4;
            out[i..i + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source;

    #[test]
    fn samples_are_sought_by_frame() {
        let pcm = Arc::new(Pcm { sample_rate: 10, channels: 2, samples: (0..40).map(|i| i * 100).collect() });
        let mut s = Samples { pcm, next: 0 };
        s.try_seek(Duration::from_millis(500)).unwrap();
        assert_eq!(s.next(), Some(1000.0 / 32768.0));
        s.try_seek(Duration::from_secs(9)).unwrap();
        assert_eq!(s.next(), None);
    }

    #[test]
    fn the_clock_of_silence() {
        let pcm = Arc::new(Pcm { sample_rate: 1, channels: 1, samples: vec![0; 10] });
        let o = Output::silent(pcm, Duration::from_secs(3));
        assert!(o.is_playing());
        o.pause();
        o.seek(Duration::from_secs(7));
        assert!(!o.is_playing() && o.position() == Duration::from_secs(7));
        o.seek(Duration::from_secs(10));
        assert!(o.finished());
    }
}
