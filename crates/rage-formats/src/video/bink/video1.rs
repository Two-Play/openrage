// Ported to Rust from FFmpeg (https://ffmpeg.org), libavcodec/bink.c and
// libavcodec/binkdsp.c,
// Copyright (c) 2009 Konstantin Shishkov, 2011 Peter Ross,
// GNU Lesser General Public License version 2.1 or later; used here under the
// GNU General Public License version 3.

//! Bink 1 video, revisions `f` to `k`.
//!
//! Every plane is made of 8×8 blocks: copied from the frame before, coded
//! with a cosine transform, as runs or patterns of colours. What the blocks
//! need is stored in bundles, a row of blocks at a time.

use super::bits::Bits;
use super::tables1::*;
use super::tables2::Vlc;
use super::{BinkError, Frame};
use std::sync::LazyLock;

type Result<T> = std::result::Result<T, BinkError>;

fn bad<T>(what: &str) -> Result<T> {
    Err(BinkError::Invalid(what.to_owned()))
}

static TREES: LazyLock<[Vlc; 16]> = LazyLock::new(|| {
    std::array::from_fn(|t| {
        let mut table = [(0, 0); 512];
        for (symbol, (&code, &len)) in TREE_BITS[t].iter().zip(&TREE_LENS[t]).enumerate() {
            for entry in table[code as usize..].iter_mut().step_by(1 << len) {
                *entry = (symbol as u8, len);
            }
        }
        table
    })
});

/// A Huffman tree: one of the fixed ones, with its symbols in another order.
#[derive(Clone, Copy, Default)]
struct Tree {
    number: usize,
    symbols: [u8; 16],
}

impl Tree {
    fn read(bits: &mut Bits) -> Self {
        let number = bits.read(4) as usize;
        let mut symbols: [u8; 16] = std::array::from_fn(|i| i as u8);
        if number == 0 {
            return Self { number, symbols };
        }
        if bits.bit() {
            // The first symbols; the others follow in order.
            let count = bits.read(3) as usize + 1;
            let mut used = [false; 16];
            for s in &mut symbols[..count] {
                *s = bits.read(4) as u8;
                used[*s as usize] = true;
            }
            let rest = (0..16).filter(|&i| !used[i as usize]);
            symbols[count..].iter_mut().zip(rest).for_each(|(s, r)| *s = r);
        } else {
            for pass in 0..=bits.read(2) {
                let size = 1 << pass;
                let mut merged = [0; 16];
                for (out, pair) in merged.chunks_mut(2 * size).zip(symbols.chunks(2 * size)) {
                    let (mut a, mut b) = (pair[..size].iter().peekable(), pair[size..].iter().peekable());
                    for out in out {
                        let second = match (a.peek(), b.peek()) {
                            (Some(_), Some(_)) => bits.bit(),
                            (first, _) => first.is_none(),
                        };
                        *out = *if second { b.next() } else { a.next() }.expect("as many as there are");
                    }
                }
                symbols = merged;
            }
        }
        Self { number, symbols }
    }

    fn get(&self, bits: &mut Bits) -> i32 {
        i32::from(self.symbols[bits.vlc(&TREES[self.number]).unwrap_or(0) as usize & 15])
    }
}

fn signed(bits: &mut Bits, v: i32) -> i32 {
    if v != 0 && bits.bit() { -v } else { v }
}

/// Values of one kind, read ahead for at least a row of blocks.
#[derive(Default)]
struct Bundle {
    /// Bits of the number of values.
    len: u32,
    tree: Tree,
    values: Vec<i16>,
    next: usize,
    ended: bool,
}

impl Bundle {
    fn start(&mut self, len: usize, tree: Tree) {
        self.len = (len + 511).ilog2() + 1;
        self.tree = tree;
        self.values.clear();
        self.next = 0;
        self.ended = false;
    }

    /// How many values to read, if any are due.
    fn due(&mut self, bits: &mut Bits, mask: u32) -> Result<Option<usize>> {
        if self.ended || self.next < self.values.len() {
            return Ok(None);
        }
        let count = (bits.read(self.len) ^ mask) as usize;
        self.ended = count == 0;
        self.values.clear();
        self.next = 0;
        if bits.overread() {
            return bad("a picture ends early");
        }
        Ok((count > 0).then_some(count))
    }

    fn get(&mut self) -> i32 {
        let v = self.values.get(self.next).copied().unwrap_or(0);
        self.next += 1;
        i32::from(v)
    }

