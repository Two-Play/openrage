// Ported to Rust from Librempeg (https://github.com/librempeg/librempeg),
// libavformat/bink.c, Copyright (c) 2008-2010 Peter Ross, 2009 Daniel Verkamp,
// GNU General Public License version 3 or later; used here under version 3.

//! Bink videos (`.bik` and, Bink 2, `.bk2`): the file, its pictures
//! ([`Player`]) and its sound.
//!
//! Of Bink 1 the revisions `f` to `k` are decoded, of Bink 2 `g` to `n`; the
//! earlier ones use other transforms and are not. The file and the two sound
//! codecs are the same in both.

mod audio;
mod bits;
mod tables1;
mod tables2;
mod video1;
mod video2;

use rage_core::bytes::LeRead;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BinkError {
    #[error("damaged Bink video: {0}")]
    Invalid(String),
    #[error("Bink revision {0:?} is not supported")]
    Unsupported(char),
}

fn bad<T>(what: impl Into<String>) -> Result<T, BinkError> {
    Err(BinkError::Invalid(what.into()))
}

/// One decoded picture: YCbCr 4:2:0, with alpha if the video has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Black and white are 0 and 255, otherwise 16 and 235.
    pub full_range: bool,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
    pub a: Option<Vec<u8>>,
}

/// A sound track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Cosine transform, otherwise real Fourier transform.
    pub dct: bool,
}

/// Where a frame is in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub key: bool,
    start: usize,
    end: usize,
}

/// A Bink file's header and index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bink {
    /// Bink 1 or 2.
    pub version: u8,
    pub revision: u8,
    pub width: u32,
    pub height: u32,
    /// Frames per second: numerator and denominator.
    pub fps: (u32, u32),
    pub flags: u32,
    pub tracks: Vec<Track>,
    pub packets: Vec<Packet>,
}

/// Limits of RAD's tools.
const MAX_WIDTH: u32 = 32767;
const MAX_PIXELS: u64 = 8192 * 8192;
const MAX_TRACKS: usize = 256;

/// Whether the file is a Bink video.
pub fn is_bink(data: &[u8]) -> bool {
    data.starts_with(b"KB2") || (data.starts_with(b"BIK") && data.get(3).is_some_and(u8::is_ascii_alphanumeric))
}

/// A 32-bit word of the header.
fn header_u32(data: &[u8], at: usize) -> Result<u32, BinkError> {
    match data.u32_at(at) {
        Some(v) => Ok(v),
        None => bad(format!("the header ends before {at:#x}")),
    }
}

/// The fixed part of the header.
struct Head {
    version: u8,
    revision: u8,
    frames: usize,
    width: u32,
    height: u32,
    fps: (u32, u32),
    flags: u32,
    /// Number of sound tracks.
    count: usize,
}

/// Reads the header and the frame index.
pub fn read(data: &[u8]) -> Result<Bink, BinkError> {
    if !is_bink(data) {
        return bad("no Bink signature");
    }
    let Head { version, revision, frames, width, height, fps, flags, count } = read_head(data)?;
    let mut at = 0x2C + if revision == b'k' || (version == 2 && revision >= b'i') { 4 } else { 0 };
    let tracks = read_tracks(data, at, count)?;
    at += 12 * count;
    if frames > (data.len() - at.min(data.len())) / 4 {
        return bad(format!("{frames} frames do not fit the file"));
    }
    let packets = read_packets(data, at, frames)?;
    Ok(Bink { version, revision, width, height, fps, flags, tracks, packets })
}

fn read_head(data: &[u8]) -> Result<Head, BinkError> {
    let version = if data[0] == b'K' { 2 } else { 1 };
    let revision = header_u32(data, 0)?.to_le_bytes()[3];
    let frames = header_u32(data, 0x08)? as usize;
    let (width, height) = read_size(data)?;
    let fps = read_fps(data)?;
    let flags = header_u32(data, 0x24)?;
    let count = header_u32(data, 0x28)? as usize;
    if count > MAX_TRACKS {
        return bad(format!("{count} sound tracks"));
    }
    Ok(Head { version, revision, frames, width, height, fps, flags, count })
}

/// Frames per second: numerator and denominator.
fn read_fps(data: &[u8]) -> Result<(u32, u32), BinkError> {
    let fps = (header_u32(data, 0x1C)?, header_u32(data, 0x20)?);
    if fps.0 == 0 || fps.1 == 0 {
        return bad("no frame rate");
    }
    Ok(fps)
}

/// Width and height, which have to be within the limits of RAD's tools.
fn read_size(data: &[u8]) -> Result<(u32, u32), BinkError> {
    let (width, height) = (header_u32(data, 0x14)?, header_u32(data, 0x18)?);
    let coded = |side: u32| u64::from(side).next_multiple_of(32);
    if width == 0 || height == 0 || width > MAX_WIDTH || height > MAX_WIDTH || coded(width) * coded(height) > MAX_PIXELS
    {
        return bad(format!("{width} × {height} pixels"));
    }
    Ok((width, height))
}

