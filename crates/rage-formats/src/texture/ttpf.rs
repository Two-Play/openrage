//! GTA VI texture pools (`.ttpf`, magic `RDS5`).
//!
//! GTA VI keeps its textures outside the archives, in pools below
//! `data/ttdb`. A pool is a virtual address space cut into chunks, with a
//! list of textures that says where in that space each one lives.
//!
//! Layout (little-endian):
//! - `RDS5`, entry count, virtual size (56 bits; the top byte is 22).
//! - From 24 on, a 64-bit word per chunk and a last one that marks the end
//!   of the file: the offset in the file in units of 256 bytes (26 bits),
//!   the codec (4 bits: 0 stored, 8 zlib, 1 to 7 BCPack), the virtual
//!   address in pages of 4 KiB (22 bits), and the bytes of padding at the
//!   end of the chunk's file range (12 bits).
//! - On the next multiple of 256: `ETPF`, version 634, texture count, size
//!   of the records that follow, 16 bytes per texture: width and height
//!   (14 bits each, minus one), mip levels, format (DXGI), depth, shape,
//!   tiling mode, and the address of its allocation in pages. Then a 64-bit
//!   key per texture: models name their textures by these.
//!
//! A texture's allocation holds its mip levels the way the console's GPU
//! stores them (AMD's "Gfx10" layout): the large levels one after the other,
//! smallest first, each cut into tiles of 4 or 64 KiB whose elements are in
//! a bit-interleaved order; the small levels share the one tile in front.
//! [`Pool::texture`] undoes that and returns ordinary rows of pixels or
//! blocks.
//!
//! Chunks packed with BCPack go through [`crate::bcpack`]. Not read: BCPack
//! for BC6H and BC7 (a texture is then read from its largest level that is
//! packed otherwise), and volumes.
//!
//! The format was worked out by the Leonida project (`docs/textures.md`,
//! GPL-3.0); this reader follows its `tools/ttpf_mips.py` and
//! `tools/tex_headers.py`.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use rage_core::bytes::LeRead;

use crate::texture::{Dimension, Format, Texture};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TtpfError {
    #[error("not a texture pool: {0}")]
    Invalid(String),
    #[error("{0}")]
    Unsupported(String),
    /// Reading the texture would take more memory than the pool's limit
    /// ([`Pool::with_decode_limit`]).
    #[error("reading it would take {needed} bytes, more than the limit of {limit}")]
    TooLarge { needed: u64, limit: usize },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn invalid(msg: impl Into<String>) -> TtpfError {
    TtpfError::Invalid(msg.into())
}

fn unsupported(msg: impl Into<String>) -> TtpfError {
    TtpfError::Unsupported(msg.into())
}

const ETPF_VERSION: u32 = 634;
/// No chunk unpacks to more than this.
const MAX_CHUNK: usize = 256 << 20;
const MAX_TEXTURES: usize = 250_000;
/// How many bytes reading one texture may take by default; see
/// [`Pool::with_decode_limit`]. In the Scarlett 1.0.2.1 pools a texture with
/// all its mip levels takes at most 22 MB, and a chunk at most 8 MB packed
/// and unpacked.
pub const DEFAULT_DECODE_LIMIT: usize = 512 << 20;

/// One chunk of the pool's virtual space.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    /// Virtual address and size.
    address: u64,
    size: usize,
    /// Where it is in the file, and how many bytes.
    offset: u64,
    packed: usize,
    codec: u8,
}

/// What the pool says about a texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Descriptor {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    /// DXGI format code.
    pub format: u32,
    /// Slices of an array or volume; 6 for a cube map.
    pub depth: u32,
    /// 1: 2D (an array if `depth` is more than 1), 2: cube map, 3: volume.
    pub dimension: u32,
    /// Tiling mode: a flag and, from bit 1, the GPU's tile mode.
    pub mode: u32,
    /// For a cube map: how many smaller copies of it come before the full one.
    pub earlier_copies: u32,
    /// Virtual address of its allocation; `None` when it has none.
    pub base: Option<u64>,
}

