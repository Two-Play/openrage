//! Bounds-checked reads of numbers from byte slices.
//!
//! Game files are untrusted input: every read here returns `None` instead of
//! panicking when it would run past the end. Callers turn `None` into their
//! own error type.
//!
//! ```
//! use rage_core::bytes::{Cursor, LeRead};
//!
//! let data = [1u8, 0, 2, 0, 0, 0];
//! assert_eq!(data.u16_at(0), Some(1));
//! assert_eq!(data.u32_at(2), Some(2));
//! assert_eq!(data.u32_at(4), None);
//!
//! let mut c = Cursor::new(&data);
//! assert_eq!(c.u16(), Some(1));
//! assert_eq!(c.u32(), Some(2));
//! assert_eq!(c.u8(), None);
//! ```

/// Little-endian reads at absolute offsets.
pub trait LeRead {
    /// `N` bytes at `at`, or `None` past the end.
    fn array_at<const N: usize>(&self, at: usize) -> Option<[u8; N]>;

    fn u8_at(&self, at: usize) -> Option<u8> {
        self.array_at::<1>(at).map(|b| b[0])
    }
    fn u16_at(&self, at: usize) -> Option<u16> {
        self.array_at(at).map(u16::from_le_bytes)
    }
    fn u32_at(&self, at: usize) -> Option<u32> {
        self.array_at(at).map(u32::from_le_bytes)
    }
    fn u64_at(&self, at: usize) -> Option<u64> {
        self.array_at(at).map(u64::from_le_bytes)
    }
    fn i16_at(&self, at: usize) -> Option<i16> {
        self.array_at(at).map(i16::from_le_bytes)
    }
    fn i32_at(&self, at: usize) -> Option<i32> {
        self.array_at(at).map(i32::from_le_bytes)
    }
    fn f32_at(&self, at: usize) -> Option<f32> {
        self.array_at(at).map(f32::from_le_bytes)
    }
    /// An IEEE half-precision float, widened (see [`f16_to_f32`]).
    fn f16_at(&self, at: usize) -> Option<f32> {
        self.u16_at(at).map(f16_to_f32)
    }
}

impl LeRead for [u8] {
    #[inline]
    fn array_at<const N: usize>(&self, at: usize) -> Option<[u8; N]> {
        self.get(at..at.checked_add(N)?)?.try_into().ok()
    }
}

/// Big-endian reads at absolute offsets (PSO metadata, a few console formats).
pub trait BeRead: LeRead {
    fn u16_be_at(&self, at: usize) -> Option<u16> {
        self.array_at(at).map(u16::from_be_bytes)
    }
    fn u32_be_at(&self, at: usize) -> Option<u32> {
        self.array_at(at).map(u32::from_be_bytes)
    }
    fn u64_be_at(&self, at: usize) -> Option<u64> {
        self.array_at(at).map(u64::from_be_bytes)
    }
    fn f32_be_at(&self, at: usize) -> Option<f32> {
        self.array_at(at).map(f32::from_be_bytes)
    }
}

impl BeRead for [u8] {}

/// A little-endian reader that moves forward through a slice.
#[derive(Clone, Debug)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// A cursor that starts at `pos`.
    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Bytes left after the position (0 when it lies past the end).
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// The next `n` bytes, or `None` (without moving) past the end.
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(b)
    }

    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let a = self.data.array_at(self.pos)?;
        self.pos += N;
        Some(a)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.array::<1>().map(|b| b[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        self.array().map(u16::from_le_bytes)
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_le_bytes)
    }
    pub fn u64(&mut self) -> Option<u64> {
        self.array().map(u64::from_le_bytes)
    }
    pub fn i32(&mut self) -> Option<i32> {
        self.array().map(i32::from_le_bytes)
    }
    pub fn f32(&mut self) -> Option<f32> {
        self.array().map(f32::from_le_bytes)
    }
}

/// Widens an IEEE 754 half-precision float (binary16) to `f32`, including
/// subnormals, infinities and NaN.
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1F);
    let frac = f32::from(bits & 0x3FF);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        31 if frac == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stop_at_the_end() {
        let d = [0x78u8, 0x56, 0x34, 0x12, 0xFF];
        assert_eq!(d.u32_at(0), Some(0x1234_5678));
        assert_eq!(d.u32_be_at(0), Some(0x7856_3412));
        assert_eq!(d.u32_at(1), Some(0xFF12_3456));
        assert_eq!(d.u32_at(2), None);
        assert_eq!(d.u64_at(0), None);
        assert_eq!(d.u16_at(usize::MAX), None);
        assert_eq!(d.u8_at(4), Some(0xFF));
    }

    #[test]
    fn cursor_does_not_move_on_failure() {
        let d = [1u8, 2, 3];
        let mut c = Cursor::new(&d);
        assert_eq!(c.u16(), Some(0x0201));
        assert_eq!(c.u16(), None);
        assert_eq!(c.pos(), 2);
        assert_eq!(c.remaining(), 1);
        assert_eq!(c.u8(), Some(3));
    }

    #[test]
    fn half_floats() {
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert_eq!(f16_to_f32(0xFC00), f32::NEG_INFINITY);
        assert!(f16_to_f32(0x7E00).is_nan());
    }
}