/// The sound tracks: `count` of them, their infos and then their ids.
fn read_tracks(data: &[u8], at: usize, count: usize) -> Result<Vec<Track>, BinkError> {
    let mut tracks = Vec::with_capacity(count);
    for i in 0..count {
        let info = header_u32(data, at + 4 * count + 4 * i)?;
        tracks.push(Track {
            id: header_u32(data, at + 8 * count + 4 * i)?,
            sample_rate: info & 0xFFFF,
            channels: if info & 0x2000_0000 != 0 { 2 } else { 1 },
            dct: info & 0x1000_0000 != 0,
        });
    }
    Ok(tracks)
}

/// The frame index.
fn read_packets(data: &[u8], at: usize, frames: usize) -> Result<Vec<Packet>, BinkError> {
    // One more entry than frames, the last is the end of the file.
    let mut packets = Vec::with_capacity(frames);
    for i in 0..frames {
        let here = header_u32(data, at + 4 * i)?;
        let next = if i + 1 < frames { header_u32(data, at + 4 * i + 4)? as usize & !1 } else { data.len() };
        let (start, end) = (here as usize & !1, next.min(data.len()));
        if start > end {
            return bad(format!("frame {i} lies outside the file"));
        }
        packets.push(Packet { key: i == 0 || here & 1 != 0, start, end });
    }
    Ok(packets)
}

impl Bink {
    pub fn frames(&self) -> usize {
        self.packets.len()
    }

    pub fn frames_per_second(&self) -> f64 {
        f64::from(self.fps.0) / f64::from(self.fps.1)
    }

    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.frames_per_second()
    }

    pub fn has_alpha(&self) -> bool {
        self.flags & video2::ALPHA != 0
    }

    /// The sound packets of a frame, one per track, and its picture.
    fn split<'a>(&self, data: &'a [u8], frame: usize) -> Result<(Vec<&'a [u8]>, &'a [u8]), BinkError> {
        let Some(p) = self.packets.get(frame) else { return bad(format!("there is no frame {frame}")) };
        let Some(mut rest) = data.get(p.start..p.end) else { return bad(format!("frame {frame} is cut off")) };
        let mut sounds = Vec::with_capacity(self.tracks.len());
        for _ in &self.tracks {
            let sound = rest.u32_at(0).and_then(|n| rest.get(4..(n as usize).checked_add(4)?));
            let Some(sound) = sound else { return bad(format!("the sound of frame {frame} is too long")) };
            sounds.push(sound);
            rest = &rest[4 + sound.len()..];
        }
        Ok((sounds, rest))
    }

    /// Decodes a sound track. `None` if there is no such track.
    pub fn sound(&self, data: &[u8], track: usize) -> Option<Result<crate::awc::Pcm, BinkError>> {
        let t = self.tracks.get(track)?;
        Some((|| {
            let mut decoder = audio::Decoder::new(t, self.revision)?;
            let mut samples = Vec::new();
            for frame in 0..self.frames() {
                decoder.packet(self.split(data, frame)?.0[track], &mut samples)?;
            }
            Ok(crate::awc::Pcm { sample_rate: t.sample_rate, channels: t.channels, samples })
        })())
    }
}

enum Decoder {
    One(Box<video1::Decoder>),
    Two(Box<video2::Decoder>),
}

/// Decodes the pictures of a video.
pub struct Player<D> {
    data: D,
    pub video: Bink,
    decoder: Decoder,
    /// The frame to decode next.
    next: usize,
}

impl<D: AsRef<[u8]>> Player<D> {
    pub fn new(data: D) -> Result<Self, BinkError> {
        let video = read(data.as_ref())?;
        let (revision, flags, width, height) = (video.revision, video.flags, video.width, video.height);
        let decoder = match video.version {
            1 => Decoder::One(video1::Decoder::new(revision, flags, width, height)?.into()),
            _ => Decoder::Two(video2::Decoder::new(revision, flags, width, height)?.into()),
        };
        Ok(Self { data, video, decoder, next: 0 })
    }

    pub fn data(&self) -> &[u8] {
        self.data.as_ref()
    }

    /// Decodes the next frame. Gives the number of bytes left unread in each
    /// of its slices (of Bink 2): at most 3 of padding in an undamaged video.
    pub fn advance(&mut self) -> Result<Vec<usize>, BinkError> {
        let picture = self.video.split(self.data.as_ref(), self.next)?.1;
        // A frame without data repeats the one before.
        let left = if picture.is_empty() && self.next > 0 {
            Vec::new()
        } else {
            match &mut self.decoder {
                Decoder::One(d) => d.decode(picture).map(|()| Vec::new())?,
                Decoder::Two(d) => d.decode(picture, self.video.packets[self.next].key)?,
            }
        };
        self.next += 1;
        Ok(left)
    }

    /// Decodes one frame on the way to `frame`, jumping to the key frame
    /// before it unless the decoder is already in between. Gives whether it
    /// is there.
    pub fn step_towards(&mut self, frame: usize) -> Result<bool, BinkError> {
        let Some(before) = self.video.packets.get(..=frame) else { return bad(format!("there is no frame {frame}")) };
        if self.next == frame + 1 {
            return Ok(true);
        }
        let key = before.iter().rposition(|p| p.key).unwrap_or(0);
        if self.next > frame || self.next < key {
            self.next = key;
        }
        self.advance()?;
        Ok(self.next == frame + 1)
    }