    /// Run lengths.
    fn runs(&mut self, bits: &mut Bits) -> Result<()> {
        if let Some(count) = self.due(bits, 0)? {
            if bits.bit() {
                self.values.resize(count, bits.read(4) as i16);
            } else {
                let tree = self.tree;
                self.values.extend((0..count).map(|_| tree.get(bits) as i16));
            }
        }
        Ok(())
    }

    fn motion(&mut self, bits: &mut Bits) -> Result<()> {
        if let Some(count) = self.due(bits, 0)? {
            if bits.bit() {
                let v = bits.read(4) as i32;
                self.values.resize(count, signed(bits, v) as i16);
            } else {
                let tree = self.tree;
                self.values.extend((0..count).map(|_| {
                    let v = tree.get(bits);
                    signed(bits, v) as i16
                }));
            }
        }
        Ok(())
    }

    fn block_types(&mut self, bits: &mut Bits, revision: u8) -> Result<()> {
        let Some(count) = self.due(bits, if revision == b'k' { 0xBB } else { 0 })? else { return Ok(()) };
        if bits.bit() {
            self.values.resize(count, bits.read(4) as i16);
            return Ok(());
        }
        let mut last = 0;
        while self.values.len() < count {
            let v = self.tree.get(bits);
            if v < 12 {
                last = v as i16;
                self.values.push(last);
            } else {
                let run = [4, 8, 12, 32][v as usize - 12];
                if self.values.len() + run > count {
                    return bad("too many block types");
                }
                self.values.resize(self.values.len() + run, last);
            }
        }
        Ok(())
    }

    fn patterns(&mut self, bits: &mut Bits) -> Result<()> {
        if let Some(count) = self.due(bits, 0)? {
            let tree = self.tree;
            self.values.extend((0..count).map(|_| {
                let low = tree.get(bits);
                (low | tree.get(bits) << 4) as i16
            }));
        }
        Ok(())
    }

    /// The first value, then differences in groups of 8.
    fn dcs(&mut self, bits: &mut Bits, has_sign: bool) -> Result<()> {
        let Some(count) = self.due(bits, 0)? else { return Ok(()) };
        let mut v = bits.read(if has_sign { 10 } else { 11 }) as i32;
        if has_sign {
            v = signed(bits, v);
        }
        self.values.push(v as i16);
        while self.values.len() < count {
            let width = bits.read(4);
            for _ in 0..(count - self.values.len()).min(8) {
                if width > 0 {
                    let d = bits.read(width) as i32;
                    v += signed(bits, d);
                }
                let Ok(v) = i16::try_from(v) else { return bad("a block is too bright or dark") };
                self.values.push(v);
            }
        }
        Ok(())
    }
}

/// What the blocks of a plane are made of.
#[derive(Default)]
struct Bundles {
    types: Bundle,
    /// Of blocks of twice the size.
    sub_types: Bundle,
    colours: Bundle,
    /// Trees of the colours' high halves, by the one before.
    high: [Tree; 16],
    last_high: usize,
    patterns: Bundle,
    x: Bundle,
    y: Bundle,
    intra_dc: Bundle,
    inter_dc: Bundle,
    runs: Bundle,
}

impl Bundles {
    /// Reads the trees, for planes of `width` pixels and `bw` blocks.
    fn start(&mut self, bits: &mut Bits, width: usize, bw: usize) {
        let blocks = width.div_ceil(8);
        self.types.start(blocks, Tree::read(bits));
        self.sub_types.start(width.next_multiple_of(8) / 16, Tree::read(bits));
        self.high = std::array::from_fn(|_| Tree::read(bits));
        self.last_high = 0;
        self.colours.start(bw * 64, Tree::read(bits));
        self.patterns.start(bw * 8, Tree::read(bits));
        self.x.start(blocks, Tree::read(bits));
        self.y.start(blocks, Tree::read(bits));
        self.intra_dc.start(blocks, Tree::default());
        self.inter_dc.start(blocks, Tree::default());
        self.runs.start(bw * 48, Tree::read(bits));
    }

    /// Reads what a row of blocks needs.
    fn read(&mut self, bits: &mut Bits, revision: u8) -> Result<()> {
        self.types.block_types(bits, revision)?;
        self.sub_types.block_types(bits, revision)?;
        self.read_colours(bits, revision)?;
        self.patterns.patterns(bits)?;
        self.x.motion(bits)?;
        self.y.motion(bits)?;
        self.intra_dc.dcs(bits, false)?;
        self.inter_dc.dcs(bits, true)?;
        self.runs.runs(bits)
    }

