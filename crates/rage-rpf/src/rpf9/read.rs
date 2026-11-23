//! Bounded payload decoding. Compression blocks and sparse CTR are independent:
//! padding contributes to the physical CTR coordinate but not to decoded output.
use super::*;

struct SelectiveReader<'a> {
    input: std::io::Take<&'a mut File>,
    cipher: Option<TfitCtr>,
    ranges: [u64; 4],
    bound: u64,
    position: u64,
}

/// Native 0x1428CEBC0 / 0x141616150, returned as (encrypted, run length).
fn next_range(ranges: [u64; 4], bound: u64, position: u64) -> (bool, u64) {
    let [prefix, encrypted, clear, suffix] = ranges;
    let prefix_end = prefix.min(bound);
    let suffix_start = bound.saturating_sub(suffix).max(prefix_end);
    if position >= bound {
        return (false, u64::MAX);
    }
    if position < prefix_end {
        return (true, prefix_end - position);
    }
    if position >= suffix_start {
        return (true, bound - position);
    }
    if encrypted == 0 {
        return (false, suffix_start - position);
    }
    // Construction checks addition; no modulo-zero case when encrypted > 0.
    let within = (position - prefix_end) % (encrypted + clear);
    let (encrypted, length) =
        if within < encrypted { (true, encrypted - within) } else { (false, encrypted + clear - within) };
    (encrypted, length.min(suffix_start - position))
}

impl Read for SelectiveReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        // Even callers offering huge buffers cannot cause a huge file read.
        let cap = bytes.len().min(64 << 10);
        let n = self.input.read(&mut bytes[..cap])?;
        let mut done = 0;
        while done < n {
            let (encrypted, length) = next_range(self.ranges, self.bound, self.position);
            let length = length.min((n - done) as u64) as usize;
            if encrypted && let Some(cipher) = &mut self.cipher {
                cipher.seek(self.position);
                cipher.apply(&mut bytes[done..done + length]);
            }
            done += length;
            self.position += length as u64;
        }
        Ok(n)
    }
}

fn copy_exact_decoded(input: &mut impl Read, output: &mut impl Write, expected: u64) -> Result<u64, Rpf9Error> {
    let mut buf = [0u8; 64 << 10];
    let mut total = 0u64;
    loop {
        // One extra byte proves an output overrun without expanding it in RAM.
        let cap = (expected - total).saturating_add(1).min(buf.len() as u64) as usize;
        let n = input.read(&mut buf[..cap])?;
        if n == 0 {
            break;
        }
        if n as u64 > expected - total {
            return Err(invalid("decoded block exceeds declared size"));
        }
        output.write_all(&buf[..n])?;
        total += n as u64;
    }
    if total != expected {
        return Err(invalid("decoded block is shorter than declared size"));
    }
    Ok(total)
}

fn copy_zlib(input: &mut impl Read, output: &mut impl Write, compressed: u64, expected: u64) -> Result<u64, Rpf9Error> {
    let mut decoder = flate2::Decompress::new(true);
    let mut source = [0u8; 64 << 10];
    let mut target = [0u8; 64 << 10];
    let (mut at, mut length) = (0, 0);
    loop {
        if at == length {
            length = input.read(&mut source)?;
            at = 0;
        }
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        let cap = (expected - before_out).saturating_add(1).min(target.len() as u64) as usize;
        let flush = if length == 0 { flate2::FlushDecompress::Finish } else { flate2::FlushDecompress::None };
        let status = decoder
            .decompress(&source[at..length], &mut target[..cap], flush)
            .map_err(|e| invalid(&format!("invalid zlib stream: {e}")))?;
        let consumed = (decoder.total_in() - before_in) as usize;
        let produced = (decoder.total_out() - before_out) as usize;
        at += consumed;
        if decoder.total_out() > expected {
            return Err(invalid("zlib output exceeds declared size"));
        }
        output.write_all(&target[..produced])?;
        if status == flate2::Status::StreamEnd {
            if decoder.total_in() != compressed || decoder.total_out() != expected {
                return Err(invalid("zlib input/output size mismatch"));
            }
            return Ok(decoder.total_out());
        }
        if consumed == 0 && produced == 0 {
            return Err(invalid("truncated zlib stream"));
        }
    }
}

