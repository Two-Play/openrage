// Ported to Rust from Librempeg (https://github.com/librempeg/librempeg),
// libavcodec/bink2.c, bink2.h and bink2g.h,
// Copyright (c) 2014 Konstantin Shishkov, 2019 Paul B Mahol,
// GNU General Public License version 3 or later; used here under version 3.

//! Bink 2 video, revisions `g` and later.
//!
//! A frame consists of 32×32 macroblocks in independent horizontal slices,
//! four planes (Y, Cr, Cb, alpha; chroma at half resolution), predicted from
//! the frame before or from neighbours, then smoothed by a loop filter.

use super::bits::Bits;
use super::tables2::*;
use super::{BinkError, Frame};

type Result<T> = std::result::Result<T, BinkError>;

fn bad<T>(what: &str) -> Result<T> {
    Err(BinkError::Invalid(what.to_owned()))
}

const INTRA: usize = 0;
const SKIP: usize = 1;
const MOTION: usize = 2;
const RESIDUE: usize = 3;

/// Header flag: the video has an alpha plane.
pub const ALPHA: u32 = 0x10_0000;
/// Frame flag: alpha is one value (the top byte of the flags) for the frame.
const FLAT_ALPHA: u32 = 0x8_0000;
/// Frame flag: the residual is not filtered, and other details of inter blocks.
const PLAIN_RESIDUAL: u32 = 0x4_0000;
/// Frame flag: lists of unfiltered edges precede the slices.
const EDGE_LISTS: u32 = 0x1_0000;
const NO_ROW_EDGES: u32 = 0x8000;
const NO_COLUMN_EDGES: u32 = 0x4000;

/// One plane of samples.
#[derive(Clone, Default)]
struct Plane {
    data: Vec<u8>,
    stride: usize,
}

/// What the loop filter needs, per 8×8 block: bit 15 intra, bits 13–14 how
/// few AC coefficients it has, the rest its DC.
#[derive(Default)]
struct Blocks {
    info: Vec<u16>,
    stride: usize,
}

pub struct Decoder {
    revision: u8,
    flags: u32,
    width: usize,
    height: usize,
    /// Size rounded up to whole macroblocks.
    coded: (usize, usize),
    planes: [Plane; 4],
    last: [Plane; 4],
    have_last: bool,
    blocks: [Blocks; 4],
    /// Motion vector of each 16×16 luma quadrant.
    mvs: Vec<Mv>,
}

type Mv = [i32; 2];

/// The state of a macroblock that its neighbours predict from.
#[derive(Clone, Copy, Default)]
struct Mb {
    intra_q: i32,
    inter_q: i32,
    mv: [Mv; 4],
    /// DC of each 8×8 block in raster order, by component.
    dc: [[i32; 16]; 4],
}

/// The neighbours of a macroblock.
struct Around<'a> {
    left: Option<&'a Mb>,
    top: Option<&'a Mb>,
    /// Above to the left.
    corner: Option<&'a Mb>,
}

