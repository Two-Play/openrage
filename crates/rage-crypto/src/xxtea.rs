//! Rockstar's variant of XXTEA ("corrected block TEA") for audio containers:
//! the key term is additionally XORed with a constant. The whole buffer is
//! one block of 32-bit little-endian words; trailing bytes that do not fill a
//! word stay as they are.

const DELTA: u32 = 0x9E37_79B9;
/// Rockstar's change to the standard round function.
const RAGE_XOR: u32 = 0x7B3A_207F;

fn mx(sum: u32, y: u32, z: u32, p: usize, e: u32, key: &[u32; 4], xor: u32) -> u32 {
    (((z >> 5) ^ (y << 2)).wrapping_add((y >> 3) ^ (z << 4)))
        ^ ((sum ^ y).wrapping_add(key[(p & 3) ^ e as usize] ^ z ^ xor))
}

fn key_words(key: &[u8; 16]) -> [u32; 4] {
    std::array::from_fn(|i| u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap()))
}

fn load(data: &[u8]) -> Vec<u32> {
    data.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect()
}

fn store(words: &[u32], data: &mut [u8]) {
    for (w, c) in words.iter().zip(data.as_chunks_mut::<4>().0) {
        c.copy_from_slice(&w.to_le_bytes());
    }
}

/// Decrypts with Rockstar's variant.
pub fn decrypt(data: &mut [u8], key: &[u8; 16]) {
    decrypt_with(data, key, RAGE_XOR)
}

/// Encrypts with Rockstar's variant.
pub fn encrypt(data: &mut [u8], key: &[u8; 16]) {
    encrypt_with(data, key, RAGE_XOR)
}

fn decrypt_with(data: &mut [u8], key: &[u8; 16], xor: u32) {
    let mut v = load(data);
    let n = v.len();
    if n < 2 {
        return;
    }
    let k = key_words(key);
    let rounds = 6 + 52 / n as u32;
    let mut sum = rounds.wrapping_mul(DELTA);
    let mut y = v[0];
    for _ in 0..rounds {
        let e = (sum >> 2) & 3;
        for p in (1..n).rev() {
            let z = v[p - 1];
            v[p] = v[p].wrapping_sub(mx(sum, y, z, p, e, &k, xor));
            y = v[p];
        }
        let z = v[n - 1];
        v[0] = v[0].wrapping_sub(mx(sum, y, z, 0, e, &k, xor));
        y = v[0];
        sum = sum.wrapping_sub(DELTA);
    }
    store(&v, data);
}

fn encrypt_with(data: &mut [u8], key: &[u8; 16], xor: u32) {
    let mut v = load(data);
    let n = v.len();
    if n < 2 {
        return;
    }
    let k = key_words(key);
    let rounds = 6 + 52 / n as u32;
    let mut sum: u32 = 0;
    let mut z = v[n - 1];
    for _ in 0..rounds {
        sum = sum.wrapping_add(DELTA);
        let e = (sum >> 2) & 3;
        for p in 0..n - 1 {
            let y = v[p + 1];
            v[p] = v[p].wrapping_add(mx(sum, y, z, p, e, &k, xor));
            z = v[p];
        }
        let y = v[0];
        v[n - 1] = v[n - 1].wrapping_add(mx(sum, y, z, n - 1, e, &k, xor));
        z = v[n - 1];
    }
    store(&v, data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = *b"0123456789abcdef";
        let original: Vec<u8> = (0..103u8).collect();
        let mut data = original.clone();
        encrypt(&mut data, &key);
        assert_ne!(data[..100], original[..100]);
        assert_eq!(data[100..], original[100..], "trailing bytes stay");
        decrypt(&mut data, &key);
        assert_eq!(data, original);
    }

    #[test]
    fn reference_vector() {
        // Well-known XXTEA vector (standard variant): key all zero, two zero words.
        let mut data = [0u8; 8];
        encrypt_with(&mut data, &[0; 16], 0);
        assert_eq!(u32::from_le_bytes(data[0..4].try_into().unwrap()), 0x053704ab);
        assert_eq!(u32::from_le_bytes(data[4..8].try_into().unwrap()), 0x575d8c80);
    }
}
