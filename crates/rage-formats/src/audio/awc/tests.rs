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
    let chunk = |kind: u8, size: usize, offset: usize| ((kind as u64) << 56) | ((size as u64) << 28) | offset as u64;
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

/// The same sound in RDR2's layout: a 32-byte `0x76` format chunk
/// (codec at 28) and the encryption flag bit that RDR2 uses otherwise.
#[test]
fn parses_rdr2_format_chunk() {
    let samples: [i16; 3] = [5, -5, 300];
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0xFF02u16.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(0x77u32 | (2 << 29)).to_le_bytes());
    let format_at = 16 + 4 + 16;
    let data_at = format_at + 32;
    let chunk = |kind: u8, size: usize, offset: usize| ((kind as u64) << 56) | ((size as u64) << 28) | offset as u64;
    out.extend_from_slice(&chunk(CHUNK_FORMAT_RDR2, 32, format_at).to_le_bytes());
    out.extend_from_slice(&chunk(CHUNK_DATA, data.len(), data_at).to_le_bytes());
    let mut format = vec![0u8; 32];
    format[0..4].copy_from_slice(&3u32.to_le_bytes());
    format[8..10].copy_from_slice(&32000u16.to_le_bytes());
    format[28] = 0; // PCM; 8 would be Vorbis with headers after byte 32
    out.extend_from_slice(&format);
    out.extend_from_slice(&data);
    let awc = Awc::parse(out, None).unwrap();
    assert_eq!(awc.sounds[0].sample_rate, 32000);
    assert_eq!(awc.sounds[0].codec, Codec::Pcm16);
    assert_eq!(awc.decode_sound(0).unwrap().samples, vec![5, -5, 300]);
}

/// GTA VI: the three optional tables in front of the streams, and a
/// stereo Opus sound of two packets without content (silence).
#[test]
fn parses_gta6_opus() {
    let mut unit = vec![0u8; 2048];
    // Two packets of one byte: 20 ms of full-band stereo, no data.
    unit[..6].copy_from_slice(&[1, 0, 0xFC, 1, 0, 0xFC]);
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0xFF61u16.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    let tables = 2 + 8 + 4;
    let format_at = 16 + tables + 4 + 16;
    out.extend_from_slice(&(format_at as u32).to_le_bytes());
    out.extend_from_slice(&[0xEE; 14]);
    out.extend_from_slice(&(0x1234u32 | (2 << 29)).to_le_bytes());
    let chunk = |kind: u8, size: usize, offset: usize| ((kind as u64) << 56) | ((size as u64) << 28) | offset as u64;
    out.extend_from_slice(&chunk(CHUNK_FORMAT_RDR2, 32, format_at).to_le_bytes());
    out.extend_from_slice(&chunk(CHUNK_DATA, unit.len(), format_at + 32).to_le_bytes());
    let frames = 2 * 960 - OPUS_PRE_SKIP as u32;
    let mut format = vec![0u8; 32];
    format[0..4].copy_from_slice(&(frames * 2).to_le_bytes());
    format[8..10].copy_from_slice(&32000u16.to_le_bytes());
    format[28] = 11;
    format[29] = 2;
    out.extend_from_slice(&format);
    out.extend_from_slice(&unit);

    let awc = Awc::parse(out, None).unwrap();
    let sound = &awc.sounds[0];
    assert_eq!((sound.codec, sound.channels, sound.samples, sound.sample_rate), (Codec::Opus, 2, frames, 48000));
    let pcm = awc.decode_sound(0).unwrap();
    assert_eq!((pcm.channels, pcm.sample_rate, pcm.samples.len()), (2, 48000, frames as usize * 2));
    assert!(pcm.samples.iter().all(|s| *s == 0));
}

#[test]
fn opus_packet_lengths() {
    // CELT 20 ms, one frame; SILK 60 ms, two frames; hybrid 10 ms, three frames, stereo.
    assert_eq!(opus_packet_samples(&[0xF8]), 960);
    assert_eq!(opus_packet_samples(&[(3 << 3) | 1, 0]), 2 * 2880);
    assert_eq!(opus_packet_samples(&[(12 << 3) | 4 | 3, 3]), 3 * 480);
    assert_eq!(opus_packet_channels(&[(12 << 3) | 4 | 3, 3]), 2);
    assert_eq!(opus_packet_samples(&[]), 0);
    let mut units = vec![0u8; 4096];
    units[..5].copy_from_slice(&[3, 0, 1, 2, 3]);
    units[2048..2052].copy_from_slice(&[2, 0, 9, 9]);
    assert_eq!(opus_packets(&units).collect::<Vec<_>>(), [&[1u8, 2, 3][..], &[9, 9]]);
}