struct Slice<'a> {
    bits: Bits<'a>,
    flags: u32,
    key: bool,
    has_alpha: bool,
    /// Size of the reference: whole macroblocks × height rounded up to 16.
    reference: (usize, usize),
    last: &'a [Plane; 4],
    dst: [&'a mut [u8]; 4],
    stride: [usize; 4],
    info: [&'a mut [u16]; 4],
    info_stride: [usize; 4],
    mvs: &'a mut [Mv],
    start: usize,
    end: usize,
}

fn shift(comp: usize) -> usize {
    usize::from(comp == 1 || comp == 2)
}

fn mid(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

fn mid_mv(a: Mv, b: Mv, c: Mv) -> Mv {
    [mid(a[0], b[0], c[0]), mid(a[1], b[1], c[1])]
}

fn add_mv(a: Mv, b: Mv) -> Mv {
    // Far beyond any picture; keeps damaged streams from overflowing.
    const LIMIT: i32 = 1 << 20;
    [(a[0] + b[0]).clamp(-LIMIT, LIMIT), (a[1] + b[1]).clamp(-LIMIT, LIMIT)]
}

fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn wrap(v: i32) -> i16 {
    v as i16
}

impl Decoder {
    pub fn new(revision: u8, flags: u32, width: u32, height: u32) -> Result<Self> {
        if !matches!(revision, b'g'..=b'n') {
            return Err(BinkError::Unsupported(revision as char));
        }
        let (width, height) = (width as usize, height as usize);
        let coded = (width.next_multiple_of(32), height.next_multiple_of(32));
        let has_alpha = flags & ALPHA != 0;
        let planes: [Plane; 4] = std::array::from_fn(|comp| {
            let (w, h) = (coded.0 >> shift(comp), coded.1 >> shift(comp));
            Plane { data: if comp < 3 || has_alpha { vec![0; w * h] } else { Vec::new() }, stride: w }
        });
        Ok(Self {
            revision,
            flags,
            width,
            height,
            coded,
            last: planes.clone(),
            planes,
            have_last: false,
            blocks: std::array::from_fn(|comp| {
                let stride = coded.0 >> 3 >> shift(comp);
                let rows = if comp < 3 || has_alpha { coded.1 >> 3 >> shift(comp) } else { 0 };
                Blocks { info: vec![0; stride * rows], stride }
            }),
            mvs: vec![[0; 2]; (coded.0 >> 4) * (coded.1 >> 4)],
        })
    }

    fn has_alpha(&self) -> bool {
        self.flags & ALPHA != 0
    }

    /// Where each slice ends, in luma rows.
    fn slice_ends(&self) -> Vec<usize> {
        let h = self.coded.1;
        match self.revision {
            b'g' if h < 128 => vec![h],
            b'g' => vec![(self.height / 2 + 16) & !31, h],
            _ => {
                let n = [2, 3, 4, 8][(self.flags & 3) as usize];
                let (mut start, mut end) = (0, h + 32 * n - 1);
                let mut ends = Vec::with_capacity(n);
                for i in 0..n - 1 {
                    start += (end.saturating_sub(start) / (n - i)) & !31;
                    end -= 32;
                    ends.push(start.min(h));
                }
                ends.push(h);
                ends
            }
        }
    }

    /// Decodes the next frame. Gives the number of bytes left unread in each
    /// slice (at most 3 of padding in an undamaged stream).
    pub fn decode(&mut self, data: &[u8], key: bool) -> Result<Vec<usize>> {
        if !key && !self.have_last {
            return bad("the video does not start with a key frame");
        }
        let left = self.decode_frame(data, key)?;
        std::mem::swap(&mut self.planes, &mut self.last);
        self.have_last = true;
        Ok(left)
    }

    fn decode_frame(&mut self, data: &[u8], key: bool) -> Result<Vec<usize>> {
        let mut bits = Bits::new(data);
        let flags = bits.read(32);
        let ends = self.slice_ends();
        let mut offsets: Vec<usize> = (1..ends.len()).map(|_| bits.read(32) as usize).collect();
        offsets.push(data.len());
        let edges = self.edge_lists(&mut bits, flags);
        if bits.overread() {
            return bad("frame header ends early");
        }
        let has_alpha = self.has_alpha() && flags & FLAT_ALPHA == 0;
        if self.has_alpha() && !has_alpha {
            self.planes[3].data.fill((flags >> 24) as u8);
        }

        let mut slices = Vec::with_capacity(ends.len());
        let mut planes = self.planes.each_mut().map(|p| p.data.as_mut_slice());
        let mut infos = self.blocks.each_mut().map(|b| b.info.as_mut_slice());
        let mut mvs = self.mvs.as_mut_slice();
        let stride = self.last.each_ref().map(|p| p.stride);
        let info_stride: [usize; 4] = std::array::from_fn(|c| self.coded.0 >> 3 >> shift(c));
        let (mut start, mut from) = (0, None);
        for (&end, &to) in ends.iter().zip(&offsets) {
            if to > data.len() || from.is_some_and(|f| f > to) {
                return bad("slice offsets are not in order");
            }
            let rows = end - start;
            slices.push(Slice {
                // The first slice follows the edge lists at once.
                bits: match from {
                    Some(from) => Bits::new(&data[from..to]),
                    None => bits.truncated(to),
                },
                flags,
                key,
                has_alpha,
                reference: (self.coded.0, self.height.next_multiple_of(16)),
                last: &self.last,
                dst: std::array::from_fn(|c| {
                    let n = if planes[c].is_empty() { 0 } else { stride[c] * (rows >> shift(c)) };
                    planes[c].split_off_mut(..n).unwrap_or_default()
                }),
                stride,
                info: std::array::from_fn(|c| {
                    let n = if infos[c].is_empty() { 0 } else { info_stride[c] * (rows >> 3 >> shift(c)) };
                    infos[c].split_off_mut(..n).unwrap_or_default()
                }),
                info_stride,
                mvs: mvs.split_off_mut(..(self.coded.0 >> 4) * (rows >> 4)).unwrap_or_default(),
                start,
                end,
            });
            (start, from) = (end, Some(to));
        }
        let threads = self.coded.0 * self.coded.1 > 256 * 256;
        let left: Vec<Result<usize>> = if threads {
            std::thread::scope(|scope| {
                let running: Vec<_> = slices.into_iter().map(|s| scope.spawn(|| s.decode())).collect();
                running.into_iter().map(|t| t.join().unwrap_or_else(|_| bad("slice"))).collect()
            })
        } else {
            slices.into_iter().map(Slice::decode).collect()
        };
        let left = left.into_iter().collect::<Result<Vec<usize>>>()?;

        let filter = Filter { flags, edges: &edges, mvs: &self.mvs, mv_stride: self.coded.0 >> 4 };
        let planes = self.planes.iter_mut().zip(&self.blocks).enumerate().take(3 + usize::from(has_alpha));
        std::thread::scope(|scope| {
            for (comp, (plane, blocks)) in planes {
                let filter = &filter;
                if threads {
                    scope.spawn(move || filter.plane(plane, blocks, comp));
                } else {
                    filter.plane(plane, blocks, comp);
                }
            }
        });
        Ok(left)
    }

    /// Rows and columns of block edges the loop filter leaves alone.
    fn edge_lists(&self, bits: &mut Bits, flags: u32) -> [Vec<bool>; 2] {
        let rows = (self.height.next_multiple_of(16) >> 3) - 1;
        let columns = (self.coded.0 >> 3) - 1;
        let mut read = |count: usize, off: u32| {
            if flags & EDGE_LISTS != 0 && flags & off == 0 { flag_list(bits, count) } else { vec![false; count] }
        };
        [read(rows, NO_ROW_EDGES), read(columns, NO_COLUMN_EDGES)]
    }

    /// The last decoded frame.
    pub fn frame(&self) -> Frame {
        let crop = |comp: usize| {
            let s = shift(comp);
            let (w, h) = (self.width.div_ceil(1 << s), self.height.div_ceil(1 << s));
            let p = &self.last[comp];
            let mut out = Vec::with_capacity(w * h);
            p.data.chunks_exact(p.stride).take(h).for_each(|row| out.extend_from_slice(&row[..w]));
            out
        };
        Frame {
            width: self.width as u32,
            height: self.height as u32,
            full_range: true,
            y: crop(0),
            cb: crop(2),
            cr: crop(1),
            a: self.has_alpha().then(|| crop(3)),
        }
    }
}

/// A list of flags, plain or as runs.
fn flag_list(bits: &mut Bits, size: usize) -> Vec<bool> {
    if !bits.bit() {
        return (0..size).map(|_| bits.bit()).collect();
    }
    let mut out: Vec<bool> = Vec::with_capacity(size);
    let (mut toggle, mut flag) = (false, false);
    while out.len() < size {
        let left = size - out.len();
        let runs = bits.bit();
        if !runs {
            flag = if toggle { !flag } else { bits.bit() };
            toggle = false;
            out.push(flag);
            for _ in 1..left.min(5) {
                out.push(bits.bit());
            }
        } else {
            let n = match left {
                ..4 => 2,
                4..16 => 4,
                _ => 5,
            };
            let mut run = n as usize + if toggle { 1 } else { 2 };
            flag = if toggle { !flag } else { bits.bit() };
            toggle = false;
            run = run.min(left);
            if run < left {
                let extra = bits.read(n);
                toggle = extra != (1 << n) - 1;
                run = (run + extra as usize).min(left);
            }
            out.resize(out.len() + run, flag);
        }
    }
    out
}

/// Predicts a DC from the blocks to the left, above, and above left.
fn gradient(corner: i32, a: i32, b: i32) -> i32 {
    (a + b - corner).clamp(a.min(b).min(corner), a.max(b).max(corner))
}

/// Predicts a DC by continuing the step from `b` to `a`.
fn extend(a: i32, b: i32) -> i32 {
    (2 * a - b).clamp(a.min(b), a.max(b))
}

/// Adds predictions to DC differences (coding order, limited to `min..=max`);
/// the result is in raster order. `left` and `top` are the DCs of the
/// neighbouring macroblocks, if any.
fn predict_dcs(
    diff: &[i32; 16],
    n: usize,
    (min, max): (i32, i32),
    left: Option<&[i32; 16]>,
    top: Option<&[i32; 16]>,
    corner: Option<&[i32; 16]>,
) -> [i32; 16] {
    // A grid with the neighbours in row and column 0.
    let mut g = [[0i32; 5]; 5];
    for i in 0..n {
        g[0][i + 1] = top.map_or(0, |t| t[(n - 1) * n + i]);
        g[i + 1][0] = left.map_or(0, |l| l[i * n + n - 1]);
    }
    g[0][0] = corner.map_or(0, |c| c[n * n - 1]);
    let mut out = [0; 16];
    for (i, d) in diff[..n * n].iter().enumerate() {
        let pos = if n == 4 { LUMA_POS[i] as usize } else { i };
        let (r, c) = (pos / n + 1, pos % n + 1);
        let predicted = match (r > 1 || top.is_some(), c > 1 || left.is_some()) {
            (true, true) => gradient(g[r - 1][c - 1], g[r][c - 1], g[r - 1][c]),
            // Along the top only even columns have the block below left yet.
            (false, true) if c % 2 == 1 => extend(g[r][c - 1], g[r + 1][c - 1]),
            (false, true) => g[r][c - 1],
            (true, false) => extend(g[r - 1][c], g[r - 1][c + 1]),
            (false, false) => 0,
        };
        g[r][c] = (predicted + d).clamp(min, max);
        out[pos] = g[r][c];
    }
    out
}

/// Predicts the one motion vector of a macroblock.
fn predict_mv(around: &Around, delta: Mv) -> Mv {
    let predicted = match (around.top, around.left) {
        (None, None) => [0; 2],
        (None, Some(l)) => mid_mv(l.mv[0], l.mv[1], l.mv[3]),
        (Some(t), None) => mid_mv(t.mv[0], t.mv[2], t.mv[3]),
        (Some(t), Some(l)) => {
            let corner = around.corner.map_or([0; 2], |c| c.mv[3]);
            mid_mv(corner, t.mv[2], l.mv[1])
        }
    };
    add_mv(delta, predicted)
}

/// Predicts the motion vectors of the four quadrants of a macroblock.
fn predict_mvs(around: &Around, delta: &[Mv; 4]) -> [Mv; 4] {
    let mut v = [[0; 2]; 4];
    v[0] = predict_mv(around, delta[0]);
    match (around.top, around.left) {
        (Some(t), None) => {
            v[1] = add_mv(delta[1], mid_mv(t.mv[2], t.mv[3], v[0]));
            v[2] = add_mv(delta[2], mid_mv(t.mv[2], v[0], v[1]));
        }
        (Some(t), Some(l)) => {
            v[1] = add_mv(delta[1], mid_mv(t.mv[2], t.mv[3], v[0]));
            v[2] = add_mv(delta[2], mid_mv(l.mv[1], l.mv[3], v[0]));
        }
        (None, None) => {
            v[1] = add_mv(delta[1], v[0]);
            v[2] = add_mv(delta[2], v[0]);
        }
        (None, Some(l)) => {
            v[2] = add_mv(delta[2], mid_mv(l.mv[1], l.mv[3], v[0]));
            v[1] = add_mv(delta[1], mid_mv(l.mv[1], v[0], v[2]));
        }
    }
    v[3] = add_mv(delta[3], mid_mv(v[0], v[1], v[2]));
    v
}

#[inline]
fn idct_1d(v: [i32; 8], shift: u32) -> [i32; 8] {
    let a = |v: i32| v + (v >> 2);
    let b = |v: i32| v >> 1;
    let c = |v: i32| v - (v >> 2) - (v >> 4);
    let d = |v: i32| v + (v >> 2) - (v >> 4);
    let e = |v: i32| v >> 2;
    let (t00, t01) = (v[3] + v[5], v[3] - v[5]);
    let (t02, t03) = (a(v[2]) + b(v[6]), b(v[2]) - a(v[6]));
    let (t0, t1, t2) = (v[0] + v[4] + t02, v[0] + v[4] - t02, v[0] - v[4]);
    let (t3, t4, t5, t6) = (v[1] + t00, v[1] - t00, t01 + v[7], t01 - v[7]);
    let (t7, t8) = (t4 + c(t6), c(t4) - t6);
    let (t9, t10) = (d(t3) + e(t5), e(t3) - d(t5));
    let (t11, t12) = (t2 + t03, t2 - t03);
    [t0 + t9, t11 + t7, t12 + t8, t1 + t10, t1 - t10, t12 - t8, t11 - t7, t0 - t9].map(|v| i32::from(wrap(v >> shift)))
}

/// Inverse transform; the result is transposed.
fn idct(block: &mut [i16; 64]) {
    let mut t = [0i32; 64];
    for i in 0..8 {
        let col = idct_1d(std::array::from_fn(|k| i32::from(block[k * 8 + i])), 0);
        for (k, v) in col.into_iter().enumerate() {
            t[k * 8 + i] = v;
        }
    }
    for i in 0..8 {
        let row = idct_1d(std::array::from_fn(|k| t[i * 8 + k]), 6);
        for (k, v) in row.into_iter().enumerate() {
            block[k * 8 + i] = wrap(v);
        }
    }
}

/// Filters 8 samples along a block edge. `at` is the first sample after the
/// edge, `step` goes across it and `next` along it; `sa` and `sb` are the
/// strengths before and after it.
macro_rules! filter_edge {
    ($pix:expr, $at:expr, $step:expr, $next:expr, $sa:expr, $sb:expr, $clip:expr) => {
        for k in 0..8 {
            let at = $at + k * $next;
            let (p1, p0) = (i32::from($pix[at - 2 * $step]), i32::from($pix[at - $step]));
            let (q0, q1) = (i32::from($pix[at]), i32::from($pix[at + $step]));
            let d = q0 - p0;
            let (a, b) = ((d + 2) >> 2, (d + 4) >> 3);
            if $sa >= 1 {
                $pix[at - $step] = $clip(p0 + a);
            }
            if $sa >= 2 {
                $pix[at - 2 * $step] = $clip(p1 + b);
            }
            if $sb >= 1 {
                $pix[at] = $clip(q0 - a);
            }
            if $sb >= 2 {
                $pix[at + $step] = $clip(q1 - b);
            }
        }
    };
}

/// A half-pel motion vector component that places a visible block exactly 19
/// pixels before the end of the reference, the last position where the 6-tap
/// filter still fits, is replaced by the full-pel position 2 pixels before.
fn edge_mv(mv: i32, pos: i32, size: i32) -> i32 {
    if mv & 1 != 0 && pos + 16 <= size && (mv >> 1) + pos == size - 19 { mv - 5 } else { mv }
}

/// Moves the vertical component of a motion vector so that the block (and the
/// rows its filter reads below) comes from inside the reference. Only happens
/// to blocks below the picture.
fn clamp_mv_y(mv: i32, y: i32, height: i32) -> i32 {
    let limit = height - 16 - if mv & 1 != 0 { 3 } else { 0 };
    if (mv >> 1) + y > limit { 2 * (limit - y) + (mv & 1) } else { edge_mv(mv, y, height) }
}

/// The half-pel filter over 6 samples `step` apart, around `at`.
fn half<T: Copy + Into<i32>>(s: &[T], at: usize, step: usize) -> i32 {
    let p = |i: usize| -> i32 { s[at + i * step - 2 * step].into() };
    ((((p(2) + p(3)) * 19) >> 1) - (p(1) + p(4)) * 2 + ((p(0) + p(5)) >> 1) + 8) >> 4
}

/// Copies an `N`×`N` area from (`x`, `y`) of the reference, whose border
/// samples continue outside it.
fn fetch<const N: usize>(plane: &Plane, (w, h): (usize, usize), x: i32, y: i32) -> [[u8; N]; N] {
    let mut out = [[0; N]; N];
    let inside = x >= 0 && x as usize + N <= w;
    for (j, row) in out.iter_mut().enumerate() {
        let line = (y + j as i32).clamp(0, h as i32 - 1) as usize * plane.stride;
        if inside {
            row.copy_from_slice(&plane.data[line + x as usize..][..N]);
        } else {
            for (i, v) in row.iter_mut().enumerate() {
                *v = plane.data[line + (x + i as i32).clamp(0, w as i32 - 1) as usize];
            }
        }
    }
    out
}

/// Predicts a 16×16 luma block at (`x`, `y`) moved by a half-pel vector.
fn luma_block(dst: &mut [u8], stride: usize, src: &Plane, size: (usize, usize), x: i32, y: i32, mv: Mv) {
    let src = fetch::<21>(src, size, x + (mv[0] >> 1) - 2, y + (mv[1] >> 1) - 2);
    let rows = dst.chunks_mut(stride).take(16);
    match (mv[0] & 1 != 0, mv[1] & 1 != 0) {
        (false, false) => rows.zip(&src[2..]).for_each(|(d, s)| d[..16].copy_from_slice(&s[2..18])),
        (true, false) => {
            rows.zip(&src[2..])
                .for_each(|(d, s)| d[..16].iter_mut().enumerate().for_each(|(i, d)| *d = clip(half(s, i + 2, 1))));
        }
        (false, true) => {
            let flat = src.as_flattened();
            rows.enumerate().for_each(|(j, d)| {
                d[..16].iter_mut().enumerate().for_each(|(i, d)| *d = clip(half(flat, (j + 2) * 21 + i + 2, 21)));
            });
        }
        (true, true) => {
            let mut t = [[0i16; 16]; 21];
            for (t, s) in t.iter_mut().zip(&src) {
                t.iter_mut().enumerate().for_each(|(i, t)| *t = half(s, i + 2, 1) as i16);
            }
            let flat = t.as_flattened();
            rows.enumerate().for_each(|(j, d)| {
                d[..16].iter_mut().enumerate().for_each(|(i, d)| *d = clip(half(flat, (j + 2) * 16 + i, 16)));
            });
        }
    }
}

/// Predicts an 8×8 chroma block at (`x`, `y`) moved by a quarter-pel vector.
fn chroma_block(dst: &mut [u8], stride: usize, src: &Plane, size: (usize, usize), x: i32, y: i32, mv: Mv) {
    let src = fetch::<9>(src, size, x + (mv[0] >> 2), y + (mv[1] >> 2));
    let (fx, fy) = (mv[0] & 3, mv[1] & 3);
    for (j, d) in dst.chunks_mut(stride).take(8).enumerate() {
        for (i, d) in d[..8].iter_mut().enumerate() {
            let row = |j: usize| (4 - fx) * i32::from(src[j][i]) + fx * i32::from(src[j][i + 1]);
            // One bilinear filter; with a zero fraction the same as rounding
            // the other direction alone.
            *d = (((4 - fy) * row(j) + fy * row(j + 1) + 8) >> 4) as u8;
        }
    }
}

/// The sum over 8 columns of their (pairwise rounded) averages.
fn average_block(src: &[u8], stride: usize) -> i32 {
    let avg = |a: i32, b: i32| (a + b + 1) >> 1;
    (0..8)
        .map(|i| {
            let p = |j: usize| i32::from(src[j * stride + i]);
            let q = |j: usize| avg(avg(p(j), p(j + 1)), avg(p(j + 2), p(j + 3)));
            avg(q(0), q(4))
        })
        .sum()
}

/// The quantiser predicted from the neighbours, plus its delta.
fn quantiser(around: &Around, f: fn(&Mb) -> i32, dq: i32) -> Result<i32> {
    let predicted = match (around.top, around.left) {
        (None, None) => 16,
        (None, Some(l)) => f(l),
        (Some(t), None) => f(t),
        (Some(t), Some(l)) => mid(f(t), f(l), around.corner.map_or(0, f)),
    };
    let q = predicted + dq;
    if (0..DC_PAT.len() as i32).contains(&q) { Ok(q) } else { bad("quantiser out of range") }
}

impl Slice<'_> {
    fn decode(mut self) -> Result<usize> {
        let columns = self.reference.0 / 32;
        let mut above = vec![Mb::default(); columns];
        let mut row = vec![Mb::default(); columns];
        for y in (self.start..self.end).step_by(32) {
            let mut lru = [MOTION, RESIDUE, SKIP, INTRA];
            // Coded block patterns of the macroblock before, intra and inter.
            let mut cbp = [[0u32; 4]; 2];
            for x in 0..columns {
                let top = y > self.start;
                let around = Around {
                    left: (x > 0).then(|| &row[x.wrapping_sub(1)]),
                    top: top.then(|| &above[x]),
                    corner: (top && x > 0).then(|| &above[x.wrapping_sub(1)]),
                };
                let mb = self.macroblock(x, y, &around, &mut lru, &mut cbp)?;
                row[x] = mb;
                if self.bits.overread() {
                    return bad("slice ends early");
                }
            }
            std::mem::swap(&mut row, &mut above);
        }
        Ok(self.bits.left() / 8)
    }

    fn macroblock(
        &mut self,
        x: usize,
        y: usize,
        around: &Around,
        lru: &mut [usize; 4],
        cbp: &mut [[u32; 4]; 2],
    ) -> Result<Mb> {
        let kind = if self.key { INTRA } else { self.kind(lru) };
        let comps = 3 + usize::from(self.has_alpha);
        let (mut mb, intra_dq, inter_dq) = self.predict(kind, x, y, around)?;
        mb.intra_q = quantiser(around, |m| m.intra_q, intra_dq)?;
        mb.inter_q = quantiser(around, |m| m.inter_q, inter_dq)?;

        match kind {
            INTRA => self.intra_residual(x, y, around, &mut mb, &mut cbp[0])?,
            RESIDUE => self.inter_residual(x, y, &mb, &mut cbp[1])?,
            _ => {}
        }

        self.store_motion(x, y, kind, &mb);
        if kind != INTRA {
            self.average_dcs(x, y, comps, &mut mb);
        }
        Ok(mb)
    }

    /// Reads the motion of a macroblock and compensates it; returns the block with the quantiser deltas
    /// (intra, inter).
    fn predict(&mut self, kind: usize, x: usize, y: usize, around: &Around) -> Result<(Mb, i32, i32)> {
        let mut mb = Mb::default();
        let (mut intra_dq, mut inter_dq) = (0, 0);
        match kind {
            INTRA => {
                mb.mv = predict_mvs(around, &[[0; 2]; 4]);
                intra_dq = self.delta_q();
            }
            SKIP => self.compensate(x, y, &mb),
            _ => {
                let (one, deltas) = self.motion_vectors()?;
                mb.mv = if one { [predict_mv(around, deltas[0]); 4] } else { predict_mvs(around, &deltas) };
                if kind == RESIDUE {
                    inter_dq = self.delta_q();
                }
                self.compensate(x, y, &mb);
            }
        }
        Ok((mb, intra_dq, inter_dq))
    }

    fn intra_residual(&mut self, x: usize, y: usize, around: &Around, mb: &mut Mb, cbp: &mut [u32; 4]) -> Result<()> {
        let comps = 3 + usize::from(self.has_alpha);
        for (comp, cbp) in cbp.iter_mut().enumerate().take(comps) {
            mb.dc[comp] = self.intra(x, y, comp, around, mb.intra_q as usize, cbp)?;
        }
        Ok(())
    }

    fn inter_residual(&mut self, x: usize, y: usize, mb: &Mb, cbp: &mut [u32; 4]) -> Result<()> {
        self.inter(x, y, 0, mb.inter_q as usize, &mut cbp[0])?;
        if self.bits.bit() {
            self.inter(x, y, 1, mb.inter_q as usize, &mut cbp[1])?;
            self.inter(x, y, 2, mb.inter_q as usize, &mut cbp[2])?;
        } else {
            (cbp[1], cbp[2]) = (0, 0);
        }
        if self.has_alpha {
            self.inter(x, y, 3, mb.inter_q as usize, &mut cbp[3])?;
        }
        Ok(())
    }

    /// For the loop filter and later predictions.
    fn store_motion(&mut self, x: usize, y: usize, kind: usize, mb: &Mb) {
        let mv_stride = self.reference.0 >> 4;
        for (j, mv) in mb.mv.iter().enumerate() {
            let at = ((y - self.start) / 16 + j / 2) * mv_stride + x * 2 + j % 2;
            self.mvs[at] = if kind == MOTION || kind == RESIDUE { *mv } else { [0; 2] };
        }
    }

    fn average_dcs(&mut self, x: usize, y: usize, comps: usize, mb: &mut Mb) {
        for comp in 0..comps {
            let n = 4 >> shift(comp);
            let stride = self.stride[comp];
            for by in 0..n {
                let info = ((y - self.start) / 32 * n + by) * self.info_stride[comp] + x * n;
                self.info[comp][info..info + n].fill(0);
                for bx in 0..n {
                    let at = self.origin(x, y, comp) + by * 8 * stride + bx * 8;
                    mb.dc[comp][by * n + bx] = average_block(&self.dst[comp][at..], stride);
                }
            }
        }
    }

    /// Where a macroblock starts in this slice's part of a plane.
    fn origin(&self, x: usize, y: usize, comp: usize) -> usize {
        ((y - self.start) >> shift(comp)) * self.stride[comp] + ((x * 32) >> shift(comp))
    }

    fn kind(&mut self, lru: &mut [usize; 4]) -> usize {
        match self.bits.unary(true, 3) {
            0 => lru[0],
            1 => {
                lru.swap(0, 1);
                lru[0]
            }
            2 => {
                lru.swap(2, 3);
                lru[2]
            }
            _ => {
                lru.swap(1, 2);
                lru[1]
            }
        }
    }

    fn delta_q(&mut self) -> i32 {
        let mut dq = self.bits.unary(true, 4) as i32;
        match dq {
            3 => dq += self.bits.read(1) as i32,
            4 => dq += self.bits.read(5) as i32 + 1,
            _ => {}
        }
        if dq != 0 && self.bits.bit() { -dq } else { dq }
    }

    /// Differences to the predicted motion vectors, and whether there is just one.
    fn motion_vectors(&mut self) -> Result<(bool, [Mv; 4])> {
        let one = self.bits.bit();
        let mut out = [[0; 2]; 4];
        for i in 0..2 {
            for mv in out.iter_mut().take(if one { 1 } else { 4 }) {
                let Some(symbol) = self.bits.vlc(&MV_VLC) else { return bad("motion vector code") };
                mv[i] = match i32::from(symbol) {
                    15 => {
                        let n = self.bits.unary(true, 12) + 4;
                        let v = self.bits.read(n) as i32 + (1 << n) - 1;
                        if v & 1 != 0 { -(v >> 1) - 1 } else { v >> 1 }
                    }
                    v if v >= 8 => v - 15,
                    v => v,
                };
            }
        }
        Ok((one, out))
    }

    /// Predicts a macroblock from the frame before.
    fn compensate(&mut self, x: usize, y: usize, mb: &Mb) {
        let (w, h) = self.reference;
        for (j, mv) in mb.mv.iter().enumerate() {
            let (px, py) = (x * 32 + j % 2 * 16, y + j / 2 * 16);
            let mv = [edge_mv(mv[0], px as i32, w as i32), clamp_mv_y(mv[1], py as i32, h as i32)];
            for comp in 0..3 + usize::from(self.has_alpha) {
                let s = shift(comp);
                let stride = self.stride[comp];
                let at = self.origin(x, y, comp) + ((j / 2 * 16) >> s) * stride + ((j % 2 * 16) >> s);
                let dst = &mut self.dst[comp][at..];
                let block = if s == 0 { luma_block } else { chroma_block };
                let size = (w >> s, h >> s);
                block(dst, stride, &self.last[comp], size, (px >> s) as i32, (py >> s) as i32, mv);
            }
        }
    }

    /// Which 8×8 blocks have coefficients (low half) and which AC run codes
    /// they use (high half).
    fn cbp_luma(&mut self, before: u32) -> u32 {
        let mut ones = (before & 0xFFFF).count_ones();
        let mut mask = 0;
        if ones > 7 {
            ones = 16 - ones;
            mask = 0xFFFF;
        }
        let mut cbp = 0;
        if !self.bits.bit() {
            if ones < 4 {
                for j in (0..16).step_by(4) {
                    if !self.bits.bit() {
                        cbp |= self.bits.read(4) << j;
                    }
                }
            } else {
                cbp = self.bits.read(16);
            }
        }
        cbp ^= mask;
        if (self.flags & PLAIN_RESIDUAL == 0 || cbp != 0) && self.bits.bit() {
            cbp |= cbp << 16;
        }
        cbp
    }

    fn cbp_chroma(&mut self, before: u32) -> u32 {
        if self.bits.bit() {
            let most = (before & 0xF).count_ones() >= 2;
            return (before & 0xF_0000) | if most { 0xF } else { 0 };
        }
        let cbp = self.bits.read(4);
        if self.bits.bit() { cbp | cbp << 16 } else { cbp }
    }

    fn dc_differences(&mut self, n: usize, q: usize) -> [i32; 16] {
        let mut out = [0; 16];
        if self.bits.bit() {
            let pat = i64::from(DC_PAT[q.max(8)]);
            for out in &mut out[..n * n] {
                let mut v = self.bits.unary(false, 12);
                if v > 3 {
                    v = (1 << (v - 3)) + self.bits.read(v - 3) + 2;
                }
                if v != 0 {
                    let scaled = ((i64::from(v) * pat + 0x200) >> 10) as i32;
                    *out = if self.bits.bit() { -scaled } else { scaled };
                }
            }
        }
        out
    }

    /// Reads the AC coefficients of four blocks. Gives how few each has: 0
    /// (8 or more) to 3 (none).
    fn coefficients(
        &mut self,
        blocks: &mut [[i16; 64]; 4],
        cbp: u32,
        q: usize,
        qmat: &[[u16; 64]; 4],
    ) -> Result<[u8; 4]> {
        *blocks = [[0; 64]; 4];
        let mut few = [3; 4];
        if cbp & 0xF == 0 {
            return Ok(few);
        }
        let qmat = &qmat[q & 3];
        let factor = 1 << (q >> 2);
        let table = &SKIP_VLC[usize::from(cbp & 0xFFFF_0000 != 0)];
        for (i, block) in blocks.iter_mut().enumerate() {
            if (cbp >> i) & 1 == 0 {
                continue;
            }
            let (mut at, mut next, mut count) = (1, 0, 0);
            while at < 64 {
                next -= 1;
                if next < 1 {
                    let Some(symbol) = self.bits.vlc(table) else { return bad("coefficient code") };
                    next = if symbol == 13 { 7 } else { 0 };
                    at += match symbol {
                        11 => self.bits.read(6) as usize,
                        12 => 64,
                        13 => 0,
                        n => n as usize,
                    };
                    if at >= 64 {
                        break;
                    }
                }
                let mut v = self.bits.unary(false, 12) as i32 + 1;
                if v > 3 {
                    v = self.bits.read(v as u32 - 3) as i32 + (1 << (v - 3)) + 2;
                }
                if self.bits.bit() {
                    v = -v;
                }
                let pos = SCAN[at] as usize;
                let v = v.wrapping_mul(i32::from(qmat[pos])).wrapping_mul(factor);
                block[pos] = wrap(v.wrapping_add(64) >> 7);
                count += 1;
                at += 1;
            }
            few[i] = u8::from(count < 8) + u8::from(count < 4) + u8::from(count < 1);
        }
        Ok(few)
    }

    /// Decodes a component of an intra macroblock; gives its DCs.
    fn intra(
        &mut self,
        x: usize,
        y: usize,
        comp: usize,
        around: &Around,
        q: usize,
        cbp: &mut u32,
    ) -> Result<[i32; 16]> {
        let n = 4 >> shift(comp);
        *cbp = if n == 4 { self.cbp_luma(*cbp) } else { self.cbp_chroma(*cbp) };
        let mut diff = self.dc_differences(n, q);
        if around.top.is_none() && around.left.is_none() {
            diff[0] += 1024;
        }
        let dcs = |m: Option<&'_ Mb>| m.map(|m| m.dc[comp]);
        let (left, top, corner) = (dcs(around.left), dcs(around.top), dcs(around.corner));
        let dc = predict_dcs(&diff, n, (0, 2047), left.as_ref(), top.as_ref(), corner.as_ref());
        let qmat = if n == 4 { &LUMA_INTRA } else { &CHROMA_INTRA };
        let mut blocks = [[0; 64]; 4];
        let stride = self.stride[comp];
        for i in 0..n * n / 4 {
            let few = self.coefficients(&mut blocks, *cbp >> (4 * i), q, qmat)?;
            for (j, block) in blocks.iter_mut().enumerate() {
                let pos = if n == 4 { LUMA_POS[i * 4 + j] as usize } else { j };
                block[0] = (dc[pos] * 8 + 32) as i16;
                idct(block);
                let at = self.origin(x, y, comp) + pos / n * 8 * stride + pos % n * 8;
                for (d, s) in self.dst[comp][at..].chunks_mut(stride).zip(block.as_chunks::<8>().0) {
                    d[..8].iter_mut().zip(s).for_each(|(d, &s)| *d = clip(i32::from(s)));
                }
                let info = ((y - self.start) / 32 * n + pos / n) * self.info_stride[comp] + x * n + pos % n;
                self.info[comp][info] = 0x8000 | u16::from(few[j]) << 13 | dc[pos] as u16;
            }
        }
        Ok(dc)
    }

    /// Adds the residual of a component to the prediction.
    fn inter(&mut self, x: usize, y: usize, comp: usize, q: usize, cbp: &mut u32) -> Result<()> {
        let n = 4 >> shift(comp);
        *cbp = if n == 4 { self.cbp_luma(*cbp) } else { self.cbp_chroma(*cbp) };
        let limit = if self.flags & PLAIN_RESIDUAL != 0 { 2047 } else { 1023 };
        let diff = self.dc_differences(n, q);
        let dc = predict_dcs(&diff, n, (-limit, limit), None, None, None);
        let mut residual = [0i16; 32 * 32];
        let size = n * 8;
        let mut few = [0; 16];
        let mut blocks = [[0; 64]; 4];
        for i in 0..n * n / 4 {
            let counts = self.coefficients(&mut blocks, *cbp >> (4 * i), q, &INTER)?;
            for (j, block) in blocks.iter_mut().enumerate() {
                let pos = if n == 4 { LUMA_POS[i * 4 + j] as usize } else { j };
                block[0] = (dc[pos] * 8 + 32) as i16;
                idct(block);
                few[pos] = counts[j];
                let at = pos / n * 8 * size + pos % n * 8;
                for (d, s) in residual[at..].chunks_mut(size).zip(block.as_chunks::<8>().0) {
                    d[..8].copy_from_slice(s);
                }
            }
        }
        const NO_EDGES: u32 = NO_ROW_EDGES | NO_COLUMN_EDGES;
        if self.flags & PLAIN_RESIDUAL == 0 && self.flags & NO_EDGES != NO_EDGES {
            self.filter_residual(&mut residual[..size * size], n, &few, &dc);
        }
        let stride = self.stride[comp];
        let at = self.origin(x, y, comp);
        for (d, s) in self.dst[comp][at..].chunks_mut(stride).zip(residual.chunks_exact(size).take(size)) {
            d[..size].iter_mut().zip(s).for_each(|(d, &s)| *d = clip(i32::from(*d) + i32::from(s)));
        }
        Ok(())
    }

    /// Filters a residual across the block edges inside the macroblock,
    /// `n`×`n` blocks.
    fn filter_residual(&self, residual: &mut [i16], n: usize, few: &[u8; 16], dc: &[i32; 16]) {
        let size = n * 8;
        for vertical in [true, false] {
            if self.flags & if vertical { NO_COLUMN_EDGES } else { NO_ROW_EDGES } != 0 {
                continue;
            }
            let (dx, dy) = (usize::from(vertical), usize::from(!vertical));
            for by in dy..n {
                for bx in dx..n {
                    let (a, b) = ((by - dy) * n + bx - dx, by * n + bx);
                    let (sa, sb) = match (few[a], few[b]) {
                        (3, 3) if (dc[a] - dc[b]).abs() <= 32 => continue,
                        (3, 3) => (2, 2),
                        (0, 0) => continue,
                        s => s,
                    };
                    let (step, next) = if vertical { (1, size) } else { (size, 1) };
                    filter_edge!(residual, by * 8 * size + bx * 8, step, next, sa, sb, wrap);
                }
            }
        }
    }
}

/// The loop filter. Its result is shown and predicted from.
struct Filter<'a> {
    flags: u32,
    /// Rows and columns of edges to leave alone.
    edges: &'a [Vec<bool>; 2],
    mvs: &'a [Mv],
    mv_stride: usize,
}

