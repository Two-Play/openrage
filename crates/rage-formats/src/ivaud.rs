//! GTA IV audio (the entries of the RPF3 audio archives, `pc/audio/sfx/*.rpf`):
//! streamed sounds (music, cutscenes) decoded to PCM.
//!
//! Layout of a streamed file (little endian on the PC):
//!
//! * `0x00` u64: block table offset; `0x08` u32: block count; `0x0C` u32:
//!   block size; `0x10` u32: 0 (a bank of sounds has its stream count there);
//!   `0x14` u64: channel table offset; `0x24` u32: channel count; `0x2C` u32:
//!   where the blocks start.
//! * Channel table: per channel 16 bytes (offset into the channel infos, name
//!   hash, size); the channel infos follow it: `0x08` name hash, `0x10` u32
//!   sample count, `0x18` u16 sample rate, `0x1C` u32 codec (`0x400`: IMA
//!   ADPCM, 4 bits per sample; `0x1`: 16-bit PCM).
//! * Each block: `0x00` u64 offset of its per-channel table, `0x08` u64 the
//!   table's end; per channel 16 bytes: `0x00` u32 where the channel's data
//!   starts in 2 KiB units after the block's header, `0x0C` u32 its sample
//!   count in this block. The header is 2 KiB (4 KiB with more than three
//!   channels). The ADPCM state runs on from block to block.
//!
//! The layout follows vgmstream's `rage_aud` reader and SparkIV.

use crate::awc::Pcm;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IvaudError {
    #[error("{0}")]
    Invalid(String),
}

fn err(msg: impl Into<String>) -> IvaudError {
    IvaudError::Invalid(msg.into())
}

fn u32_at(d: &[u8], o: usize) -> Result<u32, IvaudError> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| err(format!("truncated at {o:#x}")))
}

fn u64_at(d: &[u8], o: usize) -> Result<usize, IvaudError> {
    Ok(u32_at(d, o)? as usize | (u32_at(d, o + 4)? as usize) << 32)
}

/// Facts about a streamed sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub channels: usize,
    pub sample_rate: u32,
    /// Samples per channel.
    pub samples: usize,
    pub codec: u32,
    /// Channel name hashes.
    pub hashes: Vec<u32>,
}

const IMA: u32 = 0x400;
const PCM16: u32 = 0x1;

/// Whether `data` is a streamed sound (rather than a bank of sounds).
pub fn is_stream(data: &[u8]) -> bool {
    info(data).is_ok()
}

/// Reads a streamed sound's header.
pub fn info(d: &[u8]) -> Result<StreamInfo, IvaudError> {
    let channels = check_stream_header(d)?;
    let info = read_channel_table(d, channels)?;
    if info.sample_rate == 0 {
        return Err(err("no sample rate"));
    }
    Ok(info)
}

fn check_is_stream(d: &[u8]) -> Result<(), IvaudError> {
    let block_table = u64_at(d, 0x00)?;
    if !(0x1C..=0x24000).contains(&block_table) {
        return Err(err("not a GTA IV sound"));
    }
    if u32_at(d, 0x10)? != 0 {
        return Err(err("a bank of sounds, not a stream"));
    }
    Ok(())
}

/// Checks that `d` is a stream whose blocks fill it; returns its channel count.
fn check_stream_header(d: &[u8]) -> Result<usize, IvaudError> {
    check_is_stream(d)?;
    let (blocks, block_size, start) = (u32_at(d, 0x08)? as usize, u32_at(d, 0x0C)? as usize, u32_at(d, 0x2C)? as usize);
    let channels = u32_at(d, 0x24)? as usize;
    if channels == 0 || channels > 16 || block_size == 0 {
        return Err(err(format!("{channels} channels in blocks of {block_size} bytes")));
    }
    check_fill(d, blocks, block_size, start)?;
    Ok(channels)
}

