//! The pages of an RSC7 segment, and adding pages to one without moving
//! anything that is already there.
//!
//! A segment's flag word holds a base size (`0x200 << shift`, bits 0..3) and
//! how many pages of each of nine sizes it has (`base × 256` down to
//! `base × 1`); the pages follow each other in that order, largest first.
//! Pages can therefore be appended without changing any existing address, as
//! long as each new page is no larger than the segment's smallest page and
//! the per-size limits hold. Data that does not fit into existing memory (new
//! texture data, structures, lists) goes into such appended pages.

use crate::ResourceError;

/// Page size multipliers, largest first, with the flag bits holding each count.
const CLASSES: [(u64, u32, u32); 9] =
    [(256, 4, 1), (128, 5, 2), (64, 7, 4), (32, 11, 6), (16, 17, 7), (8, 24, 1), (4, 25, 1), (2, 26, 1), (1, 27, 1)];

/// Most pages a resource may have (system and graphics together).
pub const MAX_PAGES: u32 = 128;

/// The pages of one segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pages {
    pub shift: u32,
    /// Page counts per class, largest first (see [`CLASSES`]).
    pub counts: [u32; 9],
    /// The top four bits of the flag word (part of the resource version).
    pub version_bits: u32,
}

impl Pages {
    pub fn from_flags(flags: u32) -> Self {
        let mut counts = [0; 9];
        for (i, &(_, bit, width)) in CLASSES.iter().enumerate() {
            counts[i] = (flags >> bit) & ((1 << width) - 1);
        }
        Self { shift: flags & 0xF, counts, version_bits: flags & 0xF000_0000 }
    }

    pub fn flags(&self) -> u32 {
        let mut f = self.shift | self.version_bits;
        for (i, &(_, bit, _)) in CLASSES.iter().enumerate() {
            f |= self.counts[i] << bit;
        }
        f
    }

    fn base(&self) -> u64 {
        0x200 << self.shift
    }

    fn class_size(&self, class: usize) -> u64 {
        self.base() * CLASSES[class].0
    }

    fn class_max(class: usize) -> u32 {
        (1 << CLASSES[class].2) - 1
    }

    /// Total size in bytes.
    pub fn len(&self) -> u64 {
        (0..9).map(|c| self.class_size(c) * self.counts[c] as u64).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Number of pages.
    pub fn count(&self) -> u32 {
        self.counts.iter().sum()
    }

    /// Index of the smallest class in use (pages after it must be no larger).
    fn smallest_class(&self) -> usize {
        (0..9).rev().find(|&c| self.counts[c] > 0).unwrap_or(0)
    }
}

/// The largest page that could still be appended to the segment described
/// by `flags` (0 if none), given the other segment's page count.
pub fn largest_new_page(flags: u32, other_pages: u32) -> u64 {
    let pages = Pages::from_flags(flags);
    if pages.count() + other_pages >= MAX_PAGES {
        return 0;
    }
    let lowest = if pages.is_empty() { 0 } else { pages.smallest_class() };
    (lowest..9).find(|&c| pages.counts[c] < Pages::class_max(c)).map_or(0, |c| pages.class_size(c))
}

/// Where appended blocks went.
#[derive(Debug, Clone)]
pub struct Appended {
    /// The segment's new flag word.
    pub flags: u32,
    /// Offset of each block from the start of the segment.
    pub offsets: Vec<u64>,
    /// The segment's new size.
    pub len: u64,
    /// Pages added.
    pub added: u32,
}

/// Places `blocks` (length, alignment) in new pages appended to the segment
/// described by `flags`. `other_pages` is the page count of the other
/// segment (both share [`MAX_PAGES`]). Fails when no allowed page is big
/// enough or the page limits are reached.
pub fn append(flags: u32, other_pages: u32, blocks: &[(u64, u64)]) -> Result<Appended, ResourceError> {
    let mut pages = Pages::from_flags(flags);
    let start = pages.len();
    if blocks.is_empty() {
        return Ok(Appended { flags, offsets: Vec::new(), len: start, added: 0 });
    }
    // Pages may only get smaller from here on.
    let mut lowest = if pages.is_empty() { 0 } else { pages.smallest_class() };
    // New pages: (class, bytes used).
    let mut new_pages: Vec<(usize, u64)> = Vec::new();
    let mut placed = vec![(0usize, 0u64); blocks.len()];
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(blocks[i].0));
    for i in order {
        let (len, align) = (blocks[i].0.max(1), blocks[i].1.max(1));
        let fits = new_pages.iter().position(|&(c, used)| used.next_multiple_of(align) + len <= pages.class_size(c));
        let page = match fits {
            Some(p) => p,
            None => {
                // The smallest allowed page that holds the block and has room left.
                let class = (lowest..9)
                    .rev()
                    .filter(|&c| pages.class_size(c) >= len)
                    .find(|&c| pages.counts[c] < Pages::class_max(c))
                    .ok_or_else(|| {
                        ResourceError::Format(format!(
                            "no room for {len} more bytes: the resource's pages are full or too small \
                             (its smallest page holds {} bytes)",
                            pages.class_size(lowest)
                        ))
                    })?;
                if pages.count() + other_pages >= MAX_PAGES {
                    return Err(ResourceError::Format(format!("the resource already has {MAX_PAGES} pages")));
                }
                pages.counts[class] += 1;
                lowest = class;
                new_pages.push((class, 0));
                new_pages.len() - 1
            }
        };
        let at = new_pages[page].1.next_multiple_of(align);
        new_pages[page].1 = at + len;
        placed[i] = (page, at);
    }
    let mut page_start = Vec::with_capacity(new_pages.len());
    let mut offset = start;
    for &(class, _) in &new_pages {
        page_start.push(offset);
        offset += pages.class_size(class);
    }
    let offsets = placed.iter().map(|&(p, at)| page_start[p] + at).collect();
    Ok(Appended { flags: pages.flags(), offsets, len: offset, added: new_pages.len() as u32 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_size_from_flags;

    #[test]
    fn flags_round_trip_and_sizes_match() {
        for flags in [0x0000_0010u32, 0x6002_8114, 0x7F00_0091, 0x1000_0002] {
            let p = Pages::from_flags(flags);
            assert_eq!(p.flags(), flags);
            assert_eq!(p.len(), page_size_from_flags(flags) as u64);
        }
    }

    #[test]
    fn appends_smaller_pages_only() {
        // One page of base × 256 (base 0x2000): 2 MiB.
        let flags = 0x0000_0010 | 4;
        let a = append(flags, 1, &[(0x30000, 256), (0x100, 16), (0x8_0000, 256)]).unwrap();
        let p = Pages::from_flags(a.flags);
        assert_eq!(p.counts[0], 1, "existing page kept");
        assert_eq!(a.len, p.len());
        assert_eq!(page_size_from_flags(a.flags) as u64, a.len);
        // All new blocks after the old 2 MiB, in non-overlapping places.
        assert!(a.offsets.iter().all(|&o| o >= 0x20_0000));
        assert_eq!(a.offsets[1] % 16, 0);
        assert_eq!(a.offsets[0] % 256, 0);
        // Pages never grow after a smaller one.
        let sizes: Vec<u64> = (0..9).flat_map(|c| std::iter::repeat_n(p.class_size(c), p.counts[c] as usize)).collect();
        assert!(sizes.windows(2).all(|w| w[0] >= w[1]));
        // Too big for anything allowed after the last page.
        let small = 0x0800_0000 | 4; // one page of base × 1
        assert!(append(small, 1, &[(0x4000, 16)]).is_err());
    }
}