#[test]
fn vorbis_header_packets() {
    let mut d = Vec::new();
    for p in [&b"\x01vorbis"[..], b"\x03vorbis", b"\x05vorbis!"] {
        d.extend_from_slice(&(p.len() as u32).to_le_bytes());
        d.extend_from_slice(p);
    }
    let [a, b, c] = vorbis_headers(&d).unwrap();
    assert_eq!((a.len(), b.len(), c.len()), (7, 7, 8));
    assert!(vorbis_headers(&d[..10]).is_err());
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

fn word(kind: u8, size: usize, offset: usize) -> u64 {
    ((kind as u64) << 56) | ((size as u64) << 28) | offset as u64
}

type Chunks = Vec<(u8, Vec<u8>)>;

/// A container with the given streams (id, chunks with payloads); chunk payloads follow the tables.
fn build(flags: u16, streams: &[(u32, Chunks)]) -> Vec<u8> {
    let count = streams.len();
    let chunk_total: usize = streams.iter().map(|(_, c)| c.len()).sum();
    let head = 16 + if flags & 1 != 0 { 2 * count } else { 0 } + count * 4 + chunk_total * 8;
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(count as u32).to_le_bytes());
    out.extend_from_slice(&(head as u32).to_le_bytes());
    if flags & 1 != 0 {
        out.extend(vec![0u8; 2 * count]);
    }
    for (id, chunks) in streams {
        out.extend_from_slice(&(id | (chunks.len() as u32) << 29).to_le_bytes());
    }
    let mut at = head;
    for (_, chunks) in streams {
        for (kind, payload) in chunks {
            out.extend_from_slice(&word(*kind, payload.len(), at).to_le_bytes());
            at += payload.len();
        }
    }
    for (_, chunks) in streams {
        for (_, payload) in chunks {
            out.extend_from_slice(payload);
        }
    }
    out
}

fn format_chunk(samples: u32, rate: u16, codec: u8) -> Vec<u8> {
    let mut f = vec![0u8; 24];
    f[0..4].copy_from_slice(&samples.to_le_bytes());
    f[8..10].copy_from_slice(&rate.to_le_bytes());
    f[19] = codec;
    f
}

fn rdr2_format(samples: u32, rate: u16, codec: u8, channels: u8, headers: &[u8]) -> Vec<u8> {
    let mut f = vec![0u8; 32];
    f[0..4].copy_from_slice(&samples.to_le_bytes());
    f[8..10].copy_from_slice(&rate.to_le_bytes());
    f[28] = codec;
    f[29] = channels;
    f[30..32].copy_from_slice(&(headers.len() as u16).to_le_bytes());
    f.extend_from_slice(headers);
    f
}

fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn pcm_sound(flags: u16, data: Vec<u8>) -> Vec<u8> {
    build(flags, &[(7, vec![(CHUNK_FORMAT, format_chunk(4, 8000, 0)), (CHUNK_DATA, data)])])
}

fn xor(d: &mut [u8]) {
    d.iter_mut().for_each(|b| *b ^= 0xFF);
}

fn message(e: AwcError) -> String {
    e.to_string()
}

#[test]
fn codecs_have_names() {
    assert_eq!(Codec::from_byte(0), Codec::Pcm16);
    assert_eq!(Codec::from_byte(4), Codec::Adpcm);
    assert_eq!(Codec::from_byte(8), Codec::Vorbis);
    assert_eq!(Codec::from_byte(11), Codec::Opus);
    assert_eq!(Codec::from_byte(7), Codec::Other(7));
    let names: Vec<String> = [0, 4, 8, 11, 7].iter().map(|&b| Codec::from_byte(b).name()).collect();
    assert_eq!(names, ["PCM", "IMA ADPCM", "Vorbis", "Opus", "codec 7"]);
}

#[test]
fn durations_follow_the_sample_rate() {
    let sound = |samples, sample_rate| Sound {
        id: 0,
        samples,
        sample_rate,
        codec: Codec::Pcm16,
        channel: None,
        headers: None,
        channels: 1,
    };
    assert_eq!(sound(22050, 44100).duration_secs(), 0.5);
    assert_eq!(sound(100, 0).duration_secs(), 0.0);
    let pcm = Pcm { sample_rate: 8000, channels: 2, samples: vec![0; 16000] };
    assert_eq!(pcm.duration_secs(), 1.0);
    assert_eq!(Pcm { sample_rate: 0, channels: 1, samples: vec![0; 4] }.duration_secs(), 0.0);
    assert_eq!(Pcm { sample_rate: 4, channels: 0, samples: vec![0; 4] }.duration_secs(), 1.0);
}

#[test]
fn a_stereo_wav_has_its_fields() {
    let wav = Pcm { sample_rate: 8000, channels: 2, samples: vec![1, -1, 2, -2] }.to_wav();
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 8);
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 8000);
    assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32000);
    assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 4);
    assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
    assert_eq!(&wav[44..], pcm_bytes(&[1, -1, 2, -2]));
}

#[test]
fn a_file_too_short_for_a_magic_is_damaged() {
    assert_eq!(message(Awc::parse(vec![1, 2], None).unwrap_err()), "damaged audio container: truncated");
    assert_eq!(message(Awc::parse(Vec::new(), None).unwrap_err()), "damaged audio container: truncated");
    let mut data = pcm_sound(0, pcm_bytes(&[1, 2, 3, 4]));
    data.truncate(10);
    assert_eq!(message(Awc::parse(data, None).unwrap_err()), "damaged audio container: truncated");
}

