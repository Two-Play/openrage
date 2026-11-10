//! AES-256-ECB as used by RAGE: no padding, a trailing partial block stays plain.

use aes::Aes256;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};

/// Decrypts `data` in place. Bytes beyond the last full 16-byte block are untouched.
pub fn decrypt_aes(data: &mut [u8], key: &[u8; 32]) {
    let cipher = Aes256::new(GenericArray::from_slice(key));
    for block in data.as_chunks_mut::<16>().0 {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
    }
}

/// Encrypts `data` in place. Bytes beyond the last full 16-byte block are untouched.
pub fn encrypt_aes(data: &mut [u8], key: &[u8; 32]) {
    let cipher = Aes256::new(GenericArray::from_slice(key));
    for block in data.as_chunks_mut::<16>().0 {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_partial_tail_plain() {
        let key = [7u8; 32];
        let original: Vec<u8> = (0..37u8).collect();
        let mut data = original.clone();
        encrypt_aes(&mut data, &key);
        assert_ne!(data[..32], original[..32]);
        assert_eq!(data[32..], original[32..]);
        decrypt_aes(&mut data, &key);
        assert_eq!(data, original);
    }
}