fn check_fill(d: &[u8], blocks: usize, block_size: usize, start: usize) -> Result<(), IvaudError> {
    let expected = blocks * block_size + start;
    if expected != d.len() && expected + block_size != d.len() {
        return Err(err(format!("{blocks} blocks of {block_size} bytes do not fill {} bytes", d.len())));
    }
    Ok(())
}

/// The channel hashes, and the format of the first channel.
fn read_channel_table(d: &[u8], channels: usize) -> Result<StreamInfo, IvaudError> {
    let table = u64_at(d, 0x14)?;
    let infos = table + channels * 0x10;
    let mut hashes = Vec::with_capacity(channels);
    let (mut samples, mut sample_rate, mut codec) = (0, 0, 0);
    for c in 0..channels {
        hashes.push(u32_at(d, table + c * 0x10 + 0x08)?);
        let at = infos + u64_at(d, table + c * 0x10)?;
        if c == 0 {
            samples = u32_at(d, at + 0x10)? as usize;
            sample_rate = u32_at(d, at + 0x18)? & 0xFFFF;
            codec = u32_at(d, at + 0x1C)?;
        }
    }
    Ok(StreamInfo { channels, sample_rate, samples, codec, hashes })
}

const STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];
const INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// IMA ADPCM state of one channel.
#[derive(Default, Clone, Copy)]
struct Ima {
    predictor: i32,
    index: i32,
}

impl Ima {
    fn decode(&mut self, nibble: u8) -> i16 {
        let step = STEPS[self.index as usize];
        let mut diff = step >> 3;
        if nibble & 4 != 0 {
            diff += step;
        }
        if nibble & 2 != 0 {
            diff += step >> 1;
        }
        if nibble & 1 != 0 {
            diff += step >> 2;
        }
        self.predictor =
            if nibble & 8 != 0 { self.predictor - diff } else { self.predictor + diff }.clamp(-32768, 32767);
        self.index = (self.index + INDEX[(nibble & 7) as usize]).clamp(0, 88);
        self.predictor as i16
    }
}

/// What a block's channels are read with.
struct Blocks<'a> {
    d: &'a [u8],
    codec: u32,
    header: usize,
}

impl Blocks<'_> {
    /// Appends the samples of every channel of the block at `block`; `false` for a block that ends the stream.
    fn append(&self, block: usize, channels: &mut [Vec<i16>], states: &mut [Ima]) -> Result<bool, IvaudError> {
        let table = block + u64_at(self.d, block)?;
        let end = block + u64_at(self.d, block + 0x08)?;
        if end <= table || end > block + self.header {
            return Ok(false);
        }
        let entry = (end - table) / channels.len();
        for (c, (out, state)) in channels.iter_mut().zip(states).enumerate() {
            self.append_channel(block, table + c * entry, c, out, state)?;
        }
        Ok(true)
    }

    /// Appends the samples of the channel whose table entry is at `e`.
    fn append_channel(
        &self,
        block: usize,
        e: usize,
        c: usize,
        out: &mut Vec<i16>,
        state: &mut Ima,
    ) -> Result<(), IvaudError> {
        let d = self.d;
        let at = block + self.header + u32_at(d, e)? as usize * 0x800;
        let count = u32_at(d, e + 0x0C)? as usize;
        let bytes = if self.codec == IMA { count.div_ceil(2) } else { count * 2 };
        let Some(data) = d.get(at..at + bytes) else { return Err(err(format!("channel {c} runs past the file"))) };
        if self.codec == IMA {
            append_ima(out, state, data, count);
        } else {
            out.extend(data.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes(*s)));
        }
        Ok(())
    }
}

fn append_ima(out: &mut Vec<i16>, state: &mut Ima, data: &[u8], count: usize) {
    for (i, b) in data.iter().enumerate() {
        out.push(state.decode(b & 0x0F));
        if 2 * i + 1 < count {
            out.push(state.decode(b >> 4));
        }
    }
}