    fn read_colours(&mut self, bits: &mut Bits, revision: u8) -> Result<()> {
        let Some(count) = self.colours.due(bits, 0)? else { return Ok(()) };
        let mut colour = |bits: &mut Bits| {
            let high = self.high[self.last_high].get(bits);
            self.last_high = high as usize;
            let v = high << 4 | self.colours.tree.get(bits);
            match revision {
                // Sign and magnitude.
                ..b'i' if v & 0x80 != 0 => (0x80 - (v & 0x7F)) as i16,
                ..b'i' => (0x80 + v) as i16,
                _ => v as i16,
            }
        };
        let mut values = std::mem::take(&mut self.colours.values);
        if bits.bit() {
            values.resize(count, colour(bits));
        } else {
            values.extend((0..count).map(|_| colour(bits)));
        }
        self.colours.values = values;
        Ok(())
    }

    /// The pixels of a block made of runs.
    fn run_block(&mut self, bits: &mut Bits) -> Result<[u8; 64]> {
        let mut out = [0; 64];
        let scan = &PATTERNS[bits.read(4) as usize];
        let mut i = 0;
        while i < 63 {
            let end = i + self.runs.get() as usize + 1;
            let Some(places) = scan.get(i..end) else { return bad("a run leaves its block") };
            if bits.bit() {
                let v = self.colours.get() as u8;
                places.iter().for_each(|&p| out[p as usize] = v);
            } else {
                places.iter().for_each(|&p| out[p as usize] = self.colours.get() as u8);
            }
            i = end;
        }
        if i == 63 {
            out[scan[63] as usize] = self.colours.get() as u8;
        }
        Ok(out)
    }

    /// The pixels of a block of two colours.
    fn pattern_block(&mut self) -> [u8; 64] {
        let colours = [self.colours.get() as u8, self.colours.get() as u8];
        let mut out = [0; 64];
        for row in out.as_chunks_mut::<8>().0 {
            let pattern = self.patterns.get();
            row.iter_mut().enumerate().for_each(|(i, p)| *p = colours[(pattern >> i & 1) as usize]);
        }
        out
    }

    fn raw_block(&mut self) -> [u8; 64] {
        std::array::from_fn(|_| self.colours.get() as u8)
    }

    /// The pixels of a whole coded block.
    fn intra_block(&mut self, bits: &mut Bits) -> [u8; 64] {
        let dc = self.intra_dc.get();
        transformed(&coefficients(bits, dc, &INTRA_QUANT)).map(|v| v as u8)
    }
}

/// The places in a block whose values are still to come, as groups that are
/// split up while bits for them are read.
struct Places {
    /// (first place, what it is), from `start` to `end`.
    list: [(u8, Group); 128],
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Group {
    /// Four places, with twelve more behind them.
    First,
    /// The twelve.
    Rest,
    Four,
    One,
    Done,
}

impl Places {
    fn new(groups: &[(u8, Group)]) -> Self {
        let mut list = [(0, Group::Done); 128];
        list[64..][..groups.len()].copy_from_slice(groups);
        Self { list, start: 64, end: 64 + groups.len() }
    }

    /// Goes over the groups once; `value` gets the places whose value is next
    /// and gives whether to go on.
    fn pass(&mut self, bits: &mut Bits, mut value: impl FnMut(&mut Bits, usize) -> bool) -> bool {
        let mut at = self.start;
        while at < self.end {
            let (place, group) = self.list[at];
            if group == Group::Done || !bits.bit() {
                at += 1;
                continue;
            }
            match group {
                Group::First | Group::Four | Group::One => {
                    let slot = at;
                    if group != Group::First {
                        at += 1;
                    }
                    if !self.expand(slot, (place, group), bits, &mut value) {
                        return false;
                    }
                }
                Group::Rest => self.split_rest(at, place),
                Group::Done => {}
            }
        }
        true
    }

    /// Takes the group at `slot` off the list and reads its places: the ones set go in front of the list, the
    /// others are given to `value`.
    fn expand(
        &mut self,
        slot: usize,
        (place, group): (u8, Group),
        bits: &mut Bits,
        value: &mut impl FnMut(&mut Bits, usize) -> bool,
    ) -> bool {
        self.list[slot] = if group == Group::First { (place + 4, Group::Rest) } else { (0, Group::Done) };
        if group == Group::One {
            return value(bits, place as usize);
        }
        for place in place..place + 4 {
            if bits.bit() {
                self.start -= 1;
                self.list[self.start] = (place, Group::One);
            } else if !value(bits, place as usize) {
                return false;
            }
        }
        true
    }