impl Descriptor {
    fn parse(r: &[u8]) -> Descriptor {
        let (a, b, c) = (r.u32_at(0).unwrap_or(0), r.u32_at(4).unwrap_or(0), r.u32_at(8).unwrap_or(0));
        let page = c & 0x1F_FFFF;
        Descriptor {
            width: (a & 0x3FFF) + 1,
            height: (a >> 14 & 0x3FFF) + 1,
            mips: (a >> 28) + (b & 7),
            format: b >> 3 & 0x7F,
            depth: (b >> 10 & 0x7FF) + 1,
            dimension: b >> 21 & 3,
            mode: b >> 23,
            earlier_copies: b & 7,
            base: (page != 0 && page != 0x1F_FFFF).then(|| u64::from(page) * 4096),
        }
    }

    /// Whether it is a plain 2D texture (the kind [`Pool::texture`] reads).
    pub fn is_2d(&self) -> bool {
        self.dimension == 1 && self.depth == 1
    }

    /// "2D", "2D array", "cube map" or "volume".
    pub fn shape(&self) -> &'static str {
        match (self.dimension, self.depth) {
            (1, 1) => "2D",
            (1, _) => "2D array",
            (2, _) => "cube map",
            _ => "volume",
        }
    }
}

/// Bytes of one pixel, or of one 4×4 block of a compressed format.
pub(crate) fn element_bytes(format: u32) -> Option<usize> {
    Some(match format {
        2..=4 => 16,
        6..=8 => 12,
        10..=14 | 16..=18 => 8,
        24..=26 | 28..=32 | 34..=38 | 41..=43 | 87 | 88 | 91 | 93 => 4,
        49..=52 | 54 | 56..=59 => 2,
        61..=65 => 1,
        71 | 72 | 80 | 81 => 8,
        74 | 75 | 77 | 78 | 83 | 84 | 95 | 96 | 98 | 99 => 16,
        _ => return None,
    })
}

fn is_block_format(format: u32) -> bool {
    matches!(format, 70..=84 | 94..=99)
}

/// Where each address bit inside a tile comes from, lowest bit first: `n`
/// is bit `n - 1` of x, `-n` bit `n - 1` of y, 0 a bit inside the element.
/// By the element size (1, 2, 4, 8, 16 bytes). These are the GPU's public
/// tile patterns (Mesa's addrlib: `GFX10_SW_4K_S_RBPLUS` and
/// `GFX10_SW_64K_D_RBPLUS`).
const PATTERNS_4K: [[i8; 12]; 5] = [
    [1, 2, 3, 4, -1, -2, -3, -4, -5, 5, -6, 6],
    [0, 1, 2, 3, -1, -2, -3, 4, -4, 5, -5, 6],
    [0, 0, 1, 2, -1, -2, -3, 3, -4, 4, -5, 5],
    [0, 0, 0, 1, -1, -2, 2, 3, -3, 4, -4, 5],
    [0, 0, 0, 0, -1, -2, 1, 2, -3, 3, -4, 4],
];
const PATTERNS_64K: [[i8; 16]; 5] = [
    [1, 2, 3, -2, -1, -3, 4, -4, -5, 5, -6, 6, -7, 7, -8, 8],
    [0, 1, 2, 3, -1, -2, -3, 4, -4, 5, -5, 6, -6, 7, -7, 8],
    [0, 0, 1, 2, -1, -2, 3, -3, -4, 4, -5, 5, -6, 6, -7, 7],
    [0, 0, 0, 1, -1, 2, 3, -2, -3, 4, -4, 5, -5, 6, -6, 7],
    [0, 0, 0, 0, 1, -1, 2, -2, -3, 3, -4, 4, -5, 5, -6, 6],
];
/// Per element size: the step, in elements, of the grid on which the small
/// levels sit inside their shared tile.
const TAIL_GRID: [(usize, usize); 5] = [(16, 16), (16, 8), (8, 8), (8, 4), (4, 4)];

/// One mip level in a texture's allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Level {
    /// Size in elements (pixels, or blocks of a compressed format).
    across: usize,
    down: usize,
    /// Elements per row of the stored region, which is whole tiles.
    stored_across: usize,
    pixels: (u32, u32),
    /// Offset in the allocation and size of the stored region.
    offset: usize,
    region: usize,
    /// Position inside the shared tile, for the small levels.
    tile_x: usize,
    tile_y: usize,
}

/// How a 2D texture's allocation is laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Layout {
    size: usize,
    tile: usize,
    /// Elements per tile across and down.
    tile_across: usize,
    tile_down: usize,
    element: usize,
    /// The first level in the shared tile (the number of levels if none is).
    tail: usize,
    levels: Vec<Level>,
}

