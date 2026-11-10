//! Building RSC7 resources from blocks.
//!
//! Callers add blocks (byte buffers in system or graphics memory) and the
//! pointers between them. [`Builder::build`] assigns every block a place in
//! a page, patches the pointers and writes a standalone RSC7 file.
//!
//! Page layout follows the scheme used by CodeWalker (and accepted by the
//! game): five page sizes (`base × 1, 2, 4, 8, 16`, base = `0x2000 << shift`),
//! larger pages first, at most 127/63/15/3/1 pages per size and 128 pages in
//! total. The root block always starts at the first system page; other blocks
//! are placed largest first into the smallest page that fits, 16-byte aligned.

use std::io::Write;

use crate::ResourceError;

const ALIGN: u64 = 16;
const MAX_PAGES: usize = 128;
const SYSTEM_BASE: u64 = 0x5000_0000;
const GRAPHICS_BASE: u64 = 0x6000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    System,
    Graphics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(usize);

struct Block {
    segment: Segment,
    data: Vec<u8>,
}

struct Pointer {
    from: BlockId,
    at: usize,
    to: BlockId,
    plus: u64,
}

#[derive(Default)]
pub struct Builder {
    blocks: Vec<Block>,
    pointers: Vec<Pointer>,
    /// (block, offset of the page counts) of the pages-info block, resized during layout.
    pages_info: Option<BlockId>,
}

/// Placement of one segment.
struct Layout {
    flags: u32,
    size: u64,
    pages: usize,
    positions: Vec<(BlockId, u64)>,
}