impl Archive {
    /// Decodes one supported entry in bounded chunks. The result is its logical
    /// data; resources include the native synthesized 16-byte RSC8 header.
    pub fn copy_decoded(
        &mut self,
        index: u32,
        keys: &ExtractedMaterial,
        output: &mut impl Write,
    ) -> Result<u64, Rpf9Error> {
        let entry =
            self.metadata.entries.get(index as usize).ok_or_else(|| invalid("entry index out of bounds"))?.clone();
        let extent = entry.extent.as_ref().ok_or_else(|| Rpf9Error::Unsupported("entry extent type".into()))?;
        let prefix = if entry.resource_flags.is_some() { 16 } else { 0 };
        let stored_payload =
            extent.stored_size.checked_sub(prefix).ok_or_else(|| invalid("resource prefix exceeds extent"))?;
        if extent.stream_codec != 0 {
            return Err(Rpf9Error::Unsupported(format!("stream codec {}", extent.stream_codec)));
        }
        let blocks: std::sync::Arc<[Block]> = match &entry.blocks {
            Some(blocks) => std::sync::Arc::clone(blocks),
            None => {
                if extent.unpacked_size > stored_payload || stored_payload - extent.unpacked_size >= 16 {
                    return Err(invalid("uncompressed extent size mismatch"));
                }
                vec![Block {
                    compressed_size: extent.unpacked_size,
                    stored_size: stored_payload,
                    unpacked_size: extent.unpacked_size,
                    codec: 0,
                }]
                .into()
            }
        };
        for block in blocks.iter() {
            if block.codec != 3 && !(block.codec == 0 && block.compressed_size == block.unpacked_size) {
                return Err(Rpf9Error::Unsupported(format!("block codec {}", block.codec)));
            }
        }
        let (mut cipher, ranges) = match &entry.crypto {
            None => (None, [0; 4]),
            Some(c) => {
                if c.algorithm != 0x11 || c.range_fields[4] != 4 {
                    return Err(Rpf9Error::Unsupported("payload cipher/range coordinates".into()));
                }
                c.range_fields[1]
                    .checked_add(c.range_fields[2])
                    .ok_or_else(|| invalid("crypto range cycle overflow"))?;
                let key = keys
                    .payload_keys
                    .get(&c.selector)
                    .ok_or_else(|| Rpf9Error::Unsupported(format!("payload key {:#x}", c.selector)))?;
                (Some(TfitCtr::new(key.clone(), c.iv)?), c.range_fields[..4].try_into().unwrap())
            }
        };
        if let Some([f0, f1]) = entry.resource_flags {
            // 0x1428CED60: synthesize the standalone resource header.
            output.write_all(&0x3843_5352u32.to_le_bytes())?;
            let packed = 0x1f00 + ((f0 << 4) & 0xff) + (f1 & 0xf);
            output.write_all(&packed.to_le_bytes())?;
            output.write_all(&f0.to_le_bytes())?;
            output.write_all(&f1.to_le_bytes())?;
        }
        let mut physical = 0;
        let mut decoded = 0;
        for block in blocks.iter() {
            self.file.seek(SeekFrom::Start(self.base_offset + extent.offset + prefix + physical))?;
            let reader = SelectiveReader {
                input: Read::by_ref(&mut self.file).take(block.compressed_size),
                cipher: cipher.take(),
                ranges,
                bound: stored_payload,
                position: physical,
            };
            let mut reader = reader;
            if block.codec == 3 {
                decoded += copy_zlib(&mut reader, output, block.compressed_size, block.unpacked_size)?;
            } else {
                decoded += copy_exact_decoded(&mut reader, output, block.unpacked_size)?;
            }
            cipher = reader.cipher;
            physical += block.stored_size;
        }
        if decoded != extent.unpacked_size {
            return Err(invalid("decoded entry size mismatch"));
        }
        Ok(decoded + prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_sparse_ranges_and_clamped_edges() {
        let r = [0x400, 0x400, 0x1c00, 0];
        assert_eq!(next_range(r, 0x10000, 0), (true, 0x400));
        assert_eq!(next_range(r, 0x10000, 0x400), (true, 0x400));
        assert_eq!(next_range(r, 0x10000, 0x800), (false, 0x1c00));
        assert_eq!(next_range(r, 0x10000, 0x2400), (true, 0x400));
        assert_eq!(next_range([500, 0, 0, 500], 100, 0), (true, 100));
        assert_eq!(next_range([0, 0, 0, 10], 100, 5), (false, 85));
        assert_eq!(next_range([0, 4096, 0, 0], 5000, 4096), (true, 904));
    }
    #[test]
    fn zlib_requires_checksum_and_stream_end() {
        use flate2::{Compression, write::ZlibEncoder};
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"a small synthetic zlib payload").unwrap();
        let data = encoder.finish().unwrap();
        let mut output = Vec::new();
        assert_eq!(copy_zlib(&mut data.as_slice(), &mut output, data.len() as u64, 30).unwrap(), 30);
        assert_eq!(output, b"a small synthetic zlib payload");
        for cut in 1..=4 {
            let truncated = &data[..data.len() - cut];
            assert!(copy_zlib(&mut &truncated[..], &mut Vec::new(), truncated.len() as u64, 30).is_err());
        }
        let mut bad = data.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(copy_zlib(&mut bad.as_slice(), &mut Vec::new(), bad.len() as u64, 30).is_err());
    }
    #[test]
    fn decoded_size_check_rejects_truncation_and_overrun() {
        assert!(copy_exact_decoded(&mut &b"abc"[..], &mut Vec::new(), 4).is_err());
        assert!(copy_exact_decoded(&mut &b"abc"[..], &mut Vec::new(), 2).is_err());
        let mut output = Vec::new();
        assert_eq!(copy_exact_decoded(&mut &b"abc"[..], &mut output, 3).unwrap(), 3);
        assert_eq!(output, b"abc");
    }
}
