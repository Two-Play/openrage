//! The entropy layer of BCPack: the lane decoder.

use super::{BcpackError, MAX_WIDTH, Model, damaged};

/// The state a lane's piece ends at, and below which a byte is fed in.
const LOW: u64 = 1 << 24;
/// Bytes handed out per cycle.
const BYTES_PER_CYCLE: usize = 8;
const MAX_VIRTUAL_ZEROS: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The three header bytes of a piece, then its symbols.
    Header0,
    Header1,
    Header2,
    Symbols,
}

/// One lane of the entropy decoder. The fields mirror the registers of the
/// hardware decoder this is modelled on.
struct Lane {
    step: Step,
    /// Symbols still to come.
    left: usize,
    width: usize,
    quality: usize,
    long: bool,
    flush: bool,
    done: bool,
    /// A byte is waiting in `byte`.
    full: bool,
    /// A symbol is decoded and waits to be released.
    held: bool,
    zeros: u8,
    byte: u8,
    symbol: u32,
    bytes_left: u32,
    state_in: u64,
    state: u64,
    model: Option<Model>,
    slot: Option<u32>,
    stalled: bool,
    out: Vec<u32>,
}

fn home() -> BcpackError {
    damaged("a piece of a lane does not end at its home state")
}

/// Whether a lane waits for its next input byte.
#[inline]
fn wants_byte(l: &Lane) -> bool {
    !l.full && (l.step != Step::Symbols || l.bytes_left > 0)
}

/// The first phase of a cycle for one lane: the state moves on, a byte is
/// taken if the budget allows and a held symbol moves to its slot.
fn fetch(l: &mut Lane, raw: &[u8], pos: &mut usize, budget: &mut usize) -> Result<(), BcpackError> {
    if l.done || (l.held && l.slot.is_some()) {
        l.stalled = true;
        return Ok(());
    }
    l.stalled = false;
    if l.step == Step::Symbols {
        l.state_in = match (&l.model, l.held) {
            (Some(model), true) => model.advance(l.symbol, l.state),
            _ => l.state,
        };
    }
    if wants_byte(l) {
        if *budget == 0 {
            l.stalled = true;
            return Ok(());
        }
        l.byte = *raw.get(*pos).ok_or_else(|| damaged("the data ends too early"))?;
        *pos += 1;
        l.full = true;
        *budget -= 1;
    }
    if l.held {
        l.slot = Some(l.symbol);
        l.held = false;
    }
    Ok(())
}

fn header0(l: &mut Lane, width: u8) {
    if !l.full {
        return;
    }
    let h = l.byte;
    l.quality = usize::from(h & 15);
    l.flush = h >> 4 & 1 != 0;
    l.long = h >> 5 & 1 != 0;
    l.width = usize::from(width).wrapping_sub(usize::from(h >> 6));
    l.full = false;
    l.step = Step::Header1;
    l.zeros = 0;
}

fn header1(l: &mut Lane) -> Result<(), BcpackError> {
    if !l.full {
        return Ok(());
    }
    l.full = false;
    if !(1..=MAX_WIDTH).contains(&l.width) {
        return Err(damaged("a symbol width out of range"));
    }
    l.model = Some(
        Model::get(l.width, l.quality).ok_or_else(|| damaged("no frequency model for a lane's width and quality"))?,
    );
    if l.long {
        l.bytes_left = u32::from(l.byte);
        l.step = Step::Header2;
    } else {
        l.bytes_left = u32::from(l.byte) + 1;
        l.step = Step::Symbols;
    }
    Ok(())
}

fn header2(l: &mut Lane) {
    if !l.full {
        return;
    }
    l.bytes_left = (u32::from(l.byte) << 8 | (l.bytes_left & 0xFF)) + 257;
    l.step = Step::Symbols;
    l.full = false;
}

/// Pads a flushed state with virtual zero bytes.
fn pad(st: &mut u64, zeros: &mut u8) {
    while *st < LOW && *st != 0 && *zeros < MAX_VIRTUAL_ZEROS {
        *st <<= 8;
        *zeros += 1;
    }
}

/// A state below `LOW` takes in a byte or ends its piece. `Ok(false)` means
/// the lane is finished for this cycle.
fn refill(l: &mut Lane, st: &mut u64) -> Result<bool, BcpackError> {
    if l.bytes_left > 0 {
        if l.full {
            *st = *st << 8 | u64::from(l.byte);
            l.bytes_left -= 1;
            l.full = false;
            if l.bytes_left == 0 && l.flush {
                pad(st, &mut l.zeros);
            }
        }
    } else if l.flush {
        if *st == 0 || l.zeros >= MAX_VIRTUAL_ZEROS {
            return Err(home());
        }
        pad(st, &mut l.zeros);
    } else {
        l.state = *st;
        if l.left > 0 {
            l.step = Step::Header0;
        } else {
            l.done = true;
        }
        return Ok(false);
    }
    Ok(true)
}

