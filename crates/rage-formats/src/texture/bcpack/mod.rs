//! BCPack: the packing of block-compressed textures on the Xbox Series
//! consoles (GTA VI's texture pools use it for about a tenth of their data).
//!
//! BCPack does not replace the block formats: it takes the fields of each
//! BC1 to BC7 block apart, predicts them from earlier blocks and codes what
//! is left with an entropy coder. Unpacking gives ordinary blocks again.
//!
//! - The entropy layer: 12 or 16 *lanes*, each a stream of symbols of a few
//!   bits, coded with rANS (16-bit probabilities, bytes fed in as the state
//!   runs low). The lanes' bytes are interleaved the way the hardware reads
//!   them: at most eight bytes per cycle, handed to the lanes that need one
//!   in lane order. A lane's stream comes in pieces, each with a header
//!   (quality of the frequency model, symbol width, byte count).
//! - Per block format, a rule that turns one symbol per lane back into a
//!   block: endpoints as differences to an earlier block (up to 255 back; a
//!   reference before the first block reads a built-in history), indices
//!   through rank tables.
//!
//! Read: BC1, BC3, BC4 and BC5. Not read: BC6H and BC7. Not every entry of
//! the built-in histories is known; a chunk that needs an unknown one is not
//! unpacked (BC3 and BC5 tolerate a few such blocks, which then start from
//! the format's neutral endpoints).
//!
//! This follows `tools/bcpack.py` of the Leonida project (GPL-3.0), a
//! clean-room description of the bitstream (`docs/textures.md` there); its
//! entropy layer is a port of the decoder in Microsoft's silicon-rans (MIT).

mod lanes;
mod tables;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum BcpackError {
    #[error("damaged BCPack data: {0}")]
    Damaged(String),
    #[error("BCPack mode {0} is not read yet")]
    Mode(u8),
    #[error("the BCPack data refers to an unknown built-in history entry ({0})")]
    History(usize),
}

fn damaged(msg: impl Into<String>) -> BcpackError {
    BcpackError::Damaged(msg.into())
}

const MAX_WIDTH: usize = 10;

/// Widths of the lanes' symbols, in stream order.
fn lane_widths(mode: u8) -> Option<&'static [u8]> {
    Some(match mode {
        1 => &[8, 8, 5, 6, 5, 5, 6, 5, 8, 8, 8, 8],
        3 => &[4, 4, 8, 6, 6, 6, 6, 5, 6, 5, 8, 8],
        4 => &[8, 8, 8, 8, 6, 6, 6, 6, 6, 6, 6, 6],
        5 => &[4, 4, 8, 8, 6, 6, 6, 6, 6, 6, 6, 6],
        _ => return None,
    })
}

/// Whether [`unpack`] reads this mode (the codec number of a pool's chunk).
pub fn reads_mode(mode: u8) -> bool {
    lane_widths(mode).is_some()
}

/// A frequency model: for each band its weight per symbol and where the
/// band starts in the 16-bit range.
struct Model {
    width: usize,
    freq: &'static [u16],
    base: [u32; MAX_WIDTH + 1],
}

impl Model {
    fn get(width: usize, quality: usize) -> Option<Model> {
        let freq = *tables::MODELS.get(width.checked_sub(1)?)?.get(quality)?;
        if freq.len() != width + 1 {
            return None;
        }
        let mut base = [0u32; MAX_WIDTH + 1];
        let mut total = 0u32;
        for (band, f) in freq.iter().enumerate() {
            base[band] = total;
            total += u32::from(*f) * if band == 0 { 1 } else { 1 << (band - 1) };
        }
        (total == 65536).then_some(Model { width, freq, base })
    }

    /// The symbol whose range holds `low` (the state's low 16 bits).
    fn lookup(&self, low: u32) -> u32 {
        let mut band = self.width;
        while band > 0 && low < self.base[band] {
            band -= 1;
        }
        if band == 0 { 0 } else { (1 << (band - 1)) + (low - self.base[band]) / u32::from(self.freq[band]) }
    }

