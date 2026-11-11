//! Audio wave containers (`.awc`).
//!
//! Layout (little-endian): `ADAT` magic, version (u16), flags (u16), stream
//! count, data offset; optional chunk indices (flag 1); per stream a word with
//! the id (29 bits) and chunk count (3 bits); then per chunk a 64-bit word
//! with type (8 bits), size (28) and offset (28).
//!
//! * Single sounds: a `format` chunk (samples, sample rate, codec) and a `data`
//!   chunk per stream; flag 2 means the data chunks are encrypted.
//! * Streams (music, radio; flag 4): stream 0 has a `streamformat` chunk with
//!   the channels and a `data` chunk split into blocks; flag 8 means every
//!   block is encrypted.
//! * Some files are encrypted as a whole (no `ADAT` magic in the clear).
//!
//! Encryption is XXTEA with the game's AWC key; the caller passes the
//! decryption function.

const MAGIC: u32 = 0x5441_4441; // "ADAT"

/// Decrypts a buffer in place with the AWC key.
pub type Decrypt<'a> = &'a dyn Fn(&mut [u8]);

/// Per channel of a stream, per block: data, samples to skip, samples.
type StreamBlocks<'a> = Vec<Vec<(&'a [u8], usize, usize)>>;

pub const CHUNK_DATA: u8 = 0x55;
pub const CHUNK_FORMAT: u8 = 0xFA;
pub const CHUNK_STREAM_FORMAT: u8 = 0x48;
pub const CHUNK_SEEK_TABLE: u8 = 0xA3;
pub const CHUNK_MARKERS: u8 = 0xBD;
pub const CHUNK_PEAK: u8 = 0x36;

#[derive(Debug, thiserror::Error)]
pub enum AwcError {
    #[error("not an audio container")]
    NotAwc,
    #[error("the audio is encrypted and the AWC key is not available")]
    NoKey,
    #[error("damaged audio container: {0}")]
    Damaged(String),
}

fn damaged(msg: impl Into<String>) -> AwcError {
    AwcError::Damaged(msg.into())
}

/// Audio codecs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Pcm16,
    Adpcm,
    Other(u8),
}

impl Codec {
    fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Pcm16,
            4 => Self::Adpcm,
            other => Self::Other(other),
        }
    }

    pub fn name(self) -> String {
        match self {
            Self::Pcm16 => "PCM".into(),
            Self::Adpcm => "IMA ADPCM".into(),
            Self::Other(c) => format!("codec {c}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub kind: u8,
    pub offset: usize,
    pub size: usize,
}

/// One sound: a single stream, or one channel of a multi-channel stream.
#[derive(Debug, Clone)]
pub struct Sound {
    /// Name hash (joaat of the sound's name).
    pub id: u32,
    pub samples: u32,
    pub sample_rate: u32,
    pub codec: Codec,
    /// Channel index inside a multi-channel stream.
    pub channel: Option<usize>,
}

impl Sound {
    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 { 0.0 } else { self.samples as f32 / self.sample_rate as f32 }
    }
}

#[derive(Debug, Clone)]
pub struct StreamFormat {
    pub block_count: usize,
    pub block_size: usize,
    pub channels: usize,
}

#[derive(Debug, Clone)]
pub struct Awc {
    /// Decrypted file contents.
    pub data: Vec<u8>,
    pub flags: u16,
    /// Per stream: id and chunks.
    pub streams: Vec<(u32, Vec<Chunk>)>,
    pub sounds: Vec<Sound>,
    pub stream_format: Option<StreamFormat>,
    /// Whether the file needed decryption.
    pub encrypted: bool,
}

fn u16_at(d: &[u8], o: usize) -> Result<u16, AwcError> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| damaged("truncated"))
}

fn u32_at(d: &[u8], o: usize) -> Result<u32, AwcError> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| damaged("truncated"))
}

fn u64_at(d: &[u8], o: usize) -> Result<u64, AwcError> {
    d.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| damaged("truncated"))
}