    fn split_rest(&mut self, at: usize, place: u8) {
        self.list[at] = (place, Group::Four);
        for i in 1..4 {
            self.list[self.end] = (place + 4 * i, Group::Four);
            self.end += 1;
        }
    }
}

/// The coefficients of a block, scaled.
fn coefficients(bits: &mut Bits, dc: i32, quant: &[[i32; 64]; 16]) -> [i32; 64] {
    use Group::*;
    let mut block = [0; 64];
    let mut places = Places::new(&[(4, First), (24, First), (44, First), (1, One), (2, One), (3, One)]);
    for width in (0..bits.read(4)).rev() {
        places.pass(bits, |bits, place| {
            let v = (bits.read(width) | 1 << width) as i32;
            block[place] = if bits.bit() { -v } else { v };
            true
        });
    }
    block[0] = dc;
    let quant = &quant[bits.read(4) as usize];
    let mut out = [0; 64];
    for ((&at, v), &q) in SCAN.iter().zip(block).zip(quant) {
        out[at as usize] = v.wrapping_mul(q) >> 11;
    }
    out
}

/// The differences of a block, each of few bits.
fn residue(bits: &mut Bits) -> [i32; 64] {
    use Group::*;
    let mut block = [0i32; 64];
    let mut left = bits.read(7);
    let mut places = Places::new(&[(4, First), (24, First), (44, First), (0, Four)]);
    let mut known: Vec<usize> = Vec::with_capacity(64);
    for shift in (0..=bits.read(3)).rev() {
        let mask = 1 << shift;
        let mut take = || {
            left = left.wrapping_sub(1);
            left != u32::MAX
        };
        for &at in &known {
            if bits.bit() {
                block[at] += if block[at] < 0 { -mask } else { mask };
                if !take() {
                    return block;
                }
            }
        }
        let go_on = places.pass(bits, |bits, place| {
            let at = SCAN[place] as usize;
            known.push(at);
            block[at] = if bits.bit() { -mask } else { mask };
            take()
        });
        if !go_on {
            break;
        }
    }
    block
}

fn transform(s: [i32; 8]) -> [i32; 8] {
    let mul = |x: i32, y: i32| x.wrapping_mul(y) >> 11;
    let add = i32::wrapping_add;
    let sub = i32::wrapping_sub;
    let (a0, a1) = (add(s[0], s[4]), sub(s[0], s[4]));
    let (a2, a3) = (add(s[2], s[6]), mul(2896, sub(s[2], s[6])));
    let (a4, a5) = (add(s[5], s[3]), sub(s[5], s[3]));
    let (a6, a7) = (add(s[1], s[7]), sub(s[1], s[7]));
    let b0 = add(a4, a6);
    let b1 = mul(3784, add(a5, a7));
    let b2 = add(sub(mul(-5352, a5), b0), b1);
    let b3 = sub(mul(2896, sub(a6, a4)), b2);
    let b4 = sub(add(mul(2217, a7), b3), b1);
    let (c0, c1, c2, c3) = (add(a0, a2), sub(add(a1, a3), a2), add(sub(a1, a3), a2), sub(a0, a2));
    [add(c0, b0), add(c1, b2), add(c2, b3), sub(c3, b4), add(c3, b4), sub(c2, b3), sub(c1, b2), sub(c0, b0)]
}

/// The pixels (or differences) of coefficients.
fn transformed(block: &[i32; 64]) -> [i32; 64] {
    let mut columns = [0; 64];
    for x in 0..8 {
        let column = transform(std::array::from_fn(|y| block[8 * y + x]));
        column.into_iter().enumerate().for_each(|(y, v)| columns[8 * y + x] = v);
    }
    let mut out = [0; 64];
    for (out, row) in out.as_chunks_mut::<8>().0.iter_mut().zip(columns.as_chunks::<8>().0) {
        *out = transform(*row).map(|v| v.wrapping_add(0x7F) >> 8);
    }
    out
}

/// One plane of samples; room for a whole number of 16×16 blocks.
#[derive(Clone, Default)]
struct Plane {
    data: Vec<u8>,
    stride: usize,
}

impl Plane {
    fn new(width: usize, height: usize) -> Self {
        let stride = width.next_multiple_of(16);
        Self { data: vec![0; stride * height.next_multiple_of(16)], stride }
    }

