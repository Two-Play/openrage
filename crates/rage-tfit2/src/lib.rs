// SPDX-License-Identifier: AGPL-3.0-only
//
// Ported to Rust from RDR2-RPF-Tool by amrshaheen61
// (https://github.com/amrshaheen61/RDR2-RPF-Tool, Core/Tfit2CbcCipher.cs and
// Core/StridedCipher.cs), licensed under the GNU Affero General Public License
// v3.0. This crate stays under that license; the rest of OpenRAGE is GPL-3.0,
// which may be combined with it (GPL-3.0 section 13).
//
// No key material is included: the cipher tables, IV and keys come from the
// user's own game at runtime.

//! The TFIT2 white-box cipher (in CBC mode) that encrypts Red Dead
//! Redemption 2 PC archives, and the strided layout of encrypted file data.
//!
//! Decryption needs three parts the user's game provides: the shared
//! [`Context`] (≈ 617 KiB of tables), a 16-byte IV, and one [`Key`] per key id.

/// Size of the cipher tables as laid out in the game's memory.
pub const CONTEXT_LEN: usize = 16 * 256 * 8 + 17 * ROUND_LEN + 16 * 8 * 8 + 16 * 256 + 16;
/// Size of one key in the game's memory (18 × 2 u64).
pub const KEY_LEN: usize = 18 * 16;
/// Size of the IV.
pub const IV_LEN: usize = 16;
const BLOCK_LEN: usize = 16 * 8 + 8; // 16 masks + u32 xor, padded to 8 bytes
const ROUND_LEN: usize = 4096 * 8 + 16 * BLOCK_LEN;

struct Block {
    masks: [u64; 16],
    xor: u32,
}

struct Round {
    lookup: Vec<u64>,
    blocks: Vec<Block>,
}

/// The shared cipher tables.
pub struct Context {
    init_tables: Vec<[u64; 256]>,
    rounds: Vec<Round>,
    end_masks: [[u64; 8]; 16],
    end_tables: Vec<[u8; 256]>,
    end_xor: [u8; 16],
}

/// One key: 17 round keys (the first of the 18 stored pairs is unused).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    rounds: [[u64; 2]; 17],
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

impl Context {
    /// Reads the tables from their in-memory layout ([`CONTEXT_LEN`] bytes).
    pub fn from_bytes(b: &[u8]) -> Option<Context> {
        if b.len() != CONTEXT_LEN {
            return None;
        }
        let mut at = 0;
        let mut init_tables = Vec::with_capacity(16);
        for _ in 0..16 {
            let mut t = [0u64; 256];
            for v in &mut t {
                *v = u64_at(b, at);
                at += 8;
            }
            init_tables.push(t);
        }
        let mut rounds = Vec::with_capacity(17);
        for _ in 0..17 {
            let lookup: Vec<u64> = (0..4096).map(|i| u64_at(b, at + i * 8)).collect();
            at += 4096 * 8;
            let mut blocks = Vec::with_capacity(16);
            for _ in 0..16 {
                let mut masks = [0u64; 16];
                for (i, m) in masks.iter_mut().enumerate() {
                    *m = u64_at(b, at + i * 8);
                }
                let xor = u32::from_le_bytes(b[at + 128..at + 132].try_into().expect("4 bytes"));
                blocks.push(Block { masks, xor });
                at += BLOCK_LEN;
            }
            rounds.push(Round { lookup, blocks });
        }
        let mut end_masks = [[0u64; 8]; 16];
        for row in &mut end_masks {
            for v in row.iter_mut() {
                *v = u64_at(b, at);
                at += 8;
            }
        }
        let mut end_tables = Vec::with_capacity(16);
        for _ in 0..16 {
            let mut t = [0u8; 256];
            t.copy_from_slice(&b[at..at + 256]);
            end_tables.push(t);
            at += 256;
        }
        let mut end_xor = [0u8; 16];
        end_xor.copy_from_slice(&b[at..at + 16]);
        Some(Context { init_tables, rounds, end_masks, end_tables, end_xor })
    }
}

impl Key {
    /// Reads a key from its in-memory layout ([`KEY_LEN`] bytes).
    pub fn from_bytes(b: &[u8]) -> Option<Key> {
        if b.len() != KEY_LEN {
            return None;
        }
        let mut rounds = [[0u64; 2]; 17];
        for (i, r) in rounds.iter_mut().enumerate() {
            *r = [u64_at(b, 16 + i * 16), u64_at(b, 24 + i * 16)];
        }
        Some(Key { rounds })
    }
}

/// Spreads each of 8 bytes (in reverse order) over a whole u64.
fn splat(bytes: &[u8]) -> [u64; 8] {
    std::array::from_fn(|i| 0x0101_0101_0101_0101u64.wrapping_mul(u64::from(bytes[7 - i])))
}

/// Parity of each byte of `v`, gathered into the low byte.
fn parity_byte(mut v: u64) -> u64 {
    v ^= v >> 1;
    v ^= v >> 2;
    v ^= v >> 4;
    v &= 0x0101_0101_0101_0101;
    v |= v >> 7;
    v |= v >> 14;
    v |= v >> 28;
    v
}

fn masked(masks: &[u64], input: &[u64; 8]) -> u64 {
    masks.iter().zip(input).fold(0, |acc, (m, i)| acc ^ (m & i))
}

