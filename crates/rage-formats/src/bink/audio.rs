// Ported to Rust from Librempeg (https://github.com/librempeg/librempeg),
// libavcodec/binkaudio.c,
// Copyright (c) 2007-2011 Peter Ross, 2009 Daniel Verkamp,
// GNU General Public License version 3 or later; used here under version 3.

//! Bink audio: blocks of a real Fourier or cosine transform that overlap by
//! a sixteenth.

use super::bits::Bits;
use super::{BinkError, Track};
use std::f32::consts::PI;

/// Upper frequencies of the critical bands.
const BANDS: [u32; 25] = [
    100, 200, 300, 400, 510, 630, 770, 920, 1080, 1270, 1480, 1720, 2000, 2320, 2700, 3150, 3700, 4400, 5300, 6400,
    7700, 9500, 12000, 15500, 24500,
];

/// Run lengths in 8 coefficients.
const RUNS: [usize; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 32, 64];

pub struct Decoder {
    dct: bool,
    /// The newer bit layout of revision `n`.
    version_2: bool,
    /// Separately coded channels (the Fourier kind codes them interleaved).
    channels: usize,
    len: usize,
    root: f32,
    quant: [f32; 96],
    bands: Vec<usize>,
    previous: Vec<Vec<f32>>,
    twiddles: Vec<Complex>,
    /// Samples all blocks of a packet may have.
    limit: usize,
}

impl Decoder {
    pub fn new(track: &Track, revision: u8) -> Result<Self, BinkError> {
        let mut rate = track.sample_rate;
        if rate == 0 {
            return super::bad("sound without a sample rate");
        }
        let mut bits = match rate {
            ..22050 => 9,
            22050..44100 => 10,
            _ => 11,
        };
        let mut channels = track.channels as usize;
        if !track.dct {
            rate *= u32::from(track.channels);
            bits += channels.ilog2();
            channels = 1;
        }
        let len = 1usize << bits;
        let root = if track.dct { (len as f32).sqrt() / 32768.0 } else { 2.0 / ((len as f32).sqrt() * 32768.0) };
        let half = rate.div_ceil(2);
        let count = 1 + BANDS[..24].iter().take_while(|&&b| half > b).count();
        let mut bands = vec![2];
        bands.extend(BANDS[..count - 1].iter().map(|b| (*b as usize * len / half as usize) & !1));
        bands.push(len);
        Ok(Self {
            dct: track.dct,
            version_2: revision == b'n',
            channels,
            len,
            root,
            quant: std::array::from_fn(|i| (i as f32 * 0.152_891_65).exp() * root),
            bands,
            previous: Vec::new(),
            twiddles: twiddles(if track.dct { 2 * len } else { len }),
            limit: 1 << 22,
        })
    }

    /// Decodes a packet and appends its samples, channels interleaved.
    pub fn packet(&mut self, data: &[u8], out: &mut Vec<i16>) -> Result<(), BinkError> {
        if data.len() < 4 {
            return Ok(());
        }
        let mut bits = Bits::new(data);
        // The size of the decoded samples in bytes; blocks may hold more.
        let bytes = bits.read(32) as usize;
        let start = out.len();
        while bits.left() > 0 && out.len() - start < bytes.div_ceil(2).min(self.limit) {
            self.block(&mut bits, out)?;
            bits.align(32);
        }
        Ok(())
    }

    fn block(&mut self, bits: &mut Bits, out: &mut Vec<i16>) -> Result<(), BinkError> {
        if self.dct {
            bits.read(2);
        }
        let overlap = self.len / 16;
        let mut blocks = Vec::with_capacity(self.channels);
        for _ in 0..self.channels {
            let coeffs = self.coefficients(bits)?;
            blocks.push(if self.dct { dct(&coeffs, &self.twiddles) } else { rdft(&coeffs, &self.twiddles) });
        }
        for (ch, block) in blocks.iter_mut().enumerate() {
            let count = (overlap * self.channels) as f32;
            if let Some(previous) = self.previous.get(ch) {
                for (i, (b, p)) in block.iter_mut().zip(previous).enumerate() {
                    let j = (ch + i * self.channels) as f32;
                    *b = (p * (count - j) + *b * j) / count;
                }
            }
        }
        for i in 0..self.len - overlap {
            out.extend(blocks.iter().map(|b| (b[i] * 32768.0).round() as i16));
        }
        self.previous = blocks.into_iter().map(|mut b| b.split_off(self.len - overlap)).collect();
        Ok(())
    }