    fn put(&mut self, at: usize, block: &[u8; 64]) {
        for (row, from) in self.data[at..].chunks_mut(self.stride).zip(block.as_chunks::<8>().0) {
            row[..8].copy_from_slice(from);
        }
    }

    fn put_scaled(&mut self, at: usize, block: &[u8; 64]) {
        for (y, row) in self.data[at..].chunks_mut(self.stride).take(16).enumerate() {
            row[..16].iter_mut().enumerate().for_each(|(x, p)| *p = block[y / 2 * 8 + x / 2]);
        }
    }

    fn get(&self, at: usize) -> Option<[u8; 64]> {
        let mut out = [0; 64];
        for (y, out) in out.as_chunks_mut::<8>().0.iter_mut().enumerate() {
            out.copy_from_slice(self.data.get(at + y * self.stride..)?.get(..8)?);
        }
        Some(out)
    }
}

/// The picture before, where blocks are copied from.
struct Source<'a> {
    last: &'a Plane,
    stride: usize,
    highest: usize,
}

impl Source<'_> {
    #[inline]
    fn moved(&self, b: &mut Bundles, at: usize) -> Result<[u8; 64]> {
        let (x, y) = (b.x.get(), b.y.get());
        let from = at as i64 + i64::from(x) + i64::from(y) * self.stride as i64;
        match usize::try_from(from).ok().filter(|&from| from <= self.highest).and_then(|from| self.last.get(from)) {
            Some(block) => Ok(block),
            None => bad("a block comes from outside the picture"),
        }
    }

    #[inline]
    fn block(&self, b: &mut Bundles, bits: &mut Bits, at: usize, kind: i32) -> Result<[u8; 64]> {
        Ok(match kind {
            0 => self.last.get(at).expect("a block of the plane"),
            2 => self.moved(b, at)?,
            4 => {
                let block = self.moved(b, at)?;
                let add = residue(bits);
                std::array::from_fn(|i| block[i].wrapping_add(add[i] as u8))
            }
            7 => {
                let block = self.moved(b, at)?;
                let dc = b.inter_dc.get();
                let add = transformed(&coefficients(bits, dc, &INTER_QUANT));
                std::array::from_fn(|i| block[i].wrapping_add(add[i] as u8))
            }
            _ => basic_block(b, bits, kind).unwrap_or_else(|| bad(&format!("block of type {kind}")))?,
        })
    }
}

/// The kinds of block that need nothing from the picture before; `None` for the others.
#[inline]
fn basic_block(b: &mut Bundles, bits: &mut Bits, kind: i32) -> Option<Result<[u8; 64]>> {
    Some(match kind {
        3 => b.run_block(bits),
        5 => Ok(b.intra_block(bits)),
        6 => Ok([b.colours.get() as u8; 64]),
        8 => Ok(b.pattern_block()),
        9 => Ok(b.raw_block()),
        _ => return None,
    })
}

/// A block of the half-size grid, drawn at twice its size.
#[inline]
fn large_block(b: &mut Bundles, bits: &mut Bits) -> Result<[u8; 64]> {
    let kind = b.sub_types.get();
    basic_block(b, bits, kind).unwrap_or_else(|| bad(&format!("large block of type {kind}")))
}

pub struct Decoder {
    revision: u8,
    has_alpha: bool,
    width: usize,
    height: usize,
    /// Y, Cb, Cr, alpha.
    planes: [Plane; 4],
    last: [Plane; 4],
    bundles: Bundles,
}

impl Decoder {
    pub fn new(revision: u8, flags: u32, width: u32, height: u32) -> Result<Self> {
        if !matches!(revision, b'f'..=b'k') {
            return Err(BinkError::Unsupported(revision as char));
        }
        let has_alpha = flags & super::video2::ALPHA != 0;
        let (width, height) = (width as usize, height as usize);
        let half = Plane::new(width.div_ceil(2), height.div_ceil(2));
        let full = Plane::new(width, height);
        let alpha = if has_alpha { full.clone() } else { Plane::default() };
        let planes = [full, half.clone(), half, alpha];
        Ok(Self { revision, has_alpha, width, height, last: planes.clone(), planes, bundles: Bundles::default() })
    }

    /// Decodes a frame. A damaged one leaves the picture before.
    pub fn decode(&mut self, data: &[u8]) -> Result<()> {
        let mut bits = Bits::new(data);
        self.read_planes(&mut bits)?;
        for (last, plane) in self.last.iter_mut().zip(&mut self.planes) {
            std::mem::swap(last, plane);
        }
        // Planes the frame does not have stay.
        for (last, plane) in self.planes.iter_mut().zip(&self.last) {
            last.data.copy_from_slice(&plane.data);
        }
        Ok(())
    }