impl Awc {
    /// Parses a container. `decrypt` decrypts a buffer with the AWC key
    /// (`None`: no key available).
    pub fn parse(mut data: Vec<u8>, decrypt: Option<Decrypt>) -> Result<Self, AwcError> {
        let mut encrypted = false;
        if u32_at(&data, 0)? != MAGIC {
            let decrypt = decrypt.ok_or(AwcError::NoKey)?;
            decrypt(&mut data);
            if u32_at(&data, 0)? != MAGIC {
                return Err(AwcError::NotAwc);
            }
            encrypted = true;
        }
        let flags = u16_at(&data, 6)?;
        let count = u32_at(&data, 8)? as usize;
        if count > 1 << 16 {
            return Err(damaged("too many streams"));
        }
        let mut o = 16;
        if flags & 1 != 0 {
            o += 2 * count;
        }
        let mut infos = Vec::with_capacity(count);
        for _ in 0..count {
            let v = u32_at(&data, o)?;
            infos.push((v & 0x1FFF_FFFF, (v >> 29) as usize));
            o += 4;
        }
        let mut streams = Vec::with_capacity(count);
        for (id, chunk_count) in infos {
            let mut chunks = Vec::with_capacity(chunk_count);
            for _ in 0..chunk_count {
                let v = u64_at(&data, o)?;
                o += 8;
                let chunk = Chunk {
                    kind: (v >> 56) as u8,
                    size: ((v >> 28) & 0x0FFF_FFFF) as usize,
                    offset: (v & 0x0FFF_FFFF) as usize,
                };
                if chunk.offset + chunk.size > data.len() {
                    return Err(damaged("chunk outside the file"));
                }
                chunks.push(chunk);
            }
            streams.push((id, chunks));
        }

        let mut awc = Awc { data, flags, streams, sounds: Vec::new(), stream_format: None, encrypted };
        if flags & 4 != 0 {
            awc.read_stream_format()?;
        } else {
            awc.read_formats()?;
            if flags & 2 != 0 {
                let decrypt = decrypt.ok_or(AwcError::NoKey)?;
                for (_, chunks) in &awc.streams {
                    for c in chunks.iter().filter(|c| c.kind == CHUNK_DATA) {
                        decrypt(&mut awc.data[c.offset..c.offset + c.size]);
                    }
                }
            }
        }
        if flags & 4 != 0 && flags & 8 != 0 {
            let decrypt = decrypt.ok_or(AwcError::NoKey)?;
            let (Some(format), Some(chunk)) = (&awc.stream_format, awc.chunk(0, CHUNK_DATA).cloned()) else {
                return Err(damaged("stream without data"));
            };
            let mut start = chunk.offset;
            let end = chunk.offset + chunk.size;
            while start < end {
                let len = format.block_size.min(end - start);
                decrypt(&mut awc.data[start..start + len]);
                start += len;
            }
        }
        Ok(awc)
    }

    pub fn chunk(&self, stream: usize, kind: u8) -> Option<&Chunk> {
        self.streams.get(stream)?.1.iter().find(|c| c.kind == kind)
    }

    fn read_formats(&mut self) -> Result<(), AwcError> {
        for (i, (id, _)) in self.streams.iter().enumerate() {
            let Some(f) = self.chunk(i, CHUNK_FORMAT) else { continue };
            let d = &self.data[f.offset..f.offset + f.size];
            if d.len() < 20 {
                return Err(damaged("short format chunk"));
            }
            self.sounds.push(Sound {
                id: *id,
                samples: u32_at(d, 0)?,
                sample_rate: u16_at(d, 8)? as u32,
                codec: Codec::from_byte(d[19]),
                channel: None,
            });
        }
        Ok(())
    }