fn layout(blocks: &[(BlockId, u64)], max_pages: usize) -> Result<Layout, ResourceError> {
    if blocks.is_empty() {
        return Ok(Layout { flags: 0, size: 0, pages: 0, positions: Vec::new() });
    }
    let max_block = blocks.iter().map(|b| b.1).max().unwrap_or(0);
    let min_block = blocks.iter().map(|b| b.1).min().unwrap_or(0);
    let mut shift = 0u32;
    let mut base = 0x2000u64;
    while (base < min_block || base * 16 < max_block) && shift < 0xF {
        shift += 1;
        base = 0x2000 << shift;
    }
    if base * 16 < max_block {
        return Err(ResourceError::Format("block too large for a resource page".into()));
    }
    // The first block (root, or the largest) stays first; the rest go largest first.
    let mut order: Vec<(BlockId, u64)> = blocks[1..].to_vec();
    order.sort_by_key(|b| std::cmp::Reverse(b.1));
    order.insert(0, blocks[0]);

    loop {
        let mut largest = 0usize;
        let mut largest_size = base;
        while largest_size < max_block {
            largest += 1;
            largest_size *= 2;
        }
        let mut pages: [Vec<u64>; 5] = Default::default(); // used bytes per page, per size class
        let mut placed: Vec<(BlockId, usize, usize, u64)> = Vec::new();
        for (i, &(id, size)) in order.iter().enumerate() {
            if i == 0 {
                pages[largest].push(size);
                placed.push((id, largest, 0, 0));
                continue;
            }
            let mut class = 0usize;
            let mut class_size = base;
            while size > class_size && class < largest {
                class += 1;
                class_size *= 2;
            }
            let mut slot = None;
            let (mut c, mut c_size) = (class, class_size);
            while slot.is_none() && c <= largest {
                for (p, used) in pages[c].iter_mut().enumerate() {
                    let offset = used.div_ceil(ALIGN) * ALIGN;
                    if offset + size <= c_size {
                        *used = offset + size;
                        slot = Some((c, p, offset));
                        break;
                    }
                }
                c += 1;
                c_size *= 2;
            }
            let (c, p, offset) = slot.unwrap_or_else(|| {
                pages[class].push(size);
                (class, pages[class].len() - 1, 0)
            });
            placed.push((id, c, p, offset));
        }
        let counts: Vec<usize> = pages.iter().map(Vec::len).collect();
        let total: usize = counts.iter().sum();
        let fits = total <= max_pages
            && counts[0] <= 0x7F
            && counts[1] <= 0x3F
            && counts[2] <= 0xF
            && counts[3] <= 0x3
            && counts[4] <= 0x1;
        if fits {
            let mut class_offset = [0u64; 5];
            let mut offset = 0u64;
            for c in (0..5).rev() {
                class_offset[c] = offset;
                offset += (base << c) * counts[c] as u64;
            }
            let positions =
                placed.into_iter().map(|(id, c, p, o)| (id, class_offset[c] + (base << c) * p as u64 + o)).collect();
            let flags = shift
                | (counts[4] as u32 & 0x1) << 4
                | (counts[3] as u32 & 0x3) << 5
                | (counts[2] as u32 & 0xF) << 7
                | (counts[1] as u32 & 0x3F) << 11
                | (counts[0] as u32 & 0x7F) << 17;
            return Ok(Layout { flags, size: offset, pages: total, positions });
        }
        if shift >= 0xF {
            return Err(ResourceError::Format("cannot fit blocks into resource pages".into()));
        }
        shift += 1;
        base = 0x2000 << shift;
    }
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a block. The first system block added is the root.
    pub fn add(&mut self, segment: Segment, data: Vec<u8>) -> BlockId {
        self.blocks.push(Block { segment, data });
        BlockId(self.blocks.len() - 1)
    }

    /// Mutable access to a block's bytes (e.g. to patch fields).
    pub fn data_mut(&mut self, id: BlockId) -> &mut Vec<u8> {
        &mut self.blocks[id.0].data
    }

    /// Records that `from[at..at+8]` points to `to` (+ `plus` bytes).
    pub fn pointer(&mut self, from: BlockId, at: usize, to: BlockId, plus: u64) {
        self.pointers.push(Pointer { from, at, to, plus });
    }

    /// Adds a `pgBase` pages-info block and points `from[at]` to it. Its page
    /// counts are filled in during [`build`](Self::build).
    pub fn add_pages_info(&mut self, from: BlockId, at: usize) {
        let id = self.add(Segment::System, vec![0; 16]);
        self.pointer(from, at, id, 0);
        self.pages_info = Some(id);
    }

    /// Lays out all blocks and writes a standalone RSC7 file (header + deflated pages).
    pub fn build(mut self, version: u32) -> Result<Vec<u8>, ResourceError> {
        let segment_blocks = |b: &Builder, seg: Segment| -> Vec<(BlockId, u64)> {
            b.blocks
                .iter()
                .enumerate()
                .filter(|(_, blk)| blk.segment == seg)
                .map(|(i, blk)| (BlockId(i), blk.data.len().max(1) as u64))
                .collect()
        };
        // The pages-info block grows with the page count, which depends on the layout: iterate.
        let mut guess = 2usize;
        let (sys, gfx) = loop {
            if let Some(pi) = self.pages_info {
                self.blocks[pi.0].data.resize(16 + 8 * guess, 0);
            }
            let sys = layout(&segment_blocks(&self, Segment::System), MAX_PAGES)?;
            let gfx = layout(&segment_blocks(&self, Segment::Graphics), MAX_PAGES - sys.pages)?;
            if self.pages_info.is_none() || sys.pages + gfx.pages == guess {
                break (sys, gfx);
            }
            guess = sys.pages + gfx.pages;
        };
        if let Some(pi) = self.pages_info {
            let d = &mut self.blocks[pi.0].data;
            d[8] = sys.pages as u8;
            d[9] = gfx.pages as u8;
        }

        let mut address = vec![0u64; self.blocks.len()];
        for &(id, pos) in &sys.positions {
            address[id.0] = SYSTEM_BASE + pos;
        }
        for &(id, pos) in &gfx.positions {
            address[id.0] = GRAPHICS_BASE + pos;
        }
        for p in &self.pointers {
            let value = address[p.to.0] + p.plus;
            self.blocks[p.from.0].data[p.at..p.at + 8].copy_from_slice(&value.to_le_bytes());
        }

        let mut system = vec![0u8; sys.size as usize];
        let mut graphics = vec![0u8; gfx.size as usize];
        for (i, blk) in self.blocks.iter().enumerate() {
            let (buf, base) = match blk.segment {
                Segment::System => (&mut system, SYSTEM_BASE),
                Segment::Graphics => (&mut graphics, GRAPHICS_BASE),
            };
            let start = (address[i] - base) as usize;
            buf[start..start + blk.data.len()].copy_from_slice(&blk.data);
        }

        let sys_flags = sys.flags | ((version >> 4) & 0xF) << 28;
        let gfx_flags = gfx.flags | (version & 0xF) << 28;
        let mut out = Vec::with_capacity(16 + (system.len() + graphics.len()) / 4);
        out.extend_from_slice(b"RSC7");
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&sys_flags.to_le_bytes());
        out.extend_from_slice(&gfx_flags.to_le_bytes());
        let mut enc = flate2::write::DeflateEncoder::new(out, flate2::Compression::default());
        enc.write_all(&system)
            .and_then(|_| enc.write_all(&graphics))
            .map_err(|e| ResourceError::Format(e.to_string()))?;
        enc.finish().map_err(|e| ResourceError::Format(e.to_string()))
    }
}