impl Filter<'_> {
    fn plane(&self, plane: &mut Plane, blocks: &Blocks, comp: usize) {
        let threshold = (self.flags & 0xFF) as i32 * 8;
        let stride = plane.stride;
        for (i, &info) in blocks.info.iter().enumerate() {
            if info & 0xE000 == 0xE000 {
                let (bx, by) = (i % blocks.stride, i / blocks.stride);
                smooth_block(&mut plane.data[by * 8 * stride + bx * 8..], stride, blocks, bx, by, threshold);
            }
        }
        // Flat alpha blocks are smoothed, but alpha edges not filtered.
        if comp < 3 {
            if self.flags & NO_COLUMN_EDGES == 0 {
                self.block_edges(plane, blocks, comp, true);
            }
            if self.flags & NO_ROW_EDGES == 0 {
                self.block_edges(plane, blocks, comp, false);
            }
        }
    }

    fn block_edges(&self, plane: &mut Plane, blocks: &Blocks, comp: usize, vertical: bool) {
        let s = shift(comp);
        let rows = blocks.info.len() / blocks.stride;
        let (dx, dy) = (usize::from(vertical), usize::from(!vertical));
        let off = |list: &[bool], i: usize| list.get(i).copied().unwrap_or(true);
        for by in dy..rows {
            for bx in dx..blocks.stride {
                let skip = if vertical {
                    off(&self.edges[1], (bx << s) - 1)
                } else if comp == 0 && by == rows - 1 {
                    // The flag of the last luma row does not apply.
                    by > self.edges[0].len()
                } else {
                    off(&self.edges[0], (by << s) - 1)
                };
                if skip {
                    continue;
                }
                let (ax, ay) = (bx - dx, by - dy);
                let (a, b) = (blocks.info[ay * blocks.stride + ax], blocks.info[by * blocks.stride + bx]);
                let strength = |i: u16| if i & 0x8000 != 0 { (i >> 13) & 3 } else { 0 };
                let (mut sa, mut sb) = (strength(a), strength(b));
                if (a | b) & 0x8000 == 0 {
                    let mv = |x: usize, y: usize| self.mvs[((y << s) >> 1) * self.mv_stride + ((x << s) >> 1)];
                    let (va, vb) = (mv(ax, ay), mv(bx, by));
                    if (va[0] - vb[0]).abs() >= 2 || (va[1] - vb[1]).abs() >= 2 {
                        (sa, sb) = (2, 2);
                    }
                }
                if sa == sb && (sa == 0 || sa == 3) {
                    continue;
                }
                let at = by * 8 * plane.stride + bx * 8;
                let (step, next) = if vertical { (1, plane.stride) } else { (plane.stride, 1) };
                filter_edge!(plane.data, at, step, next, sa, sb, clip);
            }
        }
    }
}

