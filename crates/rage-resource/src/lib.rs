//! RSC7 resources: the paged, pointer-linked binary format behind most GTA V
//! game assets (textures, models, maps, …).
//!
//! A resource inflates to two memory segments laid out back to back:
//! *system* memory (CPU-side structures) followed by *graphics* memory
//! (GPU buffers such as pixel data). Their sizes come from the page flags.
//! Pointers inside the resource address these segments virtually:
//! `0x5xxxxxxx` is an offset into system memory, `0x6xxxxxxx` into graphics
//! memory, and `0` means null.

const SYSTEM_BASE: u64 = 0x5000_0000;
const GRAPHICS_BASE: u64 = 0x6000_0000;

#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("pointer {0:#x} is not a valid resource address")]
    BadPointer(u64),
    #[error("read of {len} bytes at {ptr:#x} lies outside the resource")]
    OutOfBounds { ptr: u64, len: usize },
    #[error("resource pages are {actual} bytes, page flags describe {expected}")]
    SizeMismatch { actual: usize, expected: usize },
    #[error("{0}")]
    Format(String),
}

/// Total size described by an RSC7 page flag word: a base page size
/// (`0x200 << shift`) times a weighted count of pages in nine size buckets.
pub fn page_size_from_flags(flags: u32) -> usize {
    let pages = ((flags >> 27) & 0x1)
        + (((flags >> 26) & 0x1) << 1)
        + (((flags >> 25) & 0x1) << 2)
        + (((flags >> 24) & 0x1) << 3)
        + (((flags >> 17) & 0x7F) << 4)
        + (((flags >> 11) & 0x3F) << 5)
        + (((flags >> 7) & 0xF) << 6)
        + (((flags >> 5) & 0x3) << 7)
        + (((flags >> 4) & 0x1) << 8);
    (0x200usize << (flags & 0xF)) * pages as usize
}

pub mod builder;

/// Type and flags of an RSC5 (GTA IV) resource header, if `file` starts with one.
pub fn rsc5_header(file: &[u8]) -> Option<(u32, u32)> {
    if file.len() < 12 || &file[0..4] != b"RSC\x05" {
        return None;
    }
    let word = |i: usize| u32::from_le_bytes(file[i..i + 4].try_into().unwrap());
    Some((word(4), word(8)))
}

/// System and graphics memory sizes described by RSC5 flags: each is an
/// 11-bit page count shifted by a 4-bit exponent (base page 256 bytes).
pub fn rsc5_sizes(flags: u32) -> (usize, usize) {
    let sys = ((flags & 0x7FF) as usize) << (((flags >> 11) & 0xF) + 8);
    let gfx = (((flags >> 15) & 0x7FF) as usize) << (((flags >> 26) & 0xF) + 8);
    (sys, gfx)
}

/// An inflated resource, split into its memory segments.
#[derive(Clone)]
pub struct Resource {
    pub version: u32,
    pub system: Vec<u8>,
    pub graphics: Vec<u8>,
}

impl Resource {
    /// Splits inflated page data using the page flags of the resource entry.
    pub fn from_pages(
        version: u32,
        system_flags: u32,
        graphics_flags: u32,
        mut pages: Vec<u8>,
    ) -> Result<Self, ResourceError> {
        let sys = page_size_from_flags(system_flags);
        let gfx = page_size_from_flags(graphics_flags);
        if pages.len() < sys {
            return Err(ResourceError::SizeMismatch { actual: pages.len(), expected: sys + gfx });
        }
        let graphics = pages.split_off(sys);
        Ok(Self { version, system: pages, graphics })
    }

    /// Parses a standalone RSC7 file (16-byte header + deflated pages).
    pub fn from_rsc7(file: &[u8]) -> Result<Self, ResourceError> {
        use std::io::Read;
        if file.len() < 16 || &file[0..4] != b"RSC7" {
            return Err(ResourceError::Format("not an RSC7 resource".into()));
        }
        let word = |i: usize| u32::from_le_bytes(file[i..i + 4].try_into().unwrap());
        let mut pages = Vec::new();
        flate2::read::DeflateDecoder::new(&file[16..])
            .read_to_end(&mut pages)
            .map_err(|e| ResourceError::Format(format!("cannot inflate resource: {e}")))?;
        Self::from_pages(word(4), word(8), word(12), pages)
    }