/// What happens when a lane has used up the bytes of its piece.
fn piece_end(l: &mut Lane, st: u64, next: bool) -> Result<(), BcpackError> {
    if l.flush {
        if st == LOW {
            if l.left > 0 {
                l.step = Step::Header0;
                l.state_in = 0;
                l.state = 0;
            } else {
                l.done = true;
            }
        } else if st > LOW && !next {
            return Err(home());
        }
    } else if l.left == 0 && !next {
        l.done = true;
    }
    Ok(())
}

/// Looks up the next symbol of a lane in its model.
fn lookup_symbol(l: &mut Lane, st: u64) -> Result<(), BcpackError> {
    let Some(model) = &l.model else { return Err(damaged("symbols without a model")) };
    l.symbol = model.lookup((st & 0xFFFF) as u32);
    l.left -= 1;
    if l.symbol >> l.width != 0 {
        return Err(damaged("a symbol outside its width"));
    }
    Ok(())
}

fn symbols(l: &mut Lane) -> Result<(), BcpackError> {
    let mut st = l.state_in;
    if st < LOW && !refill(l, &mut st)? {
        return Ok(());
    }
    let next = st > LOW && l.left > 0;
    if next {
        lookup_symbol(l, st)?;
    }
    l.held = next;
    l.state = st;
    if l.bytes_left == 0 { piece_end(l, st, next) } else { mid_piece(l, st, next) }
}

/// A piece that still has bytes must not be at its home state yet.
fn mid_piece(l: &Lane, st: u64, next: bool) -> Result<(), BcpackError> {
    if st == LOW || (l.left == 0 && !next && st > LOW) { Err(home()) } else { Ok(()) }
}

/// One symbol per lane is released when every lane has one.
fn release(lanes: &mut [Lane]) {
    let ready = lanes.iter().all(|l| l.slot.is_some() || (l.done && !l.held));
    if ready && lanes.iter().any(|l| l.slot.is_some()) {
        for l in &mut *lanes {
            if let Some(symbol) = l.slot.take() {
                l.out.push(symbol);
            }
        }
    }
    for l in lanes {
        slot_last_symbol(l);
    }
}

/// A finished lane's last symbol moves to its slot.
fn slot_last_symbol(l: &mut Lane) {
    if l.done && l.held && l.slot.is_none() {
        l.slot = Some(l.symbol);
        l.held = false;
    }
}

fn finished(lanes: &[Lane]) -> bool {
    lanes.iter().all(|l| l.done && l.slot.is_none())
}

/// The state moves on, bytes are handed out (eight per cycle, in lane
/// order) and decoded symbols move to their slots.
fn fetch_all(lanes: &mut [Lane], raw: &[u8], pos: &mut usize) -> Result<(), BcpackError> {
    let mut budget = BYTES_PER_CYCLE;
    for l in lanes {
        fetch(l, raw, pos, &mut budget)?;
    }
    Ok(())
}

/// Headers are read, bytes merged into the states, symbols looked up.
fn step_all(lanes: &mut [Lane], widths: &[u8]) -> Result<(), BcpackError> {
    for (i, l) in lanes.iter_mut().enumerate() {
        if l.done || l.stalled {
            continue;
        }
        match l.step {
            Step::Header0 => header0(l, widths[i]),
            Step::Header1 => header1(l)?,
            Step::Header2 => header2(l),
            Step::Symbols => symbols(l)?,
        }
    }
    Ok(())
}

/// The entropy layer: `count` symbols for each lane.
pub(super) fn decode_lanes(raw: &[u8], widths: &[u8], count: usize) -> Result<Vec<Vec<u32>>, BcpackError> {
    let mut lanes: Vec<Lane> = widths
        .iter()
        .map(|_| Lane {
            step: Step::Header0,
            left: count,
            width: 0,
            quality: 0,
            long: false,
            flush: false,
            done: count == 0,
            full: false,
            held: false,
            zeros: 0,
            byte: 0,
            symbol: 0,
            bytes_left: 0,
            state_in: 0,
            state: 0,
            model: None,
            slot: None,
            stalled: false,
            out: Vec::with_capacity(count),
        })
        .collect();
    let mut pos = 0;
    while !finished(&lanes) {
        fetch_all(&mut lanes, raw, &mut pos)?;
        step_all(&mut lanes, widths)?;
        release(&mut lanes);
    }
    if raw[pos..].iter().any(|b| *b != 0) {
        return Err(damaged("data is left over after the last lane"));
    }
    if lanes.iter().any(|l| l.out.len() != count) {
        return Err(damaged("a lane ends before its last symbol"));
    }
    Ok(lanes.into_iter().map(|l| l.out).collect())
}