fn round_block(input: &[u64; 8], block: &Block, lookup: &[u64]) -> u64 {
    let lower = parity_byte(masked(&block.masks[8..16], input));
    let mut upper = masked(&block.masks[0..8], input);
    upper ^= upper << 1;
    upper ^= upper >> 2;
    upper ^= upper >> 4;
    upper &= 0x0202_0202_0202_0202;
    upper |= upper << 7;
    upper |= upper >> 14;
    lookup[((lower & 0xFF) ^ (upper & 0xF00) ^ u64::from(block.xor)) as usize]
}

/// Which half of the state (0 = low 8 bytes, 1 = high) feeds each of the 16 blocks.
const STRAIGHT: [usize; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1];
const SHIFTED: [usize; 16] = [0, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0];

fn middle_round(ctx: &Context, index: usize, state: &mut [u8; 16], key: &[u64; 2], halves: &[usize; 16]) {
    let round = &ctx.rounds[index];
    let v = [splat(&state[0..8]), splat(&state[8..16])];
    let mut out = [key[0], key[1]];
    for (i, block) in round.blocks.iter().enumerate() {
        out[i / 8] ^= round_block(&v[halves[i]], block, &round.lookup);
    }
    state[0..8].copy_from_slice(&out[0].to_le_bytes());
    state[8..16].copy_from_slice(&out[1].to_le_bytes());
}

/// Decrypts one 16-byte block (without the CBC step).
pub fn decrypt_block(ctx: &Context, key: &Key, block: &mut [u8; 16]) {
    let mut out = [0u64; 2];
    for (i, b) in block.iter().enumerate() {
        out[i / 8] ^= ctx.init_tables[i][usize::from(*b)];
    }
    block[0..8].copy_from_slice(&out[0].to_le_bytes());
    block[8..16].copy_from_slice(&out[1].to_le_bytes());

    middle_round(ctx, 0, block, &key.rounds[0], &STRAIGHT);
    middle_round(ctx, 1, block, &key.rounds[1], &STRAIGHT);
    for i in 2..16 {
        middle_round(ctx, i, block, &key.rounds[i], &SHIFTED);
    }
    middle_round(ctx, 16, block, &key.rounds[16], &STRAIGHT);

    let v = [splat(&block[0..8]), splat(&block[8..16])];
    for i in 0..16 {
        let squashed = parity_byte(masked(&ctx.end_masks[i], &v[i / 8])) as u8;
        block[i] = ctx.end_tables[i][usize::from(squashed ^ ctx.end_xor[i])];
    }
}

/// CBC decryption that keeps its chain across calls, as the game does for
/// the strided ranges of one file.
pub struct Cbc<'a> {
    ctx: &'a Context,
    key: &'a Key,
    iv: [u8; 16],
}

impl<'a> Cbc<'a> {
    pub fn new(ctx: &'a Context, key: &'a Key, iv: [u8; 16]) -> Self {
        Cbc { ctx, key, iv }
    }

    /// Decrypts the whole 16-byte blocks of `data` in place; a shorter tail stays as is.
    pub fn decrypt(&mut self, data: &mut [u8]) {
        for chunk in data.as_chunks_mut::<16>().0 {
            let cipher = *chunk;
            let mut block = cipher;
            decrypt_block(self.ctx, self.key, &mut block);
            for (o, (b, v)) in chunk.iter_mut().zip(block.iter().zip(&self.iv)) {
                *o = b ^ v;
            }
            self.iv = cipher;
        }
    }
}

/// The byte ranges of a file's data that are encrypted, from the entry's
/// encryption config. `chunk_size` is the size the game reads the file in.
pub fn strided_ranges(config: u8, file_size: u64, chunk_size: u64) -> Vec<(u64, u64)> {
    let head_length = if config & 3 != 0 { 0x400u64 << ((config & 3) * 2) } else { 0 };
    let length_config = (config >> 2) & 7;
    let (block_length, block_stride) =
        if length_config != 0 { (0x400u64 << length_config, (u64::from((config >> 5) & 7) + 1) << 16) } else { (0, 0) };

    let mut out = Vec::new();
    let mut offset = 0u64;
    let mut add = |start: u64, end: u64, offset: &mut u64| {
        let start = start.max(*offset);
        let end = end.min(file_size);
        if start < end {
            out.push((start, end));
            *offset = end;
        }
    };
    let tail_offset = file_size.saturating_sub(0x400);
    add(0, head_length, &mut offset);
    if head_length < tail_offset {
        if block_length != 0 || block_stride != 0 {
            if block_stride == chunk_size && tail_offset < block_stride && block_stride < file_size {
                offset = block_stride;
            } else {
                let mut block = if offset != 0 { block_stride } else { 0 };
                while block + block_length <= tail_offset {
                    add(block, block + block_length, &mut offset);
                    block += block_stride;
                }
            }
        }
        add(tail_offset, file_size, &mut offset);
    }
    out
}

/// The size the game reads a file in, which the strided layout depends on.
pub fn chunk_size(is_resource: bool, compressed: bool, size: u64) -> u64 {
    match (is_resource, compressed) {
        (true, true) => 0x80000,
        (true, false) => size,
        (false, true) => 0x2000,
        (false, false) => 0x1000,
    }
}

/// Decrypts a file's data in place: each encrypted range, one CBC chain.
pub fn decrypt_file(ctx: &Context, key: &Key, iv: [u8; 16], config: u8, chunk_size: u64, data: &mut [u8]) {
    let mut cbc = Cbc::new(ctx, key, iv);
    for (start, end) in strided_ranges(config, data.len() as u64, chunk_size) {
        cbc.decrypt(&mut data[start as usize..end as usize]);
    }
}

#[cfg(test)]
mod tests;