    /// The state after taking symbol `s` out of state `x`.
    fn advance(&self, s: u32, x: u64) -> u64 {
        let band = (u32::BITS - s.leading_zeros()) as usize;
        let mut f = u64::from(self.freq[band]);
        let mut c = u64::from(self.base[band]) + f * u64::from(s - if band == 0 { 0 } else { 1 << (band - 1) });
        if c == 0 {
            (f, c) = (f - 1, 1);
        }
        (f * (x >> 16) + (x & 0xFFFF)).wrapping_sub(c)
    }
}

/// A difference stored without a sign: 0, -1, 1, -2, 2, …
fn unzig(v: u32) -> i32 {
    if v & 1 != 0 { -(((v + 1) >> 1) as i32) } else { (v >> 1) as i32 }
}

/// The bits of two 4-bit symbols, taken in turn.
fn interleave(a: u32, b: u32) -> u32 {
    (0..4).fold(0, |d, k| d | (a >> k & 1) << (2 * k) | (b >> k & 1) << (2 * k + 1))
}

fn known<const N: usize>(table: &[(u8, [u8; N])], entry: usize) -> Option<[i32; N]> {
    let i = table.binary_search_by_key(&entry, |(k, _)| usize::from(*k)).ok()?;
    Some(table[i].1.map(i32::from))
}

/// How many blocks of a chunk may start from an unknown history entry.
fn tolerated(blocks: usize) -> usize {
    (blocks / 1024).max(2)
}

/// A table symbol; a symbol the table does not have is damage.
fn ranked<T: Copy>(table: &[T], symbol: u32) -> Result<T, BcpackError> {
    table.get(symbol as usize).copied().ok_or_else(|| damaged("an index symbol outside its table"))
}

/// Four rows of 3-bit indices (48 bits) from two symbols per row.
fn index_rows(pairs: &[(u8, u8)], row: impl Fn(usize) -> (u32, u32)) -> Result<[u8; 6], BcpackError> {
    let mut bits = 0u64;
    for r in 0..4 {
        let (left, right) = row(r);
        let (left, right) = (ranked(pairs, left)?, ranked(pairs, right)?);
        let twelve = u64::from(left.0) | u64::from(left.1) << 3 | u64::from(right.0) << 6 | u64::from(right.1) << 9;
        bits |= twelve << (12 * r);
    }
    let mut out = [0u8; 6];
    out.copy_from_slice(&bits.to_le_bytes()[..6]);
    Ok(out)
}

/// The four index bytes of a colour block from four quadrant bytes (each
/// holds the 2-bit indices of the pixels top left, top right, bottom left,
/// bottom right of a quadrant).
fn colour_indices(a: u8, b: u8, c: u8, d: u8) -> [u8; 4] {
    [(a & 15) | (b & 15) << 4, (a >> 4) | (b >> 4) << 4, (c & 15) | (d & 15) << 4, (c >> 4) | (d >> 4) << 4]
}

fn colours(e: &[i32]) -> [u8; 4] {
    let (c0, c1) = ((e[0] << 11 | e[1] << 5 | e[2]) as u16, (e[3] << 11 | e[4] << 5 | e[5]) as u16);
    let (a, b) = (c0.to_le_bytes(), c1.to_le_bytes());
    [a[0], a[1], b[0], b[1]]
}

/// The distance back to the block a block's endpoints build on, for the
/// formats with one symbol per block.
fn distance(v: u32) -> usize {
    (if v & 1 != 0 { 256 - ((v + 1) >> 1) } else { v >> 1 }) as usize
}

fn bc1(lanes: &[Vec<u32>], blocks: usize) -> Result<Vec<u8>, BcpackError> {
    const MODS: [i32; 6] = [32, 64, 32, 32, 64, 32];
    let mut out = Vec::with_capacity(blocks * 8);
    let mut history: Vec<[i32; 6]> = Vec::with_capacity(blocks);
    for i in 0..blocks {
        let d = distance(lanes[0][i]);
        let base = match d {
            0 => [0; 6],
            d if d <= i => history[i - d],
            d => known(&tables::BC1_HISTORY, 256 + i - d).ok_or(BcpackError::History(256 + i - d))?,
        };
        let e: [i32; 6] = std::array::from_fn(|k| (base[k] + unzig(lanes[2 + k][i])).rem_euclid(MODS[k]));
        history.push(e);
        let quadrant = |lane: usize| ranked(&tables::BC1_QUADRANT, lanes[lane][i]);
        out.extend_from_slice(&colours(&e));
        out.extend_from_slice(&colour_indices(quadrant(8)?, quadrant(9)?, quadrant(10)?, quadrant(11)?));
    }
    Ok(out)
}