#[test]
fn a_file_that_stays_unreadable_after_decrypting_is_not_audio() {
    let mut data = pcm_sound(0, pcm_bytes(&[1, 2, 3, 4]));
    data[0] ^= 0xFF;
    assert!(matches!(Awc::parse(data.clone(), Some(&|_: &mut [u8]| {})), Err(AwcError::NotAwc)));
    assert_eq!(message(AwcError::NotAwc), "not an audio container");
    assert_eq!(message(AwcError::NoKey), "the audio is encrypted and the AWC key is not available");
    assert!(matches!(Awc::parse_rdr2(data), Err(AwcError::NoKey)));
}

#[test]
fn a_file_encrypted_as_a_whole_is_decrypted() {
    let mut data = pcm_sound(0, pcm_bytes(&[5, 6, 7, 8]));
    xor(&mut data);
    let awc = Awc::parse(data, Some(&xor)).unwrap();
    assert!(awc.encrypted);
    assert_eq!(awc.decode_sound(0).unwrap().samples, [5, 6, 7, 8]);
    assert!(!Awc::parse(pcm_sound(0, pcm_bytes(&[1])), None).unwrap().encrypted);
}

#[test]
fn too_many_streams_are_refused() {
    let mut data = pcm_sound(0, pcm_bytes(&[1]));
    data[8..12].copy_from_slice(&((1u32 << 16) + 1).to_le_bytes());
    assert_eq!(message(Awc::parse(data, None).unwrap_err()), "damaged audio container: too many streams");
}

#[test]
fn a_cut_stream_table_is_damaged() {
    let mut data = pcm_sound(0, pcm_bytes(&[1]));
    data[8..12].copy_from_slice(&10_000u32.to_le_bytes());
    assert_eq!(message(Awc::parse(data, None).unwrap_err()), "damaged audio container: truncated");
}

#[test]
fn a_chunk_outside_the_file_is_damaged() {
    let mut data = pcm_sound(0, pcm_bytes(&[1, 2, 3, 4]));
    data.truncate(data.len() - 1);
    assert_eq!(message(Awc::parse(data, None).unwrap_err()), "damaged audio container: chunk outside the file");
}

#[test]
fn chunk_indices_in_front_of_the_streams_are_skipped() {
    let data = pcm_sound(1, pcm_bytes(&[1, 2, 3, 4]));
    let awc = Awc::parse(data, None).unwrap();
    assert_eq!(awc.sounds[0].id, 7);
    assert_eq!(awc.streams[0].1.len(), 2);
    assert_eq!((awc.streams[0].1[0].kind, awc.streams[0].1[0].size), (CHUNK_FORMAT, 24));
}

#[test]
fn chunks_are_found_by_stream_and_kind() {
    let awc = Awc::parse(pcm_sound(0, pcm_bytes(&[1, 2, 3, 4])), None).unwrap();
    assert_eq!(awc.chunk(0, CHUNK_DATA).unwrap().size, 8);
    assert!(awc.chunk(0, CHUNK_PEAK).is_none());
    assert!(awc.chunk(3, CHUNK_DATA).is_none());
}

#[test]
fn encrypted_sound_data_needs_the_key() {
    let mut plain = pcm_bytes(&[10, 20, 30, 40]);
    xor(&mut plain);
    let data = pcm_sound(2, plain);
    assert!(matches!(Awc::parse(data.clone(), None), Err(AwcError::NoKey)));
    let awc = Awc::parse(data.clone(), Some(&xor)).unwrap();
    assert!(!awc.encrypted);
    assert_eq!(awc.decode_sound(0).unwrap().samples, [10, 20, 30, 40]);
    let rdr2 = Awc::parse_rdr2(data).unwrap();
    assert_ne!(rdr2.decode_sound(0).unwrap().samples, [10, 20, 30, 40], "RDR2's flag bit means something else");
}

#[test]
fn rdr2_chunks_make_the_flag_bit_mean_something_else() {
    let streams = [(9, vec![(CHUNK_FORMAT_RDR2, rdr2_format(2, 8000, 0, 1, &[])), (CHUNK_DATA, pcm_bytes(&[3, 4]))])];
    let awc = Awc::parse(build(2, &streams), None).unwrap();
    assert_eq!(awc.decode_sound(0).unwrap().samples, [3, 4]);
}

#[test]
fn a_short_format_chunk_is_damaged() {
    let streams = [(1, vec![(CHUNK_FORMAT, vec![0u8; 19]), (CHUNK_DATA, vec![0; 4])])];
    assert_eq!(
        message(Awc::parse(build(0, &streams), None).unwrap_err()),
        "damaged audio container: short format chunk"
    );
    let streams = [(1, vec![(CHUNK_FORMAT_RDR2, vec![0u8; 31]), (CHUNK_DATA, vec![0; 4])])];
    assert_eq!(
        message(Awc::parse(build(0, &streams), None).unwrap_err()),
        "damaged audio container: short format chunk"
    );
}

