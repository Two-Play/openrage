//! TFIT-CTR: reference block 0x1456437C0, stream 0x1455DC010,
//! context loader 0x1455DC890. The counter is a big-endian 128-bit integer.
use super::*;

pub struct TfitCtr {
    material: CryptoMaterial,
    counter: u128,
    initial_counter: u128,
    stream: [u8; 16],
    used: usize,
}
impl TfitCtr {
    pub fn new(material: CryptoMaterial, iv: [u8; 16]) -> Result<Self, Rpf9Error> {
        if u32_at(&material.key_blob, 0) != Some(CTR_KEY_BLOB_MAGIC) {
            return Err(Rpf9Error::InvalidMaterial("CTR key-blob magic".into()));
        }
        Ok(Self {
            material,
            counter: u128::from_be_bytes(iv),
            initial_counter: u128::from_be_bytes(iv),
            stream: [0; 16],
            used: 16,
        })
    }

    fn block(&self, input: [u8; 16]) -> [u8; 16] {
        let key = |i| u32_at(&self.material.key_blob, 4 + i * 4).expect("validated key");
        let mut words: [u32; 4] = std::array::from_fn(|g| {
            let bytes =
                std::array::from_fn(|i| self.material.fixed_tables[(g * 4 + i) * 256 + usize::from(input[g * 4 + i])]);
            u32::from_be_bytes(bytes) ^ key(g)
        });
        let groups = [[0, 5, 10, 15], [3, 4, 9, 14], [2, 7, 8, 13], [1, 6, 11, 12]];
        for round in 1..14 {
            let bytes: [u8; 16] = std::array::from_fn(|i| words[i / 4].to_be_bytes()[i % 4]);
            words = std::array::from_fn(|g| {
                groups[g].iter().fold(key(round * 4 + g), |word, &t| {
                    word ^ u32_at(&self.material.fixed_tables, 0x1000 + t * 0x400 + usize::from(bytes[t]) * 4)
                        .expect("validated tables")
                })
            });
        }
        let bytes: [u8; 16] = std::array::from_fn(|i| words[i / 4].to_be_bytes()[i % 4]);
        let permutation = [0, 5, 10, 15, 4, 9, 14, 3, 8, 13, 2, 7, 12, 1, 6, 11];
        std::array::from_fn(|i| self.material.key_blob[0xf4 + i * 256 + usize::from(bytes[permutation[i]])])
    }

    /// Selects an absolute byte position, including the partial counter block.
    /// The native range wrapper adds position >> 4 to the original IV.
    pub fn seek(&mut self, position: u64) {
        self.counter = self.initial_counter.wrapping_add(u128::from(position / 16));
        self.used = 16;
        if !position.is_multiple_of(16) {
            self.stream = self.block(self.counter.to_be_bytes());
            self.counter = self.counter.wrapping_add(1);
            self.used = (position % 16) as usize;
        }
    }

    /// Applies the keystream in place, retaining partial-block state across calls.
    pub fn apply(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            if self.used == 16 {
                self.stream = self.block(self.counter.to_be_bytes());
                self.counter = self.counter.wrapping_add(1);
                self.used = 0;
            }
            *byte ^= self.stream[self.used];
            self.used += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_chunks_preserve_ctr_state() {
        let mut key = vec![0; KEY_BLOB_SIZE];
        key[..4].copy_from_slice(&CTR_KEY_BLOB_MAGIC.to_le_bytes());
        for i in 0..16 {
            for j in 0..256 {
                key[0xf4 + i * 256 + j] = (i * 17 + j) as u8;
            }
        }
        let material =
            CryptoMaterial::from_parts_with_magic(key, vec![0; FIXED_TABLES_SIZE], [0; 16], CTR_KEY_BLOB_MAGIC)
                .unwrap();
        let mut whole = vec![0x77; 197];
        TfitCtr::new(material.clone(), [0xff; 16]).unwrap().apply(&mut whole);
        let mut chunks = vec![0x77; 197];
        let mut ctr = TfitCtr::new(material, [0xff; 16]).unwrap();
        for chunk in chunks.chunks_mut(7) {
            ctr.apply(chunk);
        }
        assert_eq!(chunks, whole);
        assert_ne!(whole, vec![0x77; 197]);
    }
}