fn bc4(lanes: &[Vec<u32>], blocks: usize) -> Result<Vec<u8>, BcpackError> {
    let mut out = Vec::with_capacity(blocks * 8);
    let mut history: Vec<[i32; 2]> = Vec::with_capacity(blocks);
    for i in 0..blocks {
        let d = distance(lanes[0][i]);
        let base = match d {
            0 => [128; 2],
            d if d <= i => history[i - d],
            d => known(&tables::BC4_HISTORY, 256 + i - d).ok_or(BcpackError::History(256 + i - d))?,
        };
        let e = [(base[0] + unzig(lanes[2][i])) & 255, (base[1] + unzig(lanes[3][i])) & 255];
        history.push(e);
        out.extend_from_slice(&[e[0] as u8, e[1] as u8]);
        out.extend_from_slice(&index_rows(&tables::BC4_PAIR, |r| (lanes[4 + 2 * r][i], lanes[5 + 2 * r][i]))?);
    }
    Ok(out)
}

/// Where a block of the two-symbol formats starts from: an earlier block,
/// a built-in entry, or (for a few blocks) the neutral endpoints.
fn base_of<const N: usize>(
    table: &[(u8, [u8; N])],
    neutral: [i32; N],
    history: &[[i32; N]],
    lost: &mut Vec<bool>,
    i: usize,
    d: usize,
) -> [i32; N] {
    match d {
        0 => {
            lost.push(false);
            neutral
        }
        d if d <= i => {
            lost.push(lost[i - d]);
            history[i - d]
        }
        d => {
            let entry = known(table, 256 + i - d);
            lost.push(entry.is_none());
            entry.unwrap_or(neutral)
        }
    }
}

fn check_lost(lost: &[bool]) -> Result<(), BcpackError> {
    let count = lost.iter().filter(|l| **l).count();
    if count > tolerated(lost.len()) { Err(BcpackError::History(count)) } else { Ok(()) }
}

fn bc3(lanes: &[Vec<u32>], blocks: usize) -> Result<Vec<u8>, BcpackError> {
    const MODS: [i32; 8] = [256, 256, 32, 64, 32, 32, 64, 32];
    const NEUTRAL: [i32; 8] = [128, 128, 0, 0, 0, 0, 0, 0];
    let mut out = Vec::with_capacity(blocks * 16);
    let mut history: Vec<[i32; 8]> = Vec::with_capacity(blocks);
    let mut lost = Vec::with_capacity(blocks);
    for i in 0..blocks {
        let (a, b) = (2 * i, 2 * i + 1);
        let d = (unzig(interleave(lanes[0][a], lanes[0][b])) & 255) as usize;
        let base = base_of(&tables::BC3_HISTORY, NEUTRAL, &history, &mut lost, i, d);
        let diffs =
            [lanes[2][a], lanes[2][b], lanes[7][a], lanes[8][a], lanes[9][a], lanes[7][b], lanes[8][b], lanes[9][b]];
        let e: [i32; 8] = std::array::from_fn(|k| (base[k] + unzig(diffs[k])).rem_euclid(MODS[k]));
        history.push(e);
        out.extend_from_slice(&[e[0] as u8, e[1] as u8]);
        out.extend_from_slice(&index_rows(&tables::BC3_ALPHA_PAIR, |r| (lanes[3 + r][a], lanes[3 + r][b]))?);
        out.extend_from_slice(&colours(&e[2..]));
        let quadrant = |lane: usize, at: usize| ranked(&tables::BC3_COLOUR_QUADRANT, lanes[lane][at]);
        out.extend_from_slice(&colour_indices(quadrant(10, a)?, quadrant(10, b)?, quadrant(11, a)?, quadrant(11, b)?));
    }
    check_lost(&lost)?;
    Ok(out)
}