#[test]
fn a_stream_without_a_format_chunk_gives_no_sound() {
    let awc = Awc::parse(build(0, &[(1, vec![(CHUNK_DATA, vec![0; 4])])]), None).unwrap();
    assert!(awc.sounds.is_empty());
    assert_eq!(message(awc.decode_sound(0).unwrap_err()), "damaged audio container: no such sound");
}

#[test]
fn a_sound_without_data_cannot_be_decoded() {
    let awc = Awc::parse(build(0, &[(1, vec![(CHUNK_FORMAT, format_chunk(4, 8000, 0))])]), None).unwrap();
    assert_eq!(message(awc.decode_sound(0).unwrap_err()), "damaged audio container: sound without data");
}

#[test]
fn decoded_pcm_is_cut_to_the_sample_count() {
    let streams = [(1, vec![(CHUNK_FORMAT, format_chunk(2, 8000, 0)), (CHUNK_DATA, pcm_bytes(&[1, 2, 3, 4]))])];
    let awc = Awc::parse(build(0, &streams), None).unwrap();
    assert_eq!(awc.decode_sound(0).unwrap().samples, [1, 2]);
}

#[test]
fn unsupported_codecs_are_refused() {
    let streams = [(1, vec![(CHUNK_FORMAT, format_chunk(2, 8000, 9)), (CHUNK_DATA, vec![0; 4])])];
    let awc = Awc::parse(build(0, &streams), None).unwrap();
    assert_eq!(awc.sounds[0].codec, Codec::Other(9));
    assert_eq!(message(awc.decode_sound(0).unwrap_err()), "damaged audio container: unsupported codec 9");
}

#[test]
fn adpcm_sounds_are_decoded() {
    let mut block = vec![0u8; 4];
    block.push(0xF7);
    let streams = [(1, vec![(CHUNK_FORMAT, format_chunk(2, 8000, 4)), (CHUNK_DATA, block)])];
    let awc = Awc::parse(build(0, &streams), None).unwrap();
    let pcm = awc.decode_sound(0).unwrap();
    assert_eq!((pcm.channels, pcm.sample_rate, pcm.samples), (1, 8000, vec![11, -19]));
}

#[test]
fn adpcm_continues_over_blocks_and_stops_at_the_sample_count() {
    let mut data = vec![0u8; 2048];
    data[0] = 0;
    data[4..].fill(0x00);
    data.extend_from_slice(&[10, 0, 0xE8, 0x03, 0x07, 0x00]);
    let all = decode_adpcm(&data, 100_000);
    assert_eq!(all.len(), 2 * 2044 + 4);
    assert!(all[..2 * 2044].iter().all(|s| *s == 0), "zero nibbles add nothing at the lowest step");
    assert_eq!(
        all[2 * 2044..],
        [1034, 1039, 1043, 1047],
        "the second block starts from its own predictor and step index"
    );
    assert_eq!(decode_adpcm(&data, 3).len(), 3);
    assert!(decode_adpcm(&data, 0).is_empty());
    assert!(decode_adpcm(&[1, 2, 3], 10).is_empty(), "a block shorter than its header");
}

#[test]
fn adpcm_clamps_its_state() {
    // A step index above 88 is clamped; a predictor at the top stays there.
    let block = [200u8, 0, 0xFF, 0x7F, 0x07];
    let out = decode_adpcm(&block, 2);
    assert_eq!(out, [32767, 32767]);
    let down = [0u8, 0, 0x00, 0x80, 0x0F];
    assert_eq!(decode_adpcm(&down, 1), [-32768]);
}

#[test]
fn mp3_without_data_is_empty_and_garbage_is_an_error() {
    assert!(decode_mp3(&[]).unwrap().is_empty());
    let err = message(decode_mp3(&[0x12; 300]).unwrap_err());
    assert!(err.starts_with("damaged audio container: MP3"), "{err}");
    let err = message(decode_mp3(&vec![0x12; 20_000]).unwrap_err());
    assert!(err.starts_with("damaged audio container: MP3"), "{err}");
}

#[test]
fn vorbis_headers_are_checked_for_length() {
    let d = [3u8, 0, 0, 0, 1, 2];
    assert_eq!(message(vorbis_headers(&d).unwrap_err()), "damaged audio container: Vorbis header outside its chunk");
    assert_eq!(message(vorbis_headers(&[1, 0]).unwrap_err()), "damaged audio container: truncated");
}

#[test]
fn vorbis_headers_that_are_not_vorbis_are_refused() {
    let mut d = Vec::new();
    for p in [&b"\x01vorbis"[..], b"\x03vorbis", b"\x05vorbis"] {
        d.extend_from_slice(&(p.len() as u32).to_le_bytes());
        d.extend_from_slice(p);
    }
    let err = message(decode_vorbis(&d, &[0; 16]).unwrap_err());
    assert!(err.starts_with("damaged audio container: Vorbis ident header"), "{err}");
    assert!(decode_vorbis(&[1], &[]).is_err());
}

