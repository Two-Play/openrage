//! Facts about media files the game plays: Scaleform movies (`.gfx`) and
//! Bink videos (`.bik`). Neither is played back; this reads their headers
//! (and, for `.gfx`, the names it exports and the images it loads).

use rage_core::bytes::LeRead;
use std::io::Read;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MediaError {
    #[error("invalid media file: {0}")]
    Invalid(String),
}

/// A Bink video's header.
#[derive(Debug, Clone, PartialEq)]
pub struct Bink {
    /// `BIK` + revision letter, or `KB2` + revision for Bink 2.
    pub signature: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps: f32,
    pub audio_tracks: u32,
    /// Size of the largest frame in bytes.
    pub largest_frame: u32,
}

impl Bink {
    pub fn duration(&self) -> f32 {
        if self.fps > 0.0 { self.frames as f32 / self.fps } else { 0.0 }
    }
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, MediaError> {
    d.u32_at(at).ok_or_else(|| MediaError::Invalid(format!("header ends before {at:#x}")))
}

/// Reads a Bink (1 or 2) header: signature, file size, frames, largest frame,
/// frames again, width, height, fps numerator and denominator, flags, audio tracks.
pub fn bink(data: &[u8]) -> Result<Bink, MediaError> {
    let sig = data.get(..4).ok_or_else(|| MediaError::Invalid("too short".into()))?;
    if &sig[..3] != b"BIK" && &sig[..3] != b"KB2" {
        return Err(MediaError::Invalid("not a Bink video".into()));
    }
    let (num, den) = (u32_at(data, 0x1C)?, u32_at(data, 0x20)?);
    Ok(Bink {
        signature: String::from_utf8_lossy(sig).into_owned(),
        frames: u32_at(data, 0x08)?,
        largest_frame: u32_at(data, 0x0C)?,
        width: u32_at(data, 0x14)?,
        height: u32_at(data, 0x18)?,
        fps: if den > 0 { num as f32 / den as f32 } else { 0.0 },
        audio_tracks: u32_at(data, 0x28)?,
    })
}

/// A Scaleform (Flash) movie's header and the names in it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gfx {
    pub version: u8,
    pub compressed: bool,
    pub width: f32,
    pub height: f32,
    pub fps: f32,
    pub frames: u16,
    pub tags: usize,
    /// Symbols the movie exports (used by scripts and other movies).
    pub exports: Vec<String>,
    /// Image files the movie loads from texture dictionaries.
    pub images: Vec<String>,
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            let byte = self.data.get(self.pos / 8).copied().unwrap_or(0);
            v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        v
    }
    fn signed(&mut self, n: u32) -> i32 {
        let v = self.read(n);
        if n > 0 && v & (1 << (n - 1)) != 0 { v as i32 - (1 << n) } else { v as i32 }
    }
}

fn cstr(d: &[u8], at: &mut usize) -> String {
    let rest = d.get(*at..).unwrap_or_default();
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    *at += end + 1;
    String::from_utf8_lossy(&rest[..end]).into_owned()
}

/// A length-prefixed (u8) string.
fn pstr(d: &[u8], at: &mut usize) -> String {
    let n = d.get(*at).copied().unwrap_or(0) as usize;
    let s = d.get(*at + 1..*at + 1 + n).unwrap_or_default();
    *at += 1 + n;
    String::from_utf8_lossy(s).trim_end_matches('\0').to_owned()
}