impl Layout {
    fn new(width: u32, height: u32, mips: u32, element: usize, block: usize, tile: usize) -> Option<Layout> {
        let log = element.trailing_zeros() as usize;
        let (width, height, mips) = (width as usize, height as usize, mips as usize);
        if !element.is_power_of_two()
            || log > 4
            || mips == 0
            || mips > (usize::BITS - width.max(height).leading_zeros()) as usize
        {
            return None;
        }
        let (half_bits, max_tail) = if tile == 4096 { (6, 8) } else { (8, 12) };
        let (tile_across, tile_down) = (1 << (half_bits - log / 2), 1 << (half_bits - log.div_ceil(2)));
        let (grid_x, grid_y) = TAIL_GRID[log];
        let (across, down) = (width.div_ceil(block), height.div_ceil(block));
        let size_of = |i: usize| (across.div_ceil(1 << i).max(1), down.div_ceil(1 << i).max(1));
        let pixels = |i: usize| ((width >> i).max(1) as u32, (height >> i).max(1) as u32);

        // The levels that get tiles of their own, largest first.
        let mut levels = Vec::with_capacity(mips);
        let mut tail = mips;
        for i in 0..mips {
            let (w, h) = size_of(i);
            if mips > 1 && w <= tile_across / 2 && h <= tile_down && mips - i <= max_tail {
                tail = i;
                break;
            }
            let (stored_across, stored_down) = (w.next_multiple_of(tile_across), h.next_multiple_of(tile_down));
            levels.push(Level {
                across: w,
                down: h,
                stored_across,
                pixels: pixels(i),
                offset: 0,
                region: stored_across * stored_down * element,
                tile_x: 0,
                tile_y: 0,
            });
        }
        // They are stored smallest first, after the shared tile if there is one.
        let mut offset = if tail < mips { tile } else { 0 };
        for level in levels.iter_mut().rev() {
            level.offset = offset;
            offset += level.region;
        }
        // The small levels, each at a fixed place of the shared tile.
        for i in tail..mips {
            let entry = max_tail - 1 - (i - tail);
            let place = if entry > 6 { 16usize << entry } else { entry << 8 };
            let (mut x, mut y) = (0, 0);
            for bit in 0..6 {
                x |= (place >> (9 + 2 * bit) & 1) << bit;
                y |= (place >> (8 + 2 * bit) & 1) << bit;
            }
            let (w, h) = size_of(i);
            levels.push(Level {
                across: w,
                down: h,
                stored_across: tile_across,
                pixels: pixels(i),
                offset: 0,
                region: tile,
                tile_x: x * grid_x,
                tile_y: y * grid_y,
            });
        }
        Some(Layout { size: offset, tile, tile_across, tile_down, element, tail, levels })
    }

    /// Puts a level's elements, as stored in tiles, into rows.
    fn untile(&self, level: &Level, stored: &[u8]) -> Option<Vec<u8>> {
        let log = self.element.trailing_zeros() as usize;
        let pattern: &[i8] = if self.tile == 4096 { &PATTERNS_4K[log] } else { &PATTERNS_64K[log] };
        // The offset inside a tile is the sum of a part from x and one from y.
        let part = |v: usize, from_x: bool| -> usize {
            pattern.iter().enumerate().fold(0, |acc, (bit, &s)| match (s, from_x) {
                (s, true) if s > 0 => acc | (v >> (s - 1) & 1) << bit,
                (s, false) if s < 0 => acc | (v >> (-s - 1) & 1) << bit,
                _ => acc,
            })
        };
        let x_parts: Vec<usize> = (0..level.across).map(|x| part(x + level.tile_x, true)).collect();
        let tiles_across = level.stored_across / self.tile_across;
        let mut out = Vec::with_capacity(level.across * level.down * self.element);
        for y in 0..level.down {
            let row = (y / self.tile_down) * tiles_across;
            let y_part = part(y + level.tile_y, false);
            for (x, x_part) in x_parts.iter().enumerate() {
                let at = ((row + x / self.tile_across) * self.tile) | x_part | y_part;
                out.extend_from_slice(stored.get(at..at + self.element)?);
            }
        }
        Some(out)
    }
}

/// How the data of a texture outside the pools is tiled.
pub(crate) struct Tiling(Layout);