fn bc5(lanes: &[Vec<u32>], blocks: usize) -> Result<Vec<u8>, BcpackError> {
    let mut out = Vec::with_capacity(blocks * 16);
    let mut history: Vec<[i32; 4]> = Vec::with_capacity(blocks);
    let mut lost = Vec::with_capacity(blocks);
    for i in 0..blocks {
        let (a, b) = (2 * i, 2 * i + 1);
        let d = (unzig(interleave(lanes[0][a], lanes[0][b])) & 255) as usize;
        let base = base_of(&tables::BC5_HISTORY, [128; 4], &history, &mut lost, i, d);
        let diffs = [lanes[2][a], lanes[2][b], lanes[3][a], lanes[3][b]];
        let e: [i32; 4] = std::array::from_fn(|k| (base[k] + unzig(diffs[k])) & 255);
        history.push(e);
        for channel in 0..2 {
            out.extend_from_slice(&[e[2 * channel] as u8, e[2 * channel + 1] as u8]);
            let rows = |r: usize| (lanes[4 + 4 * channel + r][a], lanes[4 + 4 * channel + r][b]);
            out.extend_from_slice(&index_rows(&tables::BC5_PAIR, rows)?);
        }
    }
    check_lost(&lost)?;
    Ok(out)
}

/// Unpacks a chunk to `size` bytes of blocks. `mode` is the block format:
/// 1 BC1, 3 BC3, 4 BC4, 5 BC5.
pub fn unpack(raw: &[u8], mode: u8, size: usize) -> Result<Vec<u8>, BcpackError> {
    let widths = lane_widths(mode).ok_or(BcpackError::Mode(mode))?;
    let block = if matches!(mode, 1 | 4) { 8 } else { 16 };
    if !size.is_multiple_of(block) {
        return Err(damaged("the unpacked size is not whole blocks"));
    }
    // Every lane gives one symbol per eight bytes of blocks.
    let lanes = lanes::decode_lanes(raw, widths, size / 8)?;
    if lanes[1].iter().any(|s| *s != 0) {
        return Err(damaged("the second lane is not empty"));
    }
    let blocks = size / block;
    match mode {
        1 => bc1(&lanes, blocks),
        3 => bc3(&lanes, blocks),
        4 => bc4(&lanes, blocks),
        _ => bc5(&lanes, blocks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_models_fill_their_range() {
        let mut models = 0;
        for width in 1..=MAX_WIDTH {
            for quality in 0..16 {
                if tables::MODELS[width - 1][quality].is_empty() {
                    continue;
                }
                let model = Model::get(width, quality).expect("a model that sums to 65536");
                models += 1;
                // Every symbol is found again from the start of its range.
                for s in 0..1u32 << width {
                    let band = (u32::BITS - s.leading_zeros()) as usize;
                    if model.freq[band] == 0 {
                        continue;
                    }
                    let start = model.base[band]
                        + u32::from(model.freq[band]) * (s - if band == 0 { 0 } else { 1 << (band - 1) });
                    assert_eq!(model.lookup(start), s, "width {width}, quality {quality}");
                }
            }
        }
        assert_eq!(models, 160);
    }

    #[test]
    fn small_helpers() {
        assert_eq!([0, 1, 2, 3, 4].map(unzig), [0, -1, 1, -2, 2]);
        assert_eq!([0, 1, 2, 3, 255].map(distance), [0, 255, 1, 254, 128]);
        assert_eq!(interleave(0b1111, 0), 0b0101_0101);
        assert_eq!(interleave(0, 0b1001), 0b1000_0010);
        assert_eq!(colour_indices(0x1B, 0, 0, 0xE4), [0x0B, 0x01, 0x40, 0xE0]);
        assert_eq!(known(&tables::BC1_HISTORY, 255), Some([0, 0, 1, 0, 0, 0]));
        assert_eq!(known(&tables::BC4_HISTORY, 0), None);
        assert_eq!((tolerated(100), tolerated(4096)), (2, 4));
    }

    #[test]
    fn rejects_what_is_not_bcpack() {
        assert_eq!(unpack(&[], 7, 16), Err(BcpackError::Mode(7)));
        assert!(matches!(unpack(&[], 1, 12), Err(BcpackError::Damaged(_))));
        assert!(matches!(unpack(&[0xFF; 4], 1, 8), Err(BcpackError::Damaged(_))));
        assert_eq!(unpack(&[], 1, 0), Ok(Vec::new()));
        assert!(reads_mode(5) && !reads_mode(6) && !reads_mode(0) && !reads_mode(8));
    }
}