/// Reads a `.gfx` movie: `GFX` (or zlib-compressed `CFX`), version, length,
/// frame rectangle, frame rate, frame count, then tags.
pub fn gfx(data: &[u8]) -> Result<Gfx, MediaError> {
    let sig = data.get(..8).ok_or_else(|| MediaError::Invalid("too short".into()))?;
    let compressed = match &sig[..3] {
        b"GFX" | b"FWS" => false,
        b"CFX" | b"CWS" => true,
        _ => return Err(MediaError::Invalid("not a Scaleform movie".into())),
    };
    let length = u32_at(data, 4)? as usize;
    let body: Vec<u8> = if compressed {
        let mut out = Vec::with_capacity(length.saturating_sub(8).min(256 << 20));
        flate2::read::ZlibDecoder::new(&data[8..])
            .take(256 << 20)
            .read_to_end(&mut out)
            .map_err(|e| MediaError::Invalid(format!("cannot decompress: {e}")))?;
        out
    } else {
        data[8..].to_vec()
    };
    let mut bits = Bits { data: &body, pos: 0 };
    let n = bits.read(5);
    let (xmin, xmax, ymin, ymax) = (bits.signed(n), bits.signed(n), bits.signed(n), bits.signed(n));
    let mut at = bits.pos.div_ceil(8);
    let u16_at = |at: usize| body.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0);
    let fps = u16_at(at) as f32 / 256.0;
    let frames = u16_at(at + 2);
    at += 4;
    let mut out = Gfx {
        version: sig[3],
        compressed,
        width: (xmax - xmin) as f32 / 20.0,
        height: (ymax - ymin) as f32 / 20.0,
        fps,
        frames,
        ..Default::default()
    };
    while at + 2 <= body.len() {
        let head = u16_at(at);
        let (code, mut len) = (head >> 6, (head & 0x3F) as usize);
        at += 2;
        if len == 0x3F {
            len = body.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
            at += 4;
        }
        let tag = body.get(at..(at + len).min(body.len())).unwrap_or_default();
        out.tags += 1;
        match code {
            0 => break,
            // ExportAssets: count, then (id, name).
            56 => {
                let count = tag.get(..2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
                let mut p = 2;
                for _ in 0..count {
                    p += 2;
                    let name = cstr(tag, &mut p);
                    if !name.is_empty() {
                        out.exports.push(name);
                    }
                }
            }
            // GFx DefineExternalImage (1001) and DefineExternalImage2 (1009):
            // ids and sizes, then the export name and the file name.
            1001 | 1009 => {
                let mut p = if code == 1009 { 10 } else { 8 };
                let export = pstr(tag, &mut p);
                let file = pstr(tag, &mut p);
                let name = if file.is_empty() { export } else { file };
                if !name.is_empty() && !out.images.contains(&name) {
                    out.images.push(name);
                }
            }
            _ => {}
        }
        at += len;
    }
    Ok(out)
}

/// A Scaleform movie as a Flash file (`.swf`) for Flash tools: Scaleform's
/// own tags are skipped by them, the rest is SWF. Only the signature differs
/// (`GFX` → `FWS`, `CFX` → `CWS`).
pub fn gfx_to_swf(data: &[u8]) -> Result<Vec<u8>, MediaError> {
    let mut out = data.to_vec();
    match out.get(..3) {
        Some(b"GFX") => out[..3].copy_from_slice(b"FWS"),
        Some(b"CFX") => out[..3].copy_from_slice(b"CWS"),
        Some(b"FWS" | b"CWS") => {}
        _ => return Err(MediaError::Invalid("not a Scaleform movie".into())),
    }
    Ok(out)
}

pub fn is_media(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".gfx") || lower.ends_with(".bik")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swf_signature() {
        assert_eq!(&gfx_to_swf(b"GFX\x08rest").unwrap()[..3], b"FWS");
        assert_eq!(&gfx_to_swf(b"CFX\x08rest").unwrap()[..3], b"CWS");
        assert!(gfx_to_swf(b"BIKi").is_err());
    }

    #[test]
    fn bink_header() {
        let mut d = b"BIKi".to_vec();
        for v in [0x2f9e34u32, 187, 0xc12c, 187, 1024, 576, 2997, 100, 0, 1] {
            d.extend(v.to_le_bytes());
        }
        let b = bink(&d).unwrap();
        assert_eq!((b.width, b.height, b.frames, b.audio_tracks), (1024, 576, 187, 1));
        assert!((b.fps - 29.97).abs() < 0.01);
        assert!((b.duration() - 6.24).abs() < 0.01);
    }

    #[test]
    fn gfx_header_and_exports() {
        // RECT with 16-bit fields: 0..12800 twips × 0..9600 twips (640 × 480).
        let mut body = Vec::new();
        let mut bits: Vec<u8> = Vec::new();
        let mut push = |v: u32, n: u32| (0..n).rev().for_each(|i| bits.push(((v >> i) & 1) as u8));
        push(16, 5);
        for v in [0u32, 12800, 0, 9600] {
            push(v, 16);
        }
        while !bits.len().is_multiple_of(8) {
            bits.push(0);
        }
        for chunk in bits.chunks(8) {
            body.push(chunk.iter().fold(0, |a, b| (a << 1) | b));
        }
        body.extend([0x00, 0x1E, 0x01, 0x00]); // 30 fps, 1 frame
        let export = [&[1u8, 0, 7, 0][..], b"button\0"].concat();
        body.extend((((56u16) << 6) | export.len() as u16).to_le_bytes());
        body.extend(&export);
        body.extend([0, 0]);
        let mut d = b"GFX\x08".to_vec();
        d.extend(((body.len() + 8) as u32).to_le_bytes());
        d.extend(body);
        let g = gfx(&d).unwrap();
        assert_eq!((g.width, g.height, g.fps, g.frames), (640.0, 480.0, 30.0, 1));
        assert_eq!(g.exports, vec!["button"]);
    }
}