    /// Decodes up to `frame`.
    pub fn seek(&mut self, frame: usize) -> Result<(), BinkError> {
        while !self.step_towards(frame)? {}
        Ok(())
    }

    /// The frame decoded last, if any.
    pub fn position(&self) -> Option<usize> {
        self.next.checked_sub(1)
    }

    /// The picture decoded last.
    pub fn frame(&self) -> Frame {
        match &self.decoder {
            Decoder::One(d) => d.frame(),
            Decoder::Two(d) => d.frame(),
        }
    }
}

impl Frame {
    /// The picture as RGBA (BT.601 weights), opaque if it has no alpha.
    pub fn rgba(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![255; w * h * 4];
        if w == 0 {
            return out;
        }
        let half = w.div_ceil(2);
        // In 1/65536: black, then the factors of the planes.
        let (black, l_y, cr_r, cb_g, cr_g, cb_b) = if self.full_range {
            (0, 1 << 16, 91881, 22554, 46802, 116_130)
        } else {
            (16, 76309, 104_597, 25675, 53279, 132_201)
        };
        for (y, row) in out.chunks_exact_mut(w * 4).enumerate() {
            let luma = &self.y[y * w..][..w];
            let (cb, cr) = (&self.cb[y / 2 * half..][..half], &self.cr[y / 2 * half..][..half]);
            for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (l, u, v) =
                    ((i32::from(luma[x]) - black) * l_y, i32::from(cb[x / 2]) - 128, i32::from(cr[x / 2]) - 128);
                let at = |c: i32| ((l + c + (1 << 15)) >> 16).clamp(0, 255) as u8;
                p[0] = at(cr_r * v);
                p[1] = at(-cb_g * u - cr_g * v);
                p[2] = at(cb_b * u);
            }
            if let Some(a) = &self.a {
                row.as_chunks_mut::<4>().0.iter_mut().zip(&a[y * w..]).for_each(|(p, &a)| p[3] = a);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(frames: &[&[u8]], track: Option<u32>) -> Vec<u8> {
        let mut d = b"KB2j".to_vec();
        let n = frames.len() as u32;
        for v in [0, n, 0, n, 32, 32, 30, 1, 0, u32::from(track.is_some()), 0] {
            d.extend(v.to_le_bytes());
        }
        if let Some(info) = track {
            for v in [0, info, 7] {
                d.extend(v.to_le_bytes());
            }
        }
        let mut at = d.len() as u32 + 4 * (n + 1);
        for f in frames {
            d.extend((at | u32::from(d.len() < 0x40)).to_le_bytes());
            at += f.len() as u32;
        }
        d.extend(at.to_le_bytes());
        frames.iter().for_each(|f| d.extend_from_slice(f));
        d
    }

    #[test]
    fn header_and_index() {
        let d = file(&[&[2, 0, 0, 0, 9, 9, 1, 1], &[0, 0, 0, 0, 5, 5]], Some(0x7000_BB80));
        let v = read(&d).unwrap();
        assert_eq!((v.width, v.height, v.frames(), v.frames_per_second()), (32, 32, 2, 30.0));
        assert_eq!(v.tracks, [Track { id: 7, sample_rate: 48000, channels: 2, dct: true }]);
        assert!(v.packets[0].key && !v.packets[1].key);
        assert_eq!(v.split(&d, 0).unwrap(), (vec![&[9u8, 9][..]], &[1u8, 1][..]));
        assert_eq!(v.split(&d, 1).unwrap(), (vec![&[][..]], &[5u8, 5][..]));
        assert!(v.split(&d, 2).is_err());
    }

    #[test]
    fn damaged_files() {
        let d = file(&[&[1, 2, 3, 4, 5, 6], &[7, 8]], Some(0x3000_BB80));
        for n in 0..d.len() {
            if let Ok(mut p) = Player::new(&d[..n]) {
                let _ = p.advance();
                let _ = p.seek(1);
                let _ = p.video.sound(&d[..n], 0);
            }
        }
        for i in 0..d.len() {
            for v in [0, 0x7F, 0xFF] {
                let mut d = d.clone();
                d[i] = v;
                if let Ok(v) = read(&d) {
                    let _ = v.sound(&d, 0);
                    let _ = v.split(&d, 0);
                }
            }
        }
    }

    #[test]
    fn colours() {
        let f = |y, cb, cr, a| {
            Frame { width: 1, height: 1, full_range: true, y: vec![y], cb: vec![cb], cr: vec![cr], a }.rgba()
        };
        assert_eq!(f(255, 128, 128, None), [255, 255, 255, 255]);
        assert_eq!(f(0, 128, 128, Some(vec![9])), [0, 0, 0, 9]);
        assert_eq!(f(76, 85, 255, None), [254, 0, 0, 255]);
    }
}