/// Decodes a streamed sound: all channels, interleaved.
pub fn decode(d: &[u8]) -> Result<Pcm, IvaudError> {
    let info = info(d)?;
    if info.codec != IMA && info.codec != PCM16 {
        return Err(err(format!("codec {:#x} (only IMA ADPCM and PCM are read)", info.codec)));
    }
    let (block_size, start) = (u32_at(d, 0x0C)? as usize, u32_at(d, 0x2C)? as usize);
    let header = if info.channels > 3 { 0x1000 } else { 0x800 };
    let blocks = Blocks { d, codec: info.codec, header };
    let mut channels: Vec<Vec<i16>> = vec![Vec::with_capacity(info.samples); info.channels];
    let mut states = vec![Ima::default(); info.channels];
    let mut block = start;
    while block + header <= d.len() && blocks.append(block, &mut channels, &mut states)? {
        block += block_size;
    }
    let samples = interleave(&channels, info.samples);
    Ok(Pcm { sample_rate: info.sample_rate, channels: info.channels as u16, samples })
}

/// The channels' samples frame by frame, a short channel padded with silence; at most `limit` frames.
fn interleave(channels: &[Vec<i16>], limit: usize) -> Vec<i16> {
    let frames = channels.iter().map(Vec::len).max().unwrap_or(0).min(limit.max(1));
    let mut samples = Vec::with_capacity(frames * channels.len());
    for f in 0..frames {
        samples.extend(channels.iter().map(|c| c.get(f).copied().unwrap_or(0)));
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream of two channels in one block (header 0x30 + table, blocks at
    /// 0x100, 0x1800 bytes each): channel 0 holds 4 samples, channel 1 three.
    fn stream() -> Vec<u8> {
        let mut d = vec![0u8; 0x100 + 0x1800];
        let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0x00, 0x30); // block table
        put(&mut d, 0x08, 1);
        put(&mut d, 0x0C, 0x1800);
        put(&mut d, 0x14, 0x30); // channel table
        put(&mut d, 0x24, 2);
        put(&mut d, 0x2C, 0x100);
        for c in 0..2 {
            put(&mut d, 0x30 + c * 0x10, (c * 0x20) as u32);
            put(&mut d, 0x30 + c * 0x10 + 0x08, 0xAB00 + c as u32);
            let info = 0x50 + c * 0x20;
            put(&mut d, info + 0x10, 4);
            put(&mut d, info + 0x18, 32000);
            put(&mut d, info + 0x1C, IMA);
        }
        let block = 0x100;
        put(&mut d, block, 0x18);
        put(&mut d, block + 0x08, 0x18 + 2 * 0x10);
        // Channel 0 at unit 0, channel 1 at unit 1.
        put(&mut d, block + 0x18, 0);
        put(&mut d, block + 0x18 + 0x0C, 4);
        put(&mut d, block + 0x28, 1);
        put(&mut d, block + 0x28 + 0x0C, 3);
        d[block + 0x800] = 0x77; // +7, +7
        d[block + 0x801] = 0x0F; // -7, then 0
        d
    }

    #[test]
    fn reads_the_header() {
        let i = info(&stream()).unwrap();
        assert_eq!((i.channels, i.sample_rate, i.samples, i.codec), (2, 32000, 4, IMA));
        assert_eq!(i.hashes, [0xAB00, 0xAB01]);
        assert!(!is_stream(&[0u8; 64]));
    }

    #[test]
    fn decodes_ima() {
        let mut s = Ima::default();
        // Step 7: nibble 7 adds 7 + 3 + 1 + 0 = 11 and moves the index up by 8.
        assert_eq!(s.decode(7), 11);
        assert_eq!(s.index, 8);
        assert_eq!(s.decode(0xF), 11 - (16 + 8 + 4 + 2));
        let pcm = decode(&stream()).unwrap();
        assert_eq!((pcm.channels, pcm.sample_rate), (2, 32000));
        assert_eq!(pcm.samples.len(), 8, "4 frames of two channels");
        // Channel 0's first sample, low nibble first; channel 1 is silent.
        assert_eq!(&pcm.samples[..2], &[11, 0]);
        assert_eq!(pcm.samples[7], 0, "channel 1 has three samples; the fourth frame is padded");
    }
}