    /// Parses a GTA IV RSC5 resource: a 12-byte header (`RSC\x05`, type,
    /// flags) followed by a zlib stream of system then graphics memory.
    /// Bytes after the end of the stream (archive padding) are ignored.
    pub fn from_rsc5(file: &[u8]) -> Result<Self, ResourceError> {
        use std::io::Read;
        let (version, flags) = rsc5_header(file).ok_or_else(|| ResourceError::Format("not an RSC5 resource".into()))?;
        let (sys, gfx) = rsc5_sizes(flags);
        let mut pages = Vec::with_capacity(sys + gfx);
        flate2::read::ZlibDecoder::new(&file[12..])
            .take((sys + gfx) as u64)
            .read_to_end(&mut pages)
            .map_err(|e| ResourceError::Format(format!("cannot inflate resource: {e}")))?;
        if pages.len() < sys {
            return Err(ResourceError::SizeMismatch { actual: pages.len(), expected: sys + gfx });
        }
        let graphics = pages.split_off(sys);
        Ok(Self { version, system: pages, graphics })
    }

    /// Segment and offset of a virtual address. Graphics memory can exceed
    /// 256 MiB (large Enhanced texture dictionaries), so its pointers may
    /// continue past `0x6FFFFFFF`.
    fn segment(&self, ptr: u64) -> Result<(&Vec<u8>, u64), ResourceError> {
        if ptr >= GRAPHICS_BASE {
            Ok((&self.graphics, ptr - GRAPHICS_BASE))
        } else if ptr >= SYSTEM_BASE {
            Ok((&self.system, ptr - SYSTEM_BASE))
        } else {
            Err(ResourceError::BadPointer(ptr))
        }
    }

    /// The bytes at a virtual address, `len` long.
    pub fn bytes(&self, ptr: u64, len: usize) -> Result<&[u8], ResourceError> {
        let (segment, offset) = self.segment(ptr)?;
        let start = offset as usize;
        segment
            .get(start..start.checked_add(len).ok_or(ResourceError::OutOfBounds { ptr, len })?)
            .ok_or(ResourceError::OutOfBounds { ptr, len })
    }

    /// Mutable bytes at a virtual address (for in-place edits).
    pub fn bytes_mut(&mut self, ptr: u64, len: usize) -> Result<&mut [u8], ResourceError> {
        let (segment, offset) = if ptr >= GRAPHICS_BASE {
            (&mut self.graphics, ptr - GRAPHICS_BASE)
        } else if ptr >= SYSTEM_BASE {
            (&mut self.system, ptr - SYSTEM_BASE)
        } else {
            return Err(ResourceError::BadPointer(ptr));
        };
        let start = offset as usize;
        let end = start.checked_add(len).ok_or(ResourceError::OutOfBounds { ptr, len })?;
        segment.get_mut(start..end).ok_or(ResourceError::OutOfBounds { ptr, len })
    }

    pub fn u8(&self, ptr: u64) -> Result<u8, ResourceError> {
        Ok(self.bytes(ptr, 1)?[0])
    }

    pub fn u16(&self, ptr: u64) -> Result<u16, ResourceError> {
        Ok(u16::from_le_bytes(self.bytes(ptr, 2)?.try_into().unwrap()))
    }

    pub fn u32(&self, ptr: u64) -> Result<u32, ResourceError> {
        Ok(u32::from_le_bytes(self.bytes(ptr, 4)?.try_into().unwrap()))
    }

    pub fn u64(&self, ptr: u64) -> Result<u64, ResourceError> {
        Ok(u64::from_le_bytes(self.bytes(ptr, 8)?.try_into().unwrap()))
    }

    /// NUL-terminated string at `ptr` (lossy UTF-8, at most 1024 bytes).
    pub fn string(&self, ptr: u64) -> Result<String, ResourceError> {
        let (segment, offset) = self.segment(ptr)?;
        let tail = segment.get(offset as usize..).ok_or(ResourceError::OutOfBounds { ptr, len: 1 })?;
        let end = tail.iter().take(1024).position(|&b| b == 0).unwrap_or(tail.len().min(1024));
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    }

    /// Address of the first system-memory byte (where the root structure lives).
    pub const ROOT: u64 = SYSTEM_BASE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_sizes() {
        // One 8 KiB page (shift 4, one page in the "×1" bucket … expressed via bit 27).
        assert_eq!(page_size_from_flags(0x0800_0004), 0x200 << 4);
        assert_eq!(page_size_from_flags(0), 0);
    }

    #[test]
    fn pointer_resolution() {
        let r = Resource { version: 1, system: b"abcd\0xyz".to_vec(), graphics: vec![9, 8, 7, 6] };
        assert_eq!(r.string(0x5000_0000).unwrap(), "abcd");
        assert_eq!(r.u8(0x6000_0002).unwrap(), 7);
        assert_eq!(r.u32(0x6000_0000).unwrap(), 0x0607_0809);
        assert!(matches!(r.u32(0x6000_0002), Err(ResourceError::OutOfBounds { .. })));
        assert!(matches!(r.u8(0x1234), Err(ResourceError::BadPointer(_))));
        // Graphics offsets beyond 256 MiB continue past 0x6FFFFFFF.
        let big = Resource { version: 1, system: vec![], graphics: vec![0; 0x1000_0010] };
        assert!(big.u8(0x7000_0004).is_ok());
    }
}
