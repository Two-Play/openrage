//! Cross-platform positional file reads.
//!
//! Archives are read concurrently from one shared file handle, so every read
//! carries its own absolute offset instead of relying on a shared cursor.

use std::fs::File;
use std::io;

/// Reads exactly `buf.len()` bytes starting at absolute `offset`.
pub fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0usize;
        while done < buf.len() {
            let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            done += n;
        }
        Ok(())
    }
}

/// Little-endian u32 at `pos`. Caller guarantees bounds.
#[inline]
pub fn u32_le(data: &[u8], pos: usize) -> u32 {
    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
}

/// Little-endian 24-bit integer at `pos`. Caller guarantees bounds.
#[inline]
pub fn u24_le(data: &[u8], pos: usize) -> u32 {
    data[pos] as u32 | (data[pos + 1] as u32) << 8 | (data[pos + 2] as u32) << 16
}
