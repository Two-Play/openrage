// SPDX-License-Identifier: AGPL-3.0-only
//
// Tests with random tables (no game data). The expected bytes come from a
// separate Python implementation written from the reference cipher.

use super::*;

struct XorShift(u64);

impl XorShift {
    fn fill(&mut self, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n + 8);
        while out.len() < n {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            out.extend(self.0.to_le_bytes());
        }
        out.truncate(n);
        out
    }
}

fn random_context() -> Context {
    let mut b = XorShift(0x1234_5678_90AB_CDEF).fill(CONTEXT_LEN);
    for r in 0..17 {
        for k in 0..16 {
            let at = 32768 + r * ROUND_LEN + 32768 + k * BLOCK_LEN + 128;
            let x = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) & 0xFFF;
            b[at..at + 4].copy_from_slice(&x.to_le_bytes());
        }
    }
    Context::from_bytes(&b).unwrap()
}

#[test]
fn layout_sizes() {
    assert_eq!(CONTEXT_LEN, 631_952);
    assert_eq!(KEY_LEN, 288);
}

#[test]
fn cbc_matches_the_reference() {
    let ctx = random_context();
    let key = Key::from_bytes(&XorShift(0xCAFE_BABE_DEAD_BEEF).fill(KEY_LEN)).unwrap();
    let iv: [u8; 16] = XorShift(0x0F1E_2D3C_4B5A_6978).fill(16).try_into().unwrap();
    let mut data = XorShift(0x1122_3344_5566_7788).fill(64);
    Cbc::new(&ctx, &key, iv).decrypt(&mut data);
    let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex,
        "9b207739e99cbcc6fb960dd347a5a84a8e29c2f7ca2672f0f473c101b1f57c04\
         4e138e6c41417dd7fe7e9ef602e33c4f7dea162fe6233cf711ac17284649d70e"
    );

    // The chain carries over between calls, as for a file's strided ranges.
    let mut split = XorShift(0x1122_3344_5566_7788).fill(64);
    let mut cbc = Cbc::new(&ctx, &key, iv);
    let (a, b) = split.split_at_mut(32);
    cbc.decrypt(a);
    cbc.decrypt(b);
    assert_eq!(split, data);
}

#[test]
fn strided_layout() {
    // Config 0: only the last 0x400 bytes.
    assert_eq!(strided_ranges(0, 0x3000, 0x1000), vec![(0x2C00, 0x3000)]);
    // Small files: the head covers everything.
    assert_eq!(strided_ranges(1, 0x300, 0x1000), vec![(0, 0x300)]);
    // Head 0x1000, blocks of 0x800 every 0x10000, then the tail.
    assert_eq!(
        strided_ranges(1 | 1 << 2, 0x25000, 0x1000),
        vec![(0, 0x1000), (0x10000, 0x10800), (0x20000, 0x20800), (0x24C00, 0x25000)]
    );
    // Without a head, the first block starts at 0.
    assert_eq!(strided_ranges(1 << 2, 0x12000, 0x1000), vec![(0, 0x800), (0x10000, 0x10800), (0x11C00, 0x12000)]);
    assert_eq!(chunk_size(true, true, 5), 0x80000);
    assert_eq!(chunk_size(false, false, 5), 0x1000);
}
