//! Bit reader, least significant bit first.

use super::tables::Vlc;

/// Reads past the end give zeros; [`Bits::overread`] tells.
pub struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The same position in data that ends after `len` bytes.
    pub fn truncated(&self, len: usize) -> Self {
        Self { data: &self.data[..len.min(self.data.len())], pos: self.pos }
    }

    fn peek(&self) -> u64 {
        let at = self.pos / 8;
        let word = match self.data.get(at..at + 8) {
            Some(b) => u64::from_le_bytes(b.try_into().expect("8 bytes")),
            None => {
                let mut b = [0; 8];
                let rest = self.data.get(at..).unwrap_or_default();
                b[..rest.len()].copy_from_slice(rest);
                u64::from_le_bytes(b)
            }
        };
        word >> (self.pos % 8)
    }

    /// Up to 32 bits.
    pub fn read(&mut self, n: u32) -> u32 {
        let v = self.peek() & ((1u64 << n) - 1);
        self.pos += n as usize;
        v as u32
    }

    pub fn bit(&mut self) -> bool {
        let bit = self.data.get(self.pos / 8).is_some_and(|b| (b >> (self.pos % 8)) & 1 != 0);
        self.pos += 1;
        bit
    }

    /// Counts bits until one equal to `stop` (read too), at most `max`.
    pub fn unary(&mut self, stop: bool, max: u32) -> u32 {
        (0..max).find(|_| self.bit() == stop).unwrap_or(max)
    }

    pub fn vlc(&mut self, table: &Vlc) -> Option<u8> {
        let (symbol, len) = table[(self.peek() & 511) as usize];
        self.pos += len as usize;
        (len > 0).then_some(symbol)
    }

    /// Skips to a multiple of `bits` from the start.
    pub fn align(&mut self, bits: usize) {
        self.pos = self.pos.next_multiple_of(bits);
    }

    pub fn overread(&self) -> bool {
        self.pos > self.data.len() * 8
    }

    /// Bits left to read.
    pub fn left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads() {
        let mut b = Bits::new(&[0b1010_0110, 0xFF, 0x01]);
        assert_eq!(b.read(3), 0b110);
        assert_eq!(b.unary(true, 4), 2);
        assert_eq!(b.read(2), 0b10);
        assert_eq!(b.read(9), 0x1FF);
        assert!(!b.overread());
        assert_eq!(b.read(20), 0);
        assert!(b.overread());
        assert_eq!(b.left(), 0);
    }

    #[test]
    fn unary_stops_at_the_limit() {
        let mut b = Bits::new(&[0b0100_0000]);
        assert_eq!(b.unary(true, 4), 4);
        assert_eq!(b.unary(true, 4), 2);
    }
}