    fn read_stream_format(&mut self) -> Result<(), AwcError> {
        let Some(f) = self.chunk(0, CHUNK_STREAM_FORMAT).cloned() else {
            return Err(damaged("stream without format"));
        };
        let d = &self.data[f.offset..f.offset + f.size];
        let block_count = u32_at(d, 0)? as usize;
        let block_size = u32_at(d, 4)? as usize;
        let channels = u32_at(d, 8)? as usize;
        if channels > 64 || block_size == 0 {
            return Err(damaged("bad stream format"));
        }
        for c in 0..channels {
            let o = 12 + c * 16;
            self.sounds.push(Sound {
                id: u32_at(d, o)?,
                samples: u32_at(d, o + 4)?,
                sample_rate: u16_at(d, o + 10)? as u32,
                codec: Codec::from_byte(*d.get(o + 12).ok_or_else(|| damaged("truncated"))?),
                channel: Some(c),
            });
        }
        self.stream_format = Some(StreamFormat { block_count, block_size, channels });
        Ok(())
    }
}

/// Decoded audio: 16-bit samples, interleaved if there are several channels.
#[derive(Debug, Clone)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn duration_secs(&self) -> f32 {
        let frames = self.samples.len() / self.channels.max(1) as usize;
        if self.sample_rate == 0 { 0.0 } else { frames as f32 / self.sample_rate as f32 }
    }

    /// A `.wav` file (16-bit PCM).
    pub fn to_wav(&self) -> Vec<u8> {
        let data_len = (self.samples.len() * 2) as u32;
        let block_align = self.channels * 2;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&self.channels.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&(self.sample_rate * block_align as u32).to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

const IMA_INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];
const IMA_STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];

/// IMA ADPCM in 2048-byte blocks: step index (byte 0), predictor (i16 at 2),
/// then 4-bit samples, low nibble first.
pub fn decode_adpcm(data: &[u8], samples: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(samples);
    for block in data.chunks(2048) {
        if block.len() < 4 || out.len() >= samples {
            break;
        }
        let mut index = (block[0] as i32).clamp(0, 88);
        let mut predictor = i16::from_le_bytes([block[2], block[3]]) as i32;
        for &byte in &block[4..] {
            for nibble in [byte & 0xF, byte >> 4] {
                if out.len() >= samples {
                    return out;
                }
                let step = IMA_STEP[index as usize];
                let mut diff = step >> 3;
                if nibble & 1 != 0 {
                    diff += step >> 2;
                }
                if nibble & 2 != 0 {
                    diff += step >> 1;
                }
                if nibble & 4 != 0 {
                    diff += step;
                }
                if nibble & 8 != 0 {
                    diff = -diff;
                }
                predictor = (predictor + diff).clamp(-32768, 32767);
                index = (index + IMA_INDEX[nibble as usize]).clamp(0, 88);
                out.push(predictor as i16);
            }
        }
    }
    out
}

/// Decodes MPEG audio (the Enhanced edition's codec) to mono samples.
pub fn decode_mp3(data: &[u8]) -> Result<Vec<i16>, AwcError> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    match decode_mpeg_frames(data) {
        Ok(pcm) => Ok(pcm),
        // Recognising MPEG audio needs two frames in a row; a block may hold
        // only one. Decode it twice and keep the second copy.
        Err(_) if data.len() < 8192 => {
            let doubled = [data, data].concat();
            let pcm = decode_mpeg_frames(&doubled)?;
            Ok(pcm[pcm.len() / 2..].to_vec())
        }
        Err(e) => Err(e),
    }
}

fn decode_mpeg_frames(data: &[u8]) -> Result<Vec<i16>, AwcError> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let err = |e: symphonia::core::errors::Error| damaged(format!("MP3: {e}"));
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(data.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(err)?;
    let params = format
        .default_track(TrackType::Audio)
        .and_then(|t| t.codec_params.as_ref())
        .and_then(|p| p.audio())
        .cloned()
        .ok_or_else(|| damaged("MP3 without audio"))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &{
            let mut opts = AudioDecoderOptions::default();
            opts.gapless = false;
            opts
        })
        .map_err(err)?;
    let mut out = Vec::new();
    let mut frame: Vec<i16> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // Padding after the last frame ends the stream.
            Err(_) => break,
        };
        let Ok(buf) = decoder.decode(&packet) else { continue };
        let channels = buf.spec().channels().count().max(1);
        frame.clear();
        buf.copy_to_vec_interleaved(&mut frame);
        // Rockstar's streams are mono per channel; mix down just in case.
        out.extend(frame.chunks(channels).map(|c| (c.iter().map(|&s| s as i32).sum::<i32>() / channels as i32) as i16));
    }
    Ok(out)
}