/// Re-deflates an existing resource whose segments were modified in place
/// (same page flags).
pub fn repack(res: &crate::Resource, system_flags: u32, graphics_flags: u32) -> Result<Vec<u8>, ResourceError> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RSC7");
    out.extend_from_slice(&res.version.to_le_bytes());
    out.extend_from_slice(&system_flags.to_le_bytes());
    out.extend_from_slice(&graphics_flags.to_le_bytes());
    let mut enc = flate2::write::DeflateEncoder::new(out, flate2::Compression::default());
    enc.write_all(&res.system)
        .and_then(|_| enc.write_all(&res.graphics))
        .map_err(|e| ResourceError::Format(e.to_string()))?;
    enc.finish().map_err(|e| ResourceError::Format(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Resource;

    #[test]
    fn builds_and_reads_back() {
        let mut b = Builder::new();
        let root = b.add(Segment::System, vec![0u8; 32]);
        b.add_pages_info(root, 8);
        let name = b.add(Segment::System, b"hello\0".to_vec());
        let data = b.add(Segment::Graphics, vec![7u8; 100_000]);
        b.pointer(root, 16, name, 0);
        b.pointer(root, 24, data, 4);
        let file = b.build(0x2C).unwrap();

        let res = Resource::from_rsc7(&file).unwrap();
        assert_eq!(res.version, 0x2C);
        assert_eq!(res.string(res.u64(Resource::ROOT + 16).unwrap()).unwrap(), "hello");
        let ptr = res.u64(Resource::ROOT + 24).unwrap();
        assert_eq!(ptr & 0xF000_0000, 0x6000_0000);
        assert_eq!(res.bytes(ptr, 4).unwrap(), &[7, 7, 7, 7]);
        // Pages info carries the page counts.
        let pi = res.u64(Resource::ROOT + 8).unwrap();
        assert!(res.u8(pi + 8).unwrap() >= 1 && res.u8(pi + 9).unwrap() >= 1);
    }

    #[test]
    fn many_blocks_fit_pages() {
        let mut b = Builder::new();
        let root = b.add(Segment::System, vec![1u8; 64]);
        for i in 0..2000 {
            let blk = b.add(Segment::Graphics, vec![i as u8; 3000 + i * 37]);
            if i == 0 {
                b.pointer(root, 0, blk, 0);
            }
        }
        let file = b.build(5).unwrap();
        let res = Resource::from_rsc7(&file).unwrap();
        assert_eq!(res.bytes(res.u64(Resource::ROOT).unwrap(), 3).unwrap(), &[0, 0, 0]);
    }
}