#[test]
fn a_vorbis_sound_keeps_its_headers_in_its_format_chunk() {
    let headers = [7u8; 20];
    let streams = [(3, vec![(CHUNK_FORMAT_RDR2, rdr2_format(100, 32000, 8, 1, &headers)), (CHUNK_DATA, vec![0; 8])])];
    let awc = Awc::parse(build(0, &streams), None).unwrap();
    let sound = &awc.sounds[0];
    assert_eq!((sound.codec, sound.samples, sound.sample_rate, sound.channels), (Codec::Vorbis, 100, 32000, 1));
    let format = awc.chunk(0, CHUNK_FORMAT_RDR2).unwrap();
    assert_eq!(sound.headers, Some((format.offset + 32, 20)));
}

#[test]
fn a_vorbis_sound_without_headers_of_its_own_uses_the_headers_chunk() {
    let streams = [(
        3,
        vec![
            (CHUNK_FORMAT_RDR2, rdr2_format(100, 32000, 8, 1, &[])),
            (CHUNK_VORBIS_HEADERS, vec![9; 12]),
            (CHUNK_DATA, vec![0; 8]),
        ],
    )];
    let awc = Awc::parse(build(0, &streams), None).unwrap();
    let chunk = awc.chunk(0, CHUNK_VORBIS_HEADERS).unwrap();
    assert_eq!(awc.sounds[0].headers, Some((chunk.offset, 12)));
    let without = [(3, vec![(CHUNK_FORMAT_RDR2, rdr2_format(100, 32000, 8, 1, &[])), (CHUNK_DATA, vec![0; 8])])];
    let awc = Awc::parse(build(0, &without), None).unwrap();
    assert_eq!(awc.sounds[0].headers, None);
    assert_eq!(message(awc.decode_sound(0).unwrap_err()), "damaged audio container: Vorbis sound without headers");
}

#[test]
fn header_sizes_beyond_the_chunk_are_cut() {
    let mut format = rdr2_format(100, 32000, 8, 1, &[1, 2, 3, 4]);
    format[30..32].copy_from_slice(&500u16.to_le_bytes());
    let streams = [(3, vec![(CHUNK_FORMAT_RDR2, format), (CHUNK_DATA, vec![0; 8])])];
    let awc = Awc::parse(build(0, &streams), None).unwrap();
    let at = awc.chunk(0, CHUNK_FORMAT_RDR2).unwrap().offset;
    assert_eq!(awc.sounds[0].headers, Some((at + 32, 4)));
}

#[test]
fn vorbis_headers_outside_the_file_are_refused_when_decoding() {
    let streams = [(3, vec![(CHUNK_FORMAT_RDR2, rdr2_format(100, 32000, 8, 1, &[1, 2])), (CHUNK_DATA, vec![0; 8])])];
    let mut awc = Awc::parse(build(0, &streams), None).unwrap();
    awc.sounds[0].headers = Some((1 << 20, 10));
    assert_eq!(message(awc.decode_sound(0).unwrap_err()), "damaged audio container: Vorbis headers outside the file");
}

#[test]
fn a_mono_opus_sound_is_read_at_48_khz() {
    let mut unit = vec![0u8; 2048];
    unit[..6].copy_from_slice(&[1, 0, 0xF8, 1, 0, 0xF8]);
    let frames = 2 * 960 - OPUS_PRE_SKIP as u32;
    let streams = [(5, vec![(CHUNK_FORMAT_RDR2, rdr2_format(frames, 32000, 11, 1, &[])), (CHUNK_DATA, unit)])];
    let awc = Awc::parse(build(0, &streams), None).unwrap();
    let sound = &awc.sounds[0];
    assert_eq!((sound.codec, sound.channels, sound.samples, sound.sample_rate), (Codec::Opus, 1, frames, OPUS_RATE));
    let pcm = awc.decode_sound(0).unwrap();
    assert_eq!((pcm.channels, pcm.samples.len()), (1, frames as usize));
}

#[test]
fn opus_channel_counts_are_clamped_to_one_or_two() {
    let unit = vec![0u8; 2048];
    for (given, want) in [(0u8, 1u8), (1, 1), (2, 2), (6, 2)] {
        let streams =
            [(5, vec![(CHUNK_FORMAT_RDR2, rdr2_format(1000, 32000, 11, given, &[])), (CHUNK_DATA, unit.clone())])];
        let awc = Awc::parse(build(0, &streams), None).unwrap();
        assert_eq!(awc.sounds[0].channels, want, "{given}");
        assert_eq!(awc.sounds[0].samples, 1000 / u32::from(want));
    }
}

#[test]
fn opus_data_is_decoded_to_the_requested_length() {
    let mut unit = vec![0u8; 2048];
    unit[..6].copy_from_slice(&[1, 0, 0xFC, 1, 0, 0xFC]);
    assert_eq!(decode_opus(&unit, 2, 100).unwrap().len(), 200);
    assert_eq!(decode_opus(&unit, 2, 100_000).unwrap().len(), (2 * 960 - OPUS_PRE_SKIP) * 2);
    assert!(decode_opus(&[], 1, 10).unwrap().is_empty());
}