fn decode_codec(codec: Codec, data: &[u8], samples: usize) -> Result<Vec<i16>, AwcError> {
    let mut out = match codec {
        Codec::Pcm16 => data.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect(),
        Codec::Adpcm => decode_adpcm(data, samples),
        Codec::Other(7) => decode_mp3(data)?,
        Codec::Other(c) => return Err(damaged(format!("unsupported codec {c}"))),
    };
    if samples > 0 && out.len() > samples {
        out.truncate(samples);
    }
    Ok(out)
}

impl Awc {
    /// Decodes a single sound (index into [`Awc::sounds`]) of a container
    /// without a multi-channel stream.
    pub fn decode_sound(&self, index: usize) -> Result<Pcm, AwcError> {
        let sound = self.sounds.get(index).ok_or_else(|| damaged("no such sound"))?;
        if self.stream_format.is_some() {
            return self.decode_stream();
        }
        let stream = self.streams.iter().position(|(id, _)| *id == sound.id).ok_or_else(|| damaged("no stream"))?;
        let chunk = self.chunk(stream, CHUNK_DATA).ok_or_else(|| damaged("sound without data"))?;
        let data = &self.data[chunk.offset..chunk.offset + chunk.size];
        let samples = decode_codec(sound.codec, data, sound.samples as usize)?;
        Ok(Pcm { sample_rate: sound.sample_rate, channels: 1, samples })
    }