impl Tiling {
    /// The layout that makes `slices` slices take `size` bytes.
    pub(crate) fn by_size(width: u32, height: u32, mips: u32, format: u32, slices: u32, size: usize) -> Option<Tiling> {
        let element = element_bytes(format)?;
        let block = if is_block_format(format) { 4 } else { 1 };
        [4096, 65536].into_iter().find_map(|tile| {
            let layout = Layout::new(width, height, mips, element, block, tile)?;
            (layout.size.checked_mul(slices as usize)? == size).then_some(Tiling(layout))
        })
    }

    /// The largest level of a slice, in rows.
    pub(crate) fn largest(&self, data: &[u8], slice: usize) -> Option<Vec<u8>> {
        let level = self.0.levels.first()?;
        let start = slice * self.0.size + level.offset;
        self.0.untile(level, data.get(start..start + level.region)?)
    }
}

/// An opened texture pool.
#[derive(Debug)]
pub struct Pool {
    file: Mutex<File>,
    virtual_size: u64,
    chunks: Vec<Chunk>,
    descriptors: Vec<Descriptor>,
    keys: Vec<u64>,
    decode_limit: usize,
}

impl Pool {
    /// Whether these bytes start a texture pool.
    pub fn is_pool(start: &[u8]) -> bool {
        start.starts_with(b"RDS5")
    }

    /// Opens a pool: reads its tables, not its textures.
    pub fn open(path: &Path) -> Result<Pool, TtpfError> {
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        let mut head = [0u8; 24];
        file.read_exact(&mut head).map_err(|_| invalid("too short"))?;
        if !Self::is_pool(&head) {
            return Err(invalid("no RDS5 magic"));
        }
        let count = head.u32_at(4).unwrap_or(0) as usize;
        let size_word = head.u64_at(8).unwrap_or(0);
        let virtual_size = size_word & 0x00FF_FFFF_FFFF_FFFF;
        if !(3..=1_000_002).contains(&count) || size_word >> 56 != 22 {
            return Err(invalid("unknown index layout"));
        }
        let mut table = vec![0u8; (count - 1) * 8];
        file.read_exact(&mut table).map_err(|_| invalid("the chunk table is cut off"))?;
        let words: Vec<u64> = table.as_chunks::<8>().0.iter().map(|w| u64::from_le_bytes(*w)).collect();
        let place = |w: u64| ((w & 0x3FF_FFFF) * 256, (w >> 30 & 0x3F_FFFF) * 4096);
        if words.last().map(|w| place(*w)) != Some((length, virtual_size)) {
            return Err(invalid("the chunk table does not end where the file does"));
        }
        let mut chunks = Vec::with_capacity(words.len() - 1);
        for pair in words.windows(2) {
            let ((offset, address), (end, address_end)) = (place(pair[0]), place(pair[1]));
            let padding = pair[0] >> 52;
            let packed = end.checked_sub(offset + padding).filter(|n| (1..=MAX_CHUNK as u64).contains(n));
            let size = address_end.checked_sub(address).filter(|n| (1..=MAX_CHUNK as u64).contains(n));
            let (Some(packed), Some(size)) = (packed, size) else {
                return Err(invalid("a chunk with an impossible range"));
            };
            let codec = (pair[0] >> 26 & 15) as u8;
            chunks.push(Chunk { address, size: size as usize, offset, packed: packed as usize, codec });
        }

        let etpf = (24 + table.len() as u64).next_multiple_of(256);
        let mut header = [0u8; 16];
        file.seek(SeekFrom::Start(etpf))?;
        file.read_exact(&mut header).map_err(|_| invalid("no texture list"))?;
        if &header[..4] != b"ETPF" || header.u32_at(4) != Some(ETPF_VERSION) {
            return Err(invalid("unknown texture list version"));
        }
        let textures = header.u32_at(8).unwrap_or(0) as usize;
        if !(1..=MAX_TEXTURES).contains(&textures) || (header.u32_at(12).unwrap_or(0) as usize) < textures * 16 {
            return Err(invalid("impossible texture count"));
        }
        let mut records = vec![0u8; textures * 16];
        file.read_exact(&mut records).map_err(|_| invalid("the texture list is cut off"))?;
        let descriptors = records.as_chunks::<16>().0.iter().map(|r| Descriptor::parse(r)).collect();
        // Right after the records: the key of each texture, by which models
        // refer to it.
        let mut keys = vec![0u8; textures * 8];
        file.read_exact(&mut keys).map_err(|_| invalid("the texture keys are cut off"))?;
        let keys = keys.as_chunks::<8>().0.iter().map(|k| u64::from_le_bytes(*k)).collect();
        Ok(Pool { file: Mutex::new(file), virtual_size, chunks, descriptors, keys, decode_limit: DEFAULT_DECODE_LIMIT })
    }