    fn coefficients(&self, bits: &mut Bits) -> Result<Vec<f32>, BinkError> {
        let mut coeffs = vec![0f32; self.len];
        for c in &mut coeffs[..2] {
            let power = bits.read(5) as i32;
            let v = bits.read(23) as f32 * 2f32.powi(power - 23);
            *c = if bits.bit() { -v } else { v } * self.root;
        }
        let n = if self.version_2 { 7 } else { 8 };
        let quant: Vec<f32> = (1..self.bands.len()).map(|_| self.quant[(bits.read(n) as usize).min(95)]).collect();
        let (mut k, mut q, mut i) = (0, quant[0], 2);
        while i < self.len {
            if bits.overread() {
                return super::bad("sound ends early");
            }
            let run = if bits.bit() { RUNS[bits.read(4) as usize] * 8 } else { 8 };
            let end = (i + run).min(self.len);
            let width = bits.read(4);
            if width == 0 {
                i = end;
                while k < quant.len() && self.bands[k] < i {
                    q = quant[k];
                    k += 1;
                }
                continue;
            }
            let mut values = [0u32; 512];
            if self.version_2 {
                values[..end - i].iter_mut().for_each(|v| *v = bits.read(width));
            }
            for (j, v) in (i..end).zip(values) {
                if k < quant.len() && self.bands[k] == j {
                    q = quant[k];
                    k += 1;
                }
                let v = if self.version_2 { v } else { bits.read(width) };
                if v != 0 {
                    coeffs[j] = if bits.bit() { -q } else { q } * v as f32;
                }
            }
            i = end;
        }
        Ok(coeffs)
    }
}

type Complex = (f32, f32);

/// e^(2πi·k/n) for the first half of k.
fn twiddles(n: usize) -> Vec<Complex> {
    (0..n / 2)
        .map(|i| {
            let (s, c) = (2.0 * PI * i as f32 / n as f32).sin_cos();
            (c, s)
        })
        .collect()
}

/// The sum over k of `v[k]`·e^(2πi·k·t/n), in place.
fn fft(v: &mut [Complex], twiddles: &[Complex]) {
    let n = v.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            v.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= n {
        let step = n / size;
        for part in v.chunks_exact_mut(size) {
            let (a, b) = part.split_at_mut(size / 2);
            for (i, (a, b)) in a.iter_mut().zip(b).enumerate() {
                let w = twiddles[i * step];
                let t = (b.0 * w.0 - b.1 * w.1, b.0 * w.1 + b.1 * w.0);
                *b = (a.0 - t.0, a.1 - t.1);
                *a = (a.0 + t.0, a.1 + t.1);
            }
        }
        size *= 2;
    }
}

/// Real samples from coefficients: first and middle, then the others as
/// conjugated complex numbers; scaled by a half.
fn rdft(c: &[f32], twiddles: &[Complex]) -> Vec<f32> {
    let n = c.len();
    let mut v = vec![(0.0, 0.0); n];
    v[0].0 = c[0];
    v[n / 2].0 = c[1];
    for k in 1..n / 2 {
        v[k] = (c[2 * k], -c[2 * k + 1]);
        v[n - k] = (c[2 * k], c[2 * k + 1]);
    }
    fft(&mut v, twiddles);
    v.into_iter().map(|v| v.0 * 0.5).collect()
}

/// The sum over k of `c[k]`·cos(π·k·(2t+1)/2n), scaled by 2/n.
fn dct(c: &[f32], twiddles: &[Complex]) -> Vec<f32> {
    let n = c.len();
    // Real part of a Fourier transform of twice the length.
    let mut v = vec![(0.0, 0.0); 2 * n];
    for (k, (v, c)) in v.iter_mut().zip(c).enumerate() {
        let (s, co) = (PI * k as f32 / (2 * n) as f32).sin_cos();
        *v = (c * co, c * s);
    }
    fft(&mut v, twiddles);
    v[..n].iter().map(|v| v.0 * 2.0 / n as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_match_their_sums() {
        let n = 16;
        let c: Vec<f32> = (0..n).map(|i| ((i * 7 + 3) % 11) as f32 - 5.0).collect();
        let (r, d) = (rdft(&c, &twiddles(n)), dct(&c, &twiddles(2 * n)));
        for t in 0..n {
            let angle = |k: usize| 2.0 * PI * (k * t) as f32 / n as f32;
            let mut sum = (c[0] + c[1] * angle(n / 2).cos()) / 2.0;
            (1..n / 2).for_each(|k| sum += c[2 * k] * angle(k).cos() + c[2 * k + 1] * angle(k).sin());
            assert!((r[t] - sum).abs() < 1e-3, "{t}: {} {sum}", r[t]);
            let sum: f32 = (0..n).map(|k| c[k] * (PI * (k * (2 * t + 1)) as f32 / (2 * n) as f32).cos()).sum::<f32>()
                * 2.0
                / n as f32;
            assert!((d[t] - sum).abs() < 1e-3, "{t}: {} {sum}", d[t]);
        }
    }

    #[test]
    fn garbage() {
        let mut seed = 1u32;
        for round in 0..40 {
            let track = Track {
                id: 0,
                sample_rate: [8000, 22050, 44100, 48000, 65535][round % 5],
                channels: 1 + (round % 2) as u16,
                dct: round % 3 == 0,
            };
            let mut d = Decoder::new(&track, if round % 4 == 0 { b'n' } else { b'j' }).unwrap();
            let data: Vec<u8> = (0..3000)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                })
                .collect();
            let mut out = Vec::new();
            let _ = d.packet(&data, &mut out);
        }
    }

    #[test]
    fn silence() {
        let track = Track { id: 0, sample_rate: 48000, channels: 2, dct: true };
        let mut d = Decoder::new(&track, b'j').unwrap();
        let mut out = Vec::new();
        let mut data = vec![0u8; 2000];
        data[..4].copy_from_slice(&7680u32.to_le_bytes());
        d.packet(&data, &mut out).unwrap();
        assert_eq!(out, [0; 3840]);
    }
}