    /// The data of each channel of a multi-channel stream: per block the bytes
    /// and the number of samples the block contributes.
    fn stream_channels(&self) -> Result<StreamBlocks<'_>, AwcError> {
        let format = self.stream_format.as_ref().ok_or_else(|| damaged("not a stream"))?;
        let chunk = self.chunk(0, CHUNK_DATA).ok_or_else(|| damaged("stream without data"))?;
        let mut channels = vec![Vec::new(); format.channels];
        let end = chunk.offset + chunk.size;
        let mut block = chunk.offset;
        while block < end {
            let block_end = (block + format.block_size).min(end);
            let d = &self.data[block..block_end];
            // Per channel (24 bytes): start, seek entries, samples to skip at the
            // start, samples, MPEG frames, data bytes. Then a seek table per channel, then the
            // channels' data from the next 2048-byte boundary.
            let mut infos = Vec::with_capacity(format.channels);
            for c in 0..format.channels {
                let o = c * 24;
                infos.push((
                    u32_at(d, o + 4)? as usize,
                    u32_at(d, o + 8)? as usize,
                    u32_at(d, o + 12)? as usize,
                    u32_at(d, o + 20)? as usize,
                ));
            }
            let mut pos = format.channels * 24 + infos.iter().map(|(entries, ..)| entries * 4).sum::<usize>();
            pos = pos.div_ceil(0x800) * 0x800;
            for (c, (entries, skip, samples, bytes)) in infos.into_iter().enumerate() {
                // MPEG channels give their size in bytes and follow each other
                // (16-byte aligned); PCM/ADPCM channels fill 2048-byte units.
                let mpeg = self.sounds.get(c).is_some_and(|s| matches!(s.codec, Codec::Other(_)));
                let (len, next) = if bytes > 0 {
                    (bytes, bytes.div_ceil(16) * 16)
                } else if mpeg {
                    // An MPEG channel without data in this block.
                    (0, 0)
                } else {
                    (entries * 2048, entries * 2048)
                };
                // The last block of a file may stop right after the used bytes.
                let region =
                    d.get(pos..(pos + len).min(d.len())).ok_or_else(|| damaged("block data outside the block"))?;
                channels[c].push((region, skip, samples));
                pos += next;
            }
            block = block_end;
        }
        Ok(channels)
    }

    /// Decodes a multi-channel stream to one interleaved signal.
    pub fn decode_stream(&self) -> Result<Pcm, AwcError> {
        let channels = self.stream_channels()?;
        let mut decoded = Vec::with_capacity(channels.len());
        for (blocks, sound) in channels.iter().zip(&self.sounds) {
            let mut out = Vec::with_capacity(sound.samples as usize);
            for (b, (data, skip, samples)) in blocks.iter().enumerate() {
                let mut pcm = decode_codec(sound.codec, data, 0).map_err(|e| {
                    let head: Vec<String> = data.iter().take(8).map(|x| format!("{x:02x}")).collect();
                    damaged(format!(
                        "channel {} block {b} ({} bytes: {}): {e}",
                        sound.channel.unwrap_or(0),
                        data.len(),
                        head.join(" ")
                    ))
                })?;
                // A block decodes to `samples`; MPEG decoders may add a frame
                // (info header) in front. The first `skip` samples overlap the
                // previous block.
                if pcm.len() > *samples {
                    pcm.drain(..pcm.len() - samples);
                }
                pcm.truncate(*samples);
                out.extend_from_slice(pcm.get((*skip).min(pcm.len())..).unwrap_or_default());
            }
            out.truncate(sound.samples as usize);
            decoded.push(out);
        }
        let frames = decoded.iter().map(Vec::len).max().unwrap_or(0);
        let mut samples = Vec::with_capacity(frames * decoded.len());
        for i in 0..frames {
            for ch in &decoded {
                samples.push(ch.get(i).copied().unwrap_or(0));
            }
        }
        let sample_rate = self.sounds.first().map_or(44100, |s| s.sample_rate);
        Ok(Pcm { sample_rate, channels: decoded.len().max(1) as u16, samples })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One PCM sound: header, stream info, two chunk infos, format, data.
    fn synthetic() -> Vec<u8> {
        let samples: [i16; 4] = [0, 1000, -1000, 32767];
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // version
        out.extend_from_slice(&0xFF00u16.to_le_bytes()); // flags
        out.extend_from_slice(&1u32.to_le_bytes()); // streams
        out.extend_from_slice(&0u32.to_le_bytes()); // data offset (unused here)
        out.extend_from_slice(&(0x1234u32 | (2 << 29)).to_le_bytes());
        let format_at = 16 + 4 + 16;
        let data_at = format_at + 24;
        let chunk =
            |kind: u8, size: usize, offset: usize| ((kind as u64) << 56) | ((size as u64) << 28) | offset as u64;
        out.extend_from_slice(&chunk(CHUNK_FORMAT, 24, format_at).to_le_bytes());
        out.extend_from_slice(&chunk(CHUNK_DATA, data.len(), data_at).to_le_bytes());
        let mut format = vec![0u8; 24];
        format[0..4].copy_from_slice(&4u32.to_le_bytes());
        format[8..10].copy_from_slice(&22050u16.to_le_bytes());
        format[19] = 0; // PCM
        out.extend_from_slice(&format);
        out.extend_from_slice(&data);
        out
    }

    #[test]
    fn parses_and_decodes_pcm() {
        let awc = Awc::parse(synthetic(), None).unwrap();
        assert_eq!(awc.sounds.len(), 1);
        assert_eq!(awc.sounds[0].id, 0x1234);
        assert_eq!(awc.sounds[0].sample_rate, 22050);
        let pcm = awc.decode_sound(0).unwrap();
        assert_eq!(pcm.samples, vec![0, 1000, -1000, 32767]);
        let wav = pcm.to_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 8);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 22050);
    }

    #[test]
    fn encrypted_without_key() {
        let mut data = synthetic();
        data[0] ^= 0xFF;
        assert!(matches!(Awc::parse(data, None), Err(AwcError::NoKey)));
    }

    #[test]
    fn adpcm_block() {
        // Step index 0, predictor 0: nibble 7 adds 11 and raises the step
        // index by 8 (step 16), so nibble 15 then subtracts 30.
        let block = [0u8, 0, 0, 0, 0xF7];
        assert_eq!(decode_adpcm(&block, 2), vec![11, -19]);
    }
}