#[test]
fn a_damaged_opus_packet_is_an_error() {
    let mut unit = vec![0u8; 2048];
    unit[..5].copy_from_slice(&[3, 0, 0xFB, 0xFF, 0xFF]);
    let err = message(decode_opus(&unit, 1, 10).unwrap_err());
    assert!(err.starts_with("damaged audio container: Opus packet"), "{err}");
}

/// One stream of `channels` PCM channels, `blocks` blocks of one 2048-byte unit each.
fn pcm_stream(flags: u16, channels: usize, blocks: &[Vec<Vec<i16>>], skip: u32) -> Vec<u8> {
    let block_size = 2048 + channels * 2048;
    let mut stream_format = Vec::new();
    stream_format.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    stream_format.extend_from_slice(&(block_size as u32).to_le_bytes());
    stream_format.extend_from_slice(&(channels as u32).to_le_bytes());
    let per_channel = blocks.iter().map(|b| b[0].len() as u32 - skip).sum::<u32>();
    for c in 0..channels {
        let mut entry = [0u8; 16];
        entry[0..4].copy_from_slice(&(0x100 + c as u32).to_le_bytes());
        entry[4..8].copy_from_slice(&per_channel.to_le_bytes());
        entry[10..12].copy_from_slice(&16000u16.to_le_bytes());
        entry[12] = 0;
        stream_format.extend_from_slice(&entry);
    }
    let mut data = Vec::new();
    for block in blocks {
        let mut b = vec![0u8; block_size];
        for (c, samples) in block.iter().enumerate() {
            let o = c * 24;
            b[o + 4..o + 8].copy_from_slice(&1u32.to_le_bytes());
            b[o + 8..o + 12].copy_from_slice(&skip.to_le_bytes());
            b[o + 12..o + 16].copy_from_slice(&(samples.len() as u32).to_le_bytes());
        }
        for (c, samples) in block.iter().enumerate() {
            let at = 2048 + c * 2048;
            b[at..at + samples.len() * 2].copy_from_slice(&pcm_bytes(samples));
        }
        data.extend(b);
    }
    build(flags, &[(1, vec![(CHUNK_STREAM_FORMAT, stream_format), (CHUNK_DATA, data)])])
}

fn ramp(from: i16, n: usize) -> Vec<i16> {
    (0..n as i16).map(|i| from + i).collect()
}

#[test]
fn a_pcm_stream_gives_one_interleaved_signal() {
    let blocks = [vec![ramp(0, 1024), ramp(5000, 1024)], vec![ramp(1024, 1024), ramp(6024, 1024)]];
    let awc = Awc::parse(pcm_stream(4, 2, &blocks, 0), None).unwrap();
    let format = awc.stream_format.as_ref().unwrap();
    assert_eq!((format.block_count, format.block_size, format.channels), (2, 6144, 2));
    assert_eq!(awc.sounds.len(), 2);
    assert_eq!((awc.sounds[1].id, awc.sounds[1].channel, awc.sounds[1].sample_rate), (0x101, Some(1), 16000));
    let pcm = awc.decode_stream().unwrap();
    assert_eq!((pcm.channels, pcm.sample_rate, pcm.samples.len()), (2, 16000, 4096));
    assert_eq!(&pcm.samples[..4], [0, 5000, 1, 5001]);
    assert_eq!(&pcm.samples[2048..2050], [1024, 6024]);
    assert_eq!(awc.decode_sound(1).unwrap().samples, pcm.samples, "any sound of a stream decodes the whole stream");
}

#[test]
fn the_samples_a_block_repeats_are_skipped() {
    let blocks = [vec![ramp(0, 1024)], vec![ramp(1000, 1024)]];
    let awc = Awc::parse(pcm_stream(4, 1, &blocks, 24), None).unwrap();
    let pcm = awc.decode_stream().unwrap();
    assert_eq!(pcm.samples.len(), 2 * 1000);
    assert_eq!(pcm.samples[0], 24);
    assert_eq!(pcm.samples[1000], 1024);
}

#[test]
fn a_stream_without_a_format_chunk_is_damaged() {
    let data = build(4, &[(1, vec![(CHUNK_DATA, vec![0; 16])])]);
    assert_eq!(message(Awc::parse(data, None).unwrap_err()), "damaged audio container: stream without format");
}

#[test]
fn a_bad_stream_format_is_damaged() {
    for (size, channels) in [(0u32, 1u32), (2048, 65)] {
        let mut f = vec![0u8; 12];
        f[4..8].copy_from_slice(&size.to_le_bytes());
        f[8..12].copy_from_slice(&channels.to_le_bytes());
        let data = build(4, &[(1, vec![(CHUNK_STREAM_FORMAT, f), (CHUNK_DATA, vec![0; 16])])]);
        assert_eq!(message(Awc::parse(data, None).unwrap_err()), "damaged audio container: bad stream format");
    }
}