    fn read_planes(&mut self, bits: &mut Bits) -> Result<()> {
        let sized = self.revision >= b'i';
        if self.has_alpha {
            skip_size(bits, sized);
            self.plane(bits, 3)?;
        }
        skip_size(bits, sized);
        for plane in if self.revision >= b'h' { [0, 2, 1] } else { [0, 1, 2] } {
            self.plane(bits, plane)?;
            if bits.left() == 0 {
                break;
            }
        }
        Ok(())
    }

    fn plane(&mut self, bits: &mut Bits, index: usize) -> Result<()> {
        let halved = usize::from(index == 1 || index == 2);
        let (bw, bh) = ((self.width >> halved).max(1), (self.height >> halved).max(1));
        let width = bw;
        let (bw, bh) = if halved == 1 {
            (self.width.div_ceil(16), self.height.div_ceil(16))
        } else {
            (bw.div_ceil(8), bh.div_ceil(8))
        };
        if self.revision == b'k' && bits.bit() {
            let fill = bits.read(8) as u8;
            self.planes[index].data.fill(fill);
            bits.align(32);
            return Ok(());
        }
        self.blocks(bits, index, width.max(8), bw, bh)?;
        bits.align(32);
        Ok(())
    }

    fn blocks(&mut self, bits: &mut Bits, index: usize, width: usize, bw: usize, bh: usize) -> Result<()> {
        let (b, out, last) = (&mut self.bundles, &mut self.planes[index], &self.last[index]);
        b.start(bits, width, bw);
        let stride = out.stride;
        let source = Source { last, stride, highest: (bw - 1 + stride * (bh - 1)) * 8 };
        for by in 0..bh {
            b.read(bits, self.revision)?;
            let mut bx = 0;
            while bx < bw {
                let at = (by * stride + bx) * 8;
                let kind = b.types.get();
                // The lower or right part of a larger block is there already.
                if (by | bx) & 1 == 1 && kind == 1 {
                    bx += 2;
                    continue;
                }
                if kind == 1 {
                    out.put_scaled(at, &large_block(b, bits)?);
                    bx += 2;
                    continue;
                }
                out.put(at, &source.block(b, bits, at, kind)?);
                bx += 1;
            }
            if bits.overread() {
                return bad("a picture ends early");
            }
        }
        Ok(())
    }

    pub fn frame(&self) -> Frame {
        let crop = |index: usize| {
            let halved = usize::from(index == 1 || index == 2);
            let (w, h) = (self.width.div_ceil(1 << halved), self.height.div_ceil(1 << halved));
            let p = &self.last[index];
            p.data.chunks_exact(p.stride).take(h).flat_map(|row| &row[..w]).copied().collect::<Vec<u8>>()
        };
        Frame {
            width: self.width as u32,
            height: self.height as u32,
            full_range: self.revision == b'k',
            y: crop(0),
            cb: crop(1),
            cr: crop(2),
            a: self.has_alpha.then(|| crop(3)),
        }
    }
}

/// Skips the frame size field of the revisions that have one.
fn skip_size(bits: &mut Bits, sized: bool) {
    if sized {
        bits.read(32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trees_are_complete() {
        for tree in TREES.iter() {
            assert!(tree.iter().all(|&(_, len)| len > 0));
        }
        let mut bits = Bits::new(&[0b0001_0000, 0]);
        assert_eq!(Tree::read(&mut bits).symbols[15], 15);
    }

    #[test]
    fn flat_blocks_transform_to_their_dc() {
        let mut block = [0; 64];
        block[0] = 100 << 8;
        assert_eq!(transformed(&block), [100; 64]);
    }

    #[test]
    fn garbage() {
        let mut seed = 1u32;
        for round in 0..300 {
            let revision = b"fghik"[round % 5];
            let alpha = if round % 2 == 0 { super::super::video2::ALPHA } else { 0 };
            let mut d = Decoder::new(revision, alpha, 8 + round as u32 % 60, 40).unwrap();
            let data: Vec<u8> = (0..2000)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let sparse = round % 3 == 0 && seed & 0x700 != 0;
                    if sparse { 0 } else { (seed >> 24) as u8 }
                })
                .collect();
            let _ = d.decode(&data);
            let _ = d.decode(&data);
            let _ = d.frame().rgba();
        }
    }
}