    /// Sets how many bytes reading one texture may take, so that a broken or
    /// hostile pool cannot make a reader allocate huge amounts of memory: the
    /// pixels a read returns, and separately each chunk it unpacks on the way
    /// (its packed and unpacked bytes and the range being read). A read that
    /// would take more fails with [`TtpfError::TooLarge`] before anything is
    /// allocated. The default is [`DEFAULT_DECODE_LIMIT`].
    pub fn with_decode_limit(mut self, bytes: usize) -> Pool {
        self.decode_limit = bytes;
        self
    }

    /// The keys of the textures, in their order: the materials of models
    /// name their textures by these (0: none).
    pub fn keys(&self) -> &[u64] {
        &self.keys
    }

    /// Number of textures.
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    pub fn descriptor(&self, index: usize) -> Option<&Descriptor> {
        self.descriptors.get(index)
    }

    /// The size of a texture's allocation: up to the next texture's.
    fn span(&self, index: usize, base: u64) -> Option<u64> {
        for next in self.descriptors[index + 1..].iter().filter_map(|d| d.base) {
            match next.cmp(&base) {
                std::cmp::Ordering::Less => return None,
                std::cmp::Ordering::Greater => return Some(next - base),
                std::cmp::Ordering::Equal => {}
            }
        }
        self.virtual_size.checked_sub(base)
    }

    /// The chunks that hold `size` bytes from virtual address `start`;
    /// `None` when part of the range is in no chunk.
    fn chunks_for(&self, start: u64, size: usize) -> Option<&[Chunk]> {
        let end = start + size as u64;
        let first = self.chunks.partition_point(|c| c.address + c.size as u64 <= start);
        let count = self.chunks[first..].iter().take_while(|c| c.address < end).count();
        let found = &self.chunks[first..first + count];
        let gapless = found.windows(2).all(|w| w[0].address + w[0].size as u64 == w[1].address);
        let covers = found.first().is_some_and(|c| c.address <= start)
            && found.last().is_some_and(|c| c.address + c.size as u64 >= end);
        (gapless && covers).then_some(found)
    }

    /// Whether a range can be read: none of its chunks is packed in a way
    /// that is not read (BCPack for BC6H and BC7).
    fn readable(&self, start: u64, size: usize) -> bool {
        let reads = |c: &Chunk| matches!(c.codec, 0 | 8) || crate::bcpack::reads_mode(c.codec);
        self.chunks_for(start, size).is_some_and(|chunks| chunks.iter().all(reads))
    }

    fn read_range(&self, start: u64, size: usize) -> Result<Vec<u8>, TtpfError> {
        let chunks = self.chunks_for(start, size).ok_or_else(|| invalid("a texture outside the pool's chunks"))?;
        for c in chunks {
            let needed = size as u64 + c.packed as u64 + c.size as u64;
            if needed > self.decode_limit as u64 {
                return Err(TtpfError::TooLarge { needed, limit: self.decode_limit });
            }
        }
        let mut out = vec![0u8; size];
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        for c in chunks {
            let mut packed = vec![0u8; c.packed];
            file.seek(SeekFrom::Start(c.offset))?;
            file.read_exact(&mut packed)?;
            let data = match c.codec {
                0 => packed,
                8 => {
                    let mut data = Vec::with_capacity(c.size);
                    flate2::read::ZlibDecoder::new(&packed[..]).take(c.size as u64 + 1).read_to_end(&mut data)?;
                    data
                }
                codec => crate::bcpack::unpack(&packed, codec, c.size).map_err(|e| unsupported(e.to_string()))?,
            };
            if data.len() != c.size {
                return Err(invalid("a chunk unpacks to another size than its range"));
            }
            let (from, to) = (start.max(c.address), (start + size as u64).min(c.address + c.size as u64));
            out[(from - start) as usize..(to - start) as usize]
                .copy_from_slice(&data[(from - c.address) as usize..(to - c.address) as usize]);
        }
        Ok(out)
    }

