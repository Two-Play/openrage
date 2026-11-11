//! Playing decoded sounds and drawing their waveform.

use std::num::NonZero;
use std::sync::Arc;
use std::time::Duration;

use rage_formats::awc::Pcm;

/// The audio output, opened on first use and kept while sounds play.
pub struct Output {
    _sink: rodio::MixerDeviceSink,
    player: rodio::Player,
    pcm: Arc<Pcm>,
}

impl Output {
    /// Starts playing `pcm` from `start`.
    pub fn play(pcm: Arc<Pcm>, start: Duration) -> Result<Self, String> {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        sink.log_on_drop(false);
        let player = rodio::Player::connect_new(sink.mixer());
        let channels = NonZero::new(pcm.channels.max(1)).expect("non-zero");
        let rate = NonZero::new(pcm.sample_rate.max(1)).expect("non-zero");
        let samples: Vec<rodio::Sample> = pcm.samples.iter().map(|&s| s as rodio::Sample / 32768.0).collect();
        player.append(rodio::buffer::SamplesBuffer::new(channels, rate, samples));
        if !start.is_zero() {
            let _ = player.try_seek(start);
        }
        player.play();
        Ok(Self { _sink: sink, player, pcm })
    }

    pub fn pcm(&self) -> &Arc<Pcm> {
        &self.pcm
    }

    pub fn is_playing(&self) -> bool {
        !self.player.is_paused() && !self.player.empty()
    }

    pub fn finished(&self) -> bool {
        self.player.empty()
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn resume(&self) {
        self.player.play();
    }

    pub fn seek(&self, pos: Duration) {
        let _ = self.player.try_seek(pos);
    }

    pub fn position(&self) -> Duration {
        self.player.get_pos()
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
