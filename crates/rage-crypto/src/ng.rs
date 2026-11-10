//! The "NG" cipher used for most GTA V PC archives.
//!
//! NG is a white-box AES variant: 17 rounds of table lookups, each XORed with
//! a 16-byte round key. Rounds 1, 2 and 17 use a straight byte layout, rounds
//! 3–16 use the AES ShiftRows permutation.

/// Number of NG keys; the key for a file is chosen by name hash and size.
pub const NG_KEY_COUNT: usize = 101;

/// One expanded NG key: 17 round keys of four little-endian words.
pub type NgKey = [u32; 68];

/// Decryption tables: 17 rounds × 16 byte positions × 256 entries.
pub type NgTables = [[[u32; 256]; 16]; 17];

/// Decrypts `data` in place. Bytes beyond the last full 16-byte block are untouched.
pub fn decrypt_ng(data: &mut [u8], key: &NgKey, tables: &NgTables) {
    for block in data.as_chunks_mut::<16>().0 {
        let mut b = *block;
        b = round_straight(&b, &key[0..4], &tables[0]);
        b = round_straight(&b, &key[4..8], &tables[1]);
        for r in 2..=15 {
            b = round_shifted(&b, &key[r * 4..r * 4 + 4], &tables[r]);
        }
        b = round_straight(&b, &key[64..68], &tables[16]);
        *block = b;
    }
}

#[inline(always)]
fn pack(x: [u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, w) in x.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

#[inline(always)]
fn round_straight(d: &[u8; 16], k: &[u32], t: &[[u32; 256]; 16]) -> [u8; 16] {
    let l = |i: usize| t[i][d[i] as usize];
    pack([
        l(0) ^ l(1) ^ l(2) ^ l(3) ^ k[0],
        l(4) ^ l(5) ^ l(6) ^ l(7) ^ k[1],
        l(8) ^ l(9) ^ l(10) ^ l(11) ^ k[2],
        l(12) ^ l(13) ^ l(14) ^ l(15) ^ k[3],
    ])
}

#[inline(always)]
fn round_shifted(d: &[u8; 16], k: &[u32], t: &[[u32; 256]; 16]) -> [u8; 16] {
    let l = |i: usize| t[i][d[i] as usize];
    pack([
        l(0) ^ l(7) ^ l(10) ^ l(13) ^ k[0],
        l(1) ^ l(4) ^ l(11) ^ l(14) ^ k[1],
        l(2) ^ l(5) ^ l(8) ^ l(15) ^ k[2],
        l(3) ^ l(6) ^ l(9) ^ l(12) ^ k[3],
    ])
}