#[test]
fn a_cut_stream_format_is_damaged() {
    let mut f = vec![0u8; 12];
    f[4..8].copy_from_slice(&2048u32.to_le_bytes());
    f[8..12].copy_from_slice(&1u32.to_le_bytes());
    let data = build(4, &[(1, vec![(CHUNK_STREAM_FORMAT, f), (CHUNK_DATA, vec![0; 16])])]);
    assert_eq!(message(Awc::parse(data, None).unwrap_err()), "damaged audio container: truncated");
}

#[test]
fn encrypted_stream_blocks_need_the_key_and_are_decrypted_block_by_block() {
    let blocks = [vec![ramp(0, 1024)], vec![ramp(2000, 1024)]];
    let plain = pcm_stream(4, 1, &blocks, 0);
    let mut encrypted = Awc::parse(plain.clone(), None).unwrap();
    let data_chunk = encrypted.chunk(0, CHUNK_DATA).unwrap().clone();
    let mut file = plain;
    xor(&mut file[data_chunk.offset..data_chunk.offset + data_chunk.size]);
    file[6..8].copy_from_slice(&(4u16 | 8).to_le_bytes());
    assert!(matches!(Awc::parse(file.clone(), None), Err(AwcError::NoKey)));
    let calls = std::cell::Cell::new(Vec::new());
    let count = |d: &mut [u8]| {
        let mut v = calls.take();
        v.push(d.len());
        calls.set(v);
        xor(d);
    };
    encrypted = Awc::parse(file.clone(), Some(&count)).unwrap();
    assert_eq!(calls.take(), [4096, 4096], "one call per block");
    assert_eq!(encrypted.decode_stream().unwrap().samples[..3], [0, 1, 2]);
    let rdr2 = Awc::parse_rdr2(file.clone()).unwrap();
    assert_eq!(rdr2.data, file, "RDR2 streams are never decrypted");
}

#[test]
fn an_encrypted_stream_without_data_is_damaged() {
    let mut f = vec![0u8; 12];
    f[4..8].copy_from_slice(&2048u32.to_le_bytes());
    let data = build(4 | 8, &[(1, vec![(CHUNK_STREAM_FORMAT, f)])]);
    assert_eq!(message(Awc::parse(data, Some(&xor)).unwrap_err()), "damaged audio container: stream without data");
}

#[test]
fn a_stream_block_with_data_outside_it_is_damaged() {
    let blocks = [vec![ramp(0, 1024)]];
    let mut file = pcm_stream(4, 1, &blocks, 0);
    let awc = Awc::parse(file.clone(), None).unwrap();
    let data = awc.chunk(0, CHUNK_DATA).unwrap().offset;
    file[data + 4..data + 8].copy_from_slice(&2000u32.to_le_bytes());
    let awc = Awc::parse(file, None).unwrap();
    let err = message(awc.decode_stream().unwrap_err());
    assert_eq!(err, "damaged audio container: block data outside the block");
}

#[test]
fn a_stream_with_a_codec_that_cannot_be_decoded_names_the_block() {
    let blocks = [vec![ramp(0, 1024)]];
    let mut file = pcm_stream(4, 1, &blocks, 0);
    let awc = Awc::parse(file.clone(), None).unwrap();
    let format = awc.chunk(0, CHUNK_STREAM_FORMAT).unwrap().offset;
    file[format + 12 + 12] = 9;
    let awc = Awc::parse(file, None).unwrap();
    let err = message(awc.decode_stream().unwrap_err());
    assert!(err.starts_with("damaged audio container: channel 0 block 0 ("), "{err}");
    assert!(err.ends_with("): damaged audio container: unsupported codec 9"), "{err}");
}

#[test]
fn a_container_that_is_no_stream_cannot_give_stream_blocks() {
    let awc = Awc::parse(pcm_sound(0, pcm_bytes(&[1, 2, 3, 4])), None).unwrap();
    assert_eq!(message(awc.stream_channels_with(24).unwrap_err()), "damaged audio container: not a stream");
}

#[test]
fn the_extra_tables_of_gta6_add_up_by_flag() {
    assert_eq!(gta6_tables_len(0, 3), 0);
    assert_eq!(gta6_tables_len(0x20, 3), 16);
    assert_eq!(gta6_tables_len(0x40, 3), 4);
    assert_eq!(gta6_tables_len(0x60, 3), 20);
    assert_eq!(gta6_tables_len(0x01, 3), 0);
}

#[test]
fn a_chunk_table_ends_after_the_chunks_it_counts() {
    // Two stream words: 1 chunk and 3 chunks (the count is in the top 3 bits).
    let mut data = Vec::new();
    data.extend_from_slice(&(1u32 << 29).to_le_bytes());
    data.extend_from_slice(&(3u32 << 29).to_le_bytes());
    assert_eq!(table_end(&data, 0, 2), Some(8 + 4 * 8));
    assert_eq!(table_end(&data, 4, 2), None, "the table is cut off");
    assert_eq!(table_end(&data, 0, 0), Some(0));
}