fn smooth_weight(d: i32, i: usize) -> i32 {
    match i {
        0 => (d * 26624) >> 16,
        1 => (d * 18432) >> 16,
        2 => (d * 12288) >> 16,
        3 => (d * 7168) >> 16,
        4 => d >> 4,
        5 => d >> 5,
        6 => d >> 7,
        _ => 0,
    }
}

/// Replaces an intra block without AC coefficients by a smooth surface
/// through the DCs of its neighbours that are similar flat intra blocks.
fn smooth_block(dst: &mut [u8], stride: usize, blocks: &Blocks, bx: usize, by: usize, threshold: i32) {
    let (bw, bh) = (blocks.stride, blocks.info.len() / blocks.stride);
    let center = blocks.info[by * bw + bx];
    let dc = |info: u16| i32::from(info & 0x1FFF);
    let mut g = [[None; 3]; 3];
    for (dy, row) in g.iter_mut().enumerate() {
        for (dx, g) in row.iter_mut().enumerate() {
            let (x, y) = ((bx + dx).wrapping_sub(1), (by + dy).wrapping_sub(1));
            if x < bw && y < bh {
                let n = blocks.info[y * bw + x];
                if i32::from(n.wrapping_sub(center) as i16).abs() < threshold {
                    *g = Some(dc(n));
                }
            }
        }
    }
    let c = dc(center);
    let (left, right) = (g[1][0].unwrap_or(c), g[1][2].unwrap_or(c));
    let g = [0, 1, 2].map(|r| [g[r][0].unwrap_or(left), g[r][1].unwrap_or(c), g[r][2].unwrap_or(right)]);
    let g = [g[0], [left, c, right], g[2]];
    let h = g.map(|g| -> [i32; 8] {
        std::array::from_fn(|i| g[1] + 4 + smooth_weight(g[0] - g[1], i) + smooth_weight(g[2] - g[1], 7 - i))
    });
    for (j, row) in dst.chunks_mut(stride).take(8).enumerate() {
        for (i, d) in row[..8].iter_mut().enumerate() {
            let mid = h[1][i];
            let v = mid + smooth_weight(h[0][i] - mid, j) + smooth_weight(h[2][i] - mid, 7 - j);
            *d = clip(v >> 3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes bits, least significant first.
    #[derive(Default)]
    struct Writer(Vec<bool>);

    impl Writer {
        fn put(&mut self, v: u32, n: u32) {
            (0..n).for_each(|i| self.0.push((v >> i) & 1 != 0));
        }

        fn bytes(&self) -> Vec<u8> {
            let byte = |c: &[bool]| c.iter().enumerate().fold(0, |a, (i, &b)| a | u8::from(b) << i);
            self.0.chunks(8).map(byte).collect()
        }
    }

    /// A key frame of one macroblock: no coefficients, luma DC differences
    /// `luma` (quantiser index 8 steps, coded as unary + sign).
    fn key_frame(luma: &[u32; 16]) -> Vec<u8> {
        let mut w = Writer::default();
        w.put(PLAIN_RESIDUAL | NO_ROW_EDGES | NO_COLUMN_EDGES, 32);
        w.put(1, 1); // quantiser unchanged
        w.put(1, 1); // no coefficients
        w.put(1, 1); // DC differences follow
        for &v in luma {
            w.put((1 << v) - 1, v + 1);
            if v > 0 {
                w.put(0, 1);
            }
        }
        for _ in 0..2 {
            w.put(1, 1); // pattern from before
            w.put(0, 1); // no DC differences
        }
        w.bytes()
    }

    fn decoder() -> Decoder {
        Decoder::new(b'g', 0, 32, 32).unwrap()
    }

    #[test]
    fn flat_key_frame() {
        let mut d = decoder();
        assert!(d.decode(&key_frame(&[0; 16]), false).is_err(), "needs a key frame");
        assert_eq!(d.decode(&key_frame(&[0; 16]), true).unwrap(), [0]);
        let f = d.frame();
        assert!(f.y.iter().chain(&f.cb).chain(&f.cr).all(|&v| v == 128));
        assert_eq!((f.y.len(), f.cb.len(), f.a), (1024, 256, None));
    }

    #[test]
    fn dc_difference_moves_the_whole_block() {
        // DC_PAT[16] = 16384: one step is 16, which is 2 in samples.
        let mut first = [0; 16];
        first[0] = 2;
        let mut d = decoder();
        d.decode(&key_frame(&first), true).unwrap();
        // Every other block is predicted from the first.
        assert!(d.frame().y.iter().all(|&v| v == 132));
    }

    #[test]
    fn skipped_frame_repeats() {
        let mut first = [0; 16];
        first[5] = 3;
        let mut d = decoder();
        d.decode(&key_frame(&first), true).unwrap();
        let before = d.frame();
        let mut w = Writer::default();
        w.put(PLAIN_RESIDUAL | NO_ROW_EDGES | NO_COLUMN_EDGES, 32);
        w.put(0b000, 3); // the third of the list: skip
        assert_eq!(d.decode(&w.bytes(), false).unwrap(), [0]);
        assert!(d.frame().y == before.y);
    }

    #[test]
    fn moved_frame() {
        let mut first = [0; 16];
        first[0] = 3;
        let mut d = decoder();
        d.decode(&key_frame(&first), true).unwrap();
        let mut w = Writer::default();
        w.put(PLAIN_RESIDUAL | NO_ROW_EDGES | NO_COLUMN_EDGES, 32);
        w.put(1, 1); // motion
        w.put(1, 1); // one vector
        w.put(0x06, 3); // +1 half sample
        w.put(0x01, 1); // 0
        d.decode(&w.bytes(), false).unwrap();
        assert_eq!(d.mvs, [[1, 0]; 4]);
        // Flat, so it looks the same.
        assert!(d.frame().y.iter().all(|&v| v == 134));
    }

    #[test]
    fn idct_of_dc_is_flat() {
        let mut b = [0; 64];
        b[0] = 100 * 64 + 32;
        idct(&mut b);
        assert_eq!(b, [100; 64]);
    }

    #[test]
    fn run_list() {
        let mut w = Writer::default();
        w.put(1, 1); // runs
        w.put(1, 1); // a run …
        w.put(1, 1); // … of ones
        w.put(3, 5); // 5 + 2 + 3
        w.put(0, 1); // plain: starts with the opposite
        w.put(0b1010, 4);
        let bytes = w.bytes();
        let list = flag_list(&mut Bits::new(&bytes), 17);
        assert_eq!(list[..10], [true; 10]);
        assert_eq!(list[10..15], [false, false, true, false, true]);
    }

    #[test]
    fn slices() {
        let ends = |flags, h| Decoder::new(b'j', flags, 64, h).unwrap().slice_ends();
        for flags in 0..4 {
            for h in [1, 32, 33, 64, 512, 1080, 2160] {
                let e = ends(flags, h);
                assert_eq!(e.len(), [2, 3, 4, 8][flags as usize]);
                assert!(e.is_sorted() && e.iter().all(|v| v % 32 == 0), "{e:?}");
                assert_eq!(*e.last().unwrap(), h.next_multiple_of(32) as usize);
            }
        }
        assert_eq!(ends(2, 512), [128, 256, 384, 512]);
    }

    /// Damaged frames give an error or a picture, never a panic.
    #[test]
    fn garbage() {
        let mut seed = 1u32;
        for round in 0..60 {
            let mut d = Decoder::new(b'j', ALPHA | (round % 4), 33 + round * 3, 40 + round).unwrap();
            for frame in 0..4 {
                let mut data: Vec<u8> = (0..4000)
                    .map(|_| {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (seed >> 24) as u8
                    })
                    .collect();
                let n = d.slice_ends().len();
                for i in 1..n {
                    data[i * 4..i * 4 + 4].copy_from_slice(&((i * 4000 / n) as u32).to_le_bytes());
                }
                let _ = d.decode(&data, frame == 0);
            }
        }
    }
}