    /// Where the slices of each mip level are: per level the offset of the
    /// first slice in the allocation and the distance to the next one.
    /// `None` when the texture's kind or its allocation is not understood.
    fn placement(&self, d: &Descriptor, span: u64) -> Option<(Layout, Vec<(u64, u64)>)> {
        let element = element_bytes(d.format)?;
        let block = if is_block_format(d.format) { 4 } else { 1 };
        let tile = if d.mode >> 1 & 0xF == 0 { 65536 } else { 4096 };
        match (d.dimension, d.depth) {
            (1, 1) => {
                let layout = Layout::new(d.width, d.height, d.mips, element, block, tile)?;
                let places = layout.levels.iter().map(|l| (l.offset as u64, 0)).collect();
                (span == layout.size as u64).then_some((layout, places))
            }
            // An array: level by level, each with all its slices. The small
            // levels of slice `s` are in tile `s`.
            (1, depth) if d.mode == 1 => {
                let layout = Layout::new(d.width, d.height, d.mips, element, block, tile)?;
                let depth = u64::from(depth);
                let mut places = vec![(0, tile as u64); layout.levels.len()];
                let mut offset = if layout.tail < layout.levels.len() { depth * tile as u64 } else { 0 };
                for i in (0..layout.tail).rev() {
                    let region = layout.levels[i].region as u64;
                    places[i] = (offset, region);
                    offset += depth * region;
                }
                (span == offset).then_some((layout, places))
            }
            // A cube map: face by face, each laid out like a 2D texture in
            // 4 KiB tiles; smaller copies of the whole cube map may come first.
            (2, 6) if d.earlier_copies <= 3 && d.mode == 18 + 2 * d.earlier_copies => {
                let layout = Layout::new(d.width, d.height, d.mips, element, block, 4096)?;
                let all = 6 * layout.size as u64;
                let start = if d.earlier_copies == 0 {
                    (span == all).then_some(0)?
                } else {
                    let padded = if all >= 36 * 4096 { all.next_multiple_of(65536) } else { all };
                    span.checked_sub(padded)?
                };
                let places = layout.levels.iter().map(|l| (start + l.offset as u64, layout.size as u64)).collect();
                Some((layout, places))
            }
            _ => None,
        }
    }

    /// Reads a texture: its largest mip level that can be read and is at
    /// most `max_side` pixels wide and high (`None`: any size). The result
    /// has that one level, of every slice of an array and every face of a
    /// cube map; its name is the texture's number. Volumes are not read.
    pub fn texture(&self, index: usize, max_side: Option<u32>) -> Result<Texture, TtpfError> {
        self.read(index, max_side, false)
    }

    /// The same with the smaller mip levels that follow, down to four pixels,
    /// if it is neither an array nor a cube map.
    pub fn texture_with_mips(&self, index: usize, max_side: Option<u32>) -> Result<Texture, TtpfError> {
        self.read(index, max_side, true)
    }