#[test]
fn a_chunk_table_fits_when_its_chunks_are_inside_the_file() {
    let mut data = Vec::new();
    data.extend_from_slice(&(1u32 << 29).to_le_bytes());
    data.extend_from_slice(&word(CHUNK_DATA, 4, 12).to_le_bytes());
    data.extend_from_slice(&[0; 4]);
    assert!(table_fits(&data, 0, 1));
    let mut outside = data.clone();
    outside.truncate(data.len() - 1);
    assert!(!table_fits(&outside, 0, 1));
    assert!(!table_fits(&data, 0, 9), "more streams than the file has");
}

#[test]
fn only_rdr2_chunks_mark_an_rdr2_container() {
    let chunk = |kind| Chunk { kind, offset: 0, size: 0 };
    assert!(!has_rdr2_chunks(&[(1, vec![chunk(CHUNK_DATA), chunk(CHUNK_FORMAT)])]));
    assert!(has_rdr2_chunks(&[(1, vec![chunk(CHUNK_DATA)]), (2, vec![chunk(CHUNK_FORMAT_RDR2)])]));
    assert!(has_rdr2_chunks(&[(1, vec![chunk(CHUNK_VORBIS_HEADERS)])]));
}

#[test]
fn the_start_of_the_chunk_table_follows_the_flags() {
    let data = pcm_sound(1, pcm_bytes(&[1, 2, 3, 4]));
    assert_eq!(chunk_table_start(&data, 1, 1).unwrap(), (18, false));
    let data = pcm_sound(0, pcm_bytes(&[1, 2, 3, 4]));
    assert_eq!(chunk_table_start(&data, 0, 1).unwrap(), (16, false));
    assert!(chunk_table_start(&[0; 8], 0, 1).is_err());
}

#[test]
fn gta6_tables_are_only_taken_when_they_make_the_header_add_up() {
    let mut data = pcm_sound(0, pcm_bytes(&[1, 2, 3, 4]));
    let (start, gta6) = chunk_table_start(&data, 0x40, 1).unwrap();
    assert_eq!((start, gta6), (16, false), "the header ends where the plain table ends");
    let end = u32::from_le_bytes(data[12..16].try_into().unwrap());
    data[12..16].copy_from_slice(&(end + 4).to_le_bytes());
    assert!(!chunk_table_start(&data, 0x40, 1).unwrap().1, "the tables of GTA VI are not in the file");
}

/// A one-channel Opus stream of one block: its unit of packets, the samples to skip and the stream's sample count.
fn opus_stream(unit: &[u8], skip: u32, samples: u32) -> Vec<u8> {
    let mut format = Vec::new();
    format.extend_from_slice(&1u32.to_le_bytes());
    format.extend_from_slice(&4096u32.to_le_bytes());
    format.extend_from_slice(&1u32.to_le_bytes());
    let mut entry = [0u8; 16];
    entry[4..8].copy_from_slice(&samples.to_le_bytes());
    entry[12] = 11;
    format.extend_from_slice(&entry);
    let mut block = vec![0u8; 4096];
    block[4..8].copy_from_slice(&1u32.to_le_bytes());
    block[8..12].copy_from_slice(&skip.to_le_bytes());
    block[2048..2048 + unit.len()].copy_from_slice(unit);
    build(4, &[(1, vec![(CHUNK_STREAM_FORMAT, format), (CHUNK_DATA, block)])])
}

#[test]
fn a_stereo_opus_stream_is_found_by_its_packets() {
    let frames = 2 * 960 - OPUS_PRE_SKIP as u32;
    let file = opus_stream(&[1, 0, 0xFC, 1, 0, 0xFC], 0, frames * 2);
    let awc = Awc::parse(file, None).unwrap();
    let sound = &awc.sounds[0];
    assert_eq!((sound.codec, sound.channels, sound.samples, sound.sample_rate), (Codec::Opus, 2, frames, OPUS_RATE));
    let pcm = awc.decode_stream().unwrap();
    assert_eq!((pcm.channels, pcm.sample_rate, pcm.samples.len()), (2, OPUS_RATE, frames as usize * 2));
    assert!(pcm.samples.iter().all(|s| *s == 0));
}

#[test]
fn a_mono_opus_stream_keeps_its_sample_count() {
    let frames = 2 * 960 - OPUS_PRE_SKIP as u32;
    let file = opus_stream(&[1, 0, 0xF8, 1, 0, 0xF8], 0, frames);
    let awc = Awc::parse(file, None).unwrap();
    assert_eq!((awc.sounds[0].channels, awc.sounds[0].samples), (1, frames));
    let pcm = awc.decode_stream().unwrap();
    assert_eq!((pcm.channels, pcm.samples.len()), (1, frames as usize));
}

#[test]
fn opus_packets_the_block_repeats_are_left_out() {
    let file = opus_stream(&[1, 0, 0xFC, 1, 0, 0xFC], 960, 2 * (960 - OPUS_PRE_SKIP as u32));
    let awc = Awc::parse(file, None).unwrap();
    let pcm = awc.decode_stream().unwrap();
    assert_eq!(pcm.samples.len(), 2 * (960 - OPUS_PRE_SKIP), "only the second packet is decoded");
}