    fn read(&self, index: usize, max_side: Option<u32>, mips: bool) -> Result<Texture, TtpfError> {
        let d = *self.descriptor(index).ok_or_else(|| invalid("no such texture"))?;
        if d.dimension == 3 {
            return Err(unsupported("a volume, which is not read yet"));
        }
        let base = d.base.ok_or_else(|| unsupported("it has no data in this pool"))?;
        if element_bytes(d.format).is_none() {
            return Err(unsupported(format!("unknown pixel format {}", d.format)));
        }
        let span = self.span(index, base).ok_or_else(|| unsupported("its layout is not understood"))?;
        let (layout, places) = self
            .placement(&d, span)
            .ok_or_else(|| unsupported(format!("a {} whose layout is not understood", d.shape())))?;
        let slices = u64::from(d.depth);
        let slice_at = |i: usize, s: u64| base + places[i].0 + s * places[i].1;
        let fits =
            |i: &usize| max_side.is_none_or(|max| layout.levels[*i].pixels.0.max(layout.levels[*i].pixels.1) <= max);
        let readable = |i: &usize| (0..slices).all(|s| self.readable(slice_at(*i, s), layout.levels[*i].region));
        // The largest level that fits; if only larger ones can be read, the
        // smallest of those.
        let all = 0..layout.levels.len();
        let chosen = all
            .clone()
            .filter(fits)
            .find(readable)
            .or_else(|| all.rev().filter(|i| !fits(i)).find(readable))
            .ok_or_else(|| unsupported("packed with BCPack for BC6H or BC7, which is not read yet"))?;
        let level = &layout.levels[chosen];
        let smaller: Vec<usize> = if mips && d.depth == 1 {
            (chosen + 1..layout.levels.len())
                .take_while(|i| layout.levels[*i].pixels.0.min(layout.levels[*i].pixels.1) >= 4 && readable(i))
                .collect()
        } else {
            Vec::new()
        };
        // Checked before anything is read.
        let needed = smaller
            .iter()
            .map(|i| layout.levels[*i].region as u64)
            .fold((level.region as u64).saturating_mul(slices), |sum, region| sum.saturating_add(region));
        if needed > self.decode_limit as u64 {
            return Err(TtpfError::TooLarge { needed, limit: self.decode_limit });
        }
        let mut data = Vec::new();
        for s in 0..slices {
            let stored = self.read_range(slice_at(chosen, s), level.region)?;
            data.extend(layout.untile(level, &stored).ok_or_else(|| invalid("a mip level outside its allocation"))?);
        }
        let mut levels = 1;
        for i in smaller {
            let next = &layout.levels[i];
            let stored = self.read_range(slice_at(i, 0), next.region)?;
            let Some(rows) = layout.untile(next, &stored) else { break };
            data.extend(rows);
            levels += 1;
        }
        Ok(Texture {
            name: format!("{index:06}"),
            width: level.pixels.0,
            height: level.pixels.1,
            depth: d.depth,
            levels,
            format: Format(d.format),
            dimension: if d.dimension == 2 { Dimension::Cube } else { Dimension::Texture2D },
            usage: 0,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The address of element (x, y) in its tile, bit by bit from the pattern.
    fn tile_offset(pattern: &[i8], x: usize, y: usize) -> usize {
        pattern.iter().enumerate().fold(0, |acc, (bit, &s)| {
            let source = if s > 0 {
                x >> (s - 1)
            } else if s < 0 {
                y >> (-s - 1)
            } else {
                0
            };
            acc | (source & 1) << bit
        })
    }

    #[test]
    fn layout_of_a_compressed_texture() {
        // 256×256 in blocks of 8 bytes, 4 KiB tiles: 64×64 blocks; a tile is
        // 32×16 blocks. Levels 0 and 1 get tiles, the rest share one.
        let layout = Layout::new(256, 256, 9, 8, 4, 4096).unwrap();
        assert_eq!((layout.tile_across, layout.tile_down), (32, 16));
        let own: Vec<(usize, usize, usize)> = layout.levels.iter().map(|l| (l.across, l.offset, l.region)).collect();
        assert_eq!(&own[..3], [(64, 4096 + 8192, 32768), (32, 4096, 8192), (16, 0, 4096)]);
        assert_eq!(layout.size, 4096 + 8192 + 32768);
        assert!(layout.levels[2..].iter().all(|l| l.offset == 0 && l.region == 4096));
        // The shared tile: no two levels overlap.
        let mut used = std::collections::HashSet::new();
        for l in &layout.levels[2..] {
            for (x, y) in (0..l.across).flat_map(|x| (0..l.down).map(move |y| (x, y))) {
                assert!(used.insert((x + l.tile_x, y + l.tile_y)), "levels overlap in the shared tile");
            }
        }
        assert_eq!(Layout::new(256, 256, 10, 8, 4, 4096), None);
        assert_eq!(Layout::new(256, 256, 1, 8, 4, 4096).unwrap().size, 32768);
    }

    #[test]
    fn untiling_puts_every_element_in_its_row() {
        let layout = Layout::new(128, 64, 1, 4, 1, 4096).unwrap();
        let level = layout.levels[0];
        assert_eq!((layout.tile_across, layout.tile_down, level.region), (32, 32, 128 * 64 * 4));
        // Each element holds its own coordinates.
        let mut stored = vec![0u8; level.region];
        for (x, y) in (0..128usize).flat_map(|x| (0..64usize).map(move |y| (x, y))) {
            let tile = (y / 32) * 4 + x / 32;
            let at = tile * 4096 + tile_offset(&PATTERNS_4K[2], x, y);
            stored[at..at + 4].copy_from_slice(&[x as u8, y as u8, 0xAB, 0xCD]);
        }
        let rows = layout.untile(&level, &stored).unwrap();
        for (i, element) in rows.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(*element, [(i % 128) as u8, (i / 128) as u8, 0xAB, 0xCD]);
        }
    }

    /// A pool with one texture of 64×64 pixels (4 bytes each, one level) in
    /// one zlib chunk, and one without data.
    #[test]
    fn reads_a_texture_from_a_pool() {
        let layout = Layout::new(64, 64, 1, 4, 1, 4096).unwrap();
        let level = layout.levels[0];
        let mut stored = vec![0u8; level.region];
        for (x, y) in (0..64usize).flat_map(|x| (0..64usize).map(move |y| (x, y))) {
            let at = ((y / 32) * 2 + x / 32) * 4096 + tile_offset(&PATTERNS_4K[2], x, y);
            stored[at..at + 4].copy_from_slice(&[x as u8, y as u8, 7, 255]);
        }
        let mut packed = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        packed.write_all(&stored).unwrap();
        let mut packed = packed.finish().unwrap();
        let padding = packed.len().next_multiple_of(256) - packed.len();
        packed.resize(packed.len() + padding, 0);

        let data_at = 1024u64;
        let word = |offset: u64, codec: u64, page: u64, padding: u64| {
            (offset / 256) | (codec << 26) | page << 30 | padding << 52
        };
        let mut file = b"RDS5".to_vec();
        file.extend_from_slice(&3u32.to_le_bytes());
        file.extend_from_slice(&(((1 + 4) * 4096u64) | (22 << 56)).to_le_bytes());
        file.extend_from_slice(&[0; 8]);
        file.extend_from_slice(&word(data_at, 8, 1, padding as u64).to_le_bytes());
        file.extend_from_slice(&word(data_at + packed.len() as u64, 0, 5, 0).to_le_bytes());
        file.resize(256, 0);
        file.extend_from_slice(b"ETPF");
        file.extend_from_slice(&ETPF_VERSION.to_le_bytes());
        file.extend_from_slice(&2u32.to_le_bytes());
        file.extend_from_slice(&48u32.to_le_bytes());
        let record = |page: u32| {
            let mut r = Vec::new();
            r.extend_from_slice(&(63u32 | 63 << 14 | 1 << 28).to_le_bytes());
            // Format 28 (RGBA8), 2D, tiling mode 2 (4 KiB tiles).
            r.extend_from_slice(&(28u32 << 3 | 1 << 21 | 2 << 23).to_le_bytes());
            r.extend_from_slice(&page.to_le_bytes());
            r.extend_from_slice(&[0; 4]);
            r
        };
        file.extend(record(1));
        file.extend(record(0));
        file.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        file.extend_from_slice(&[0; 8]);
        file.resize(data_at as usize, 0);
        file.extend_from_slice(&packed);

        let dir = std::env::temp_dir().join(format!("rage-ttpf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pool.ttpf");
        std::fs::write(&path, &file).unwrap();
        let pool = Pool::open(&path).unwrap();
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.keys(), [0x1122_3344_5566_7788, 0]);
        let d = pool.descriptor(0).unwrap();
        assert_eq!((d.width, d.height, d.mips, d.format, d.shape()), (64, 64, 1, 28, "2D"));
        let texture = pool.texture(0, None).unwrap();
        assert_eq!((texture.width, texture.height, texture.levels, texture.name.as_str()), (64, 64, 1, "000000"));
        for (i, pixel) in texture.data.as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(*pixel, [(i % 64) as u8, (i / 64) as u8, 7, 255]);
        }
        assert!(matches!(pool.texture(1, None), Err(TtpfError::Unsupported(_))));
        assert!(pool.texture(2, None).is_err());
        // The decode limit: first for the pixels, then for the chunk that is
        // unpacked on the way.
        let region = level.region as u64;
        let limited = |limit: usize| Pool::open(&path).unwrap().with_decode_limit(limit).texture(0, None);
        assert!(matches!(limited(1024), Err(TtpfError::TooLarge { needed, limit: 1024 }) if needed == region));
        assert!(matches!(limited(region as usize), Err(TtpfError::TooLarge { needed, .. }) if needed > region));
        assert!(limited(2 * region as usize + packed.len()).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
